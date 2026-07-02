//! The unified agent-facing skills subsystem.
//!
//! Generalizes the method-selection KB (`engines::methods`) along two axes:
//! its data can come from disk `SKILL.md` files, not just the compiled-in core;
//! and a skill is a general domain workflow, not only a method-selection rule.
//! The built-in KB is folded in as the [`SkillSource::Builtin`] source, so one
//! validation path, one manifest, and one lookup serve both.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::engines::methods::Engine;

/// Where a loaded skill came from. Governs name-collision precedence
/// (`Project` > `User` > `Builtin`) and is shown in the manifest for provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    Builtin,
    User,
    Project,
}

/// A declared placeholder in a skill's `.sls` command template.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillParam {
    pub name: String,
    #[serde(default)]
    pub required: bool,
}

/// One skill: a discoverable, parameterizable domain workflow. Superset of
/// [`crate::engines::methods::MethodRule`].
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    /// Method-class skills carry an engine; general workflow skills may omit it.
    pub engine: Option<Engine>,
    pub description: String,
    pub triggers: Vec<String>,
    pub params: Vec<SkillParam>,
    pub command: Vec<String>,
    pub caveats: Vec<String>,
    pub body: String,
    pub in_table: bool,
    pub source: SkillSource,
}

/// The `SKILL.md` YAML frontmatter. Deserialized on load, serialized on save.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct Frontmatter {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    engine: Option<Engine>,
    description: String,
    #[serde(default)]
    triggers: Vec<String>,
    #[serde(default)]
    params: Vec<SkillParam>,
    #[serde(default)]
    command: Vec<String>,
    #[serde(default)]
    caveats: Vec<String>,
    #[serde(default = "default_true")]
    in_table: bool,
}

fn default_true() -> bool {
    true
}

/// Parse a `SKILL.md` document (YAML frontmatter fenced by `---`, then a
/// markdown body) into a [`Skill`]. Returns a human-readable error on a missing
/// fence or a YAML parse failure.
pub fn parse_skill_md(text: &str, source: SkillSource) -> Result<Skill, String> {
    let rest = text
        .strip_prefix("---")
        .ok_or("missing YAML frontmatter fence")?;
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
        .unwrap_or(rest);
    let end = rest.find("\n---").ok_or("unterminated YAML frontmatter")?;
    let yaml = &rest[..end];
    let after = &rest[end + "\n---".len()..];
    let body = after
        .strip_prefix("\r\n")
        .or_else(|| after.strip_prefix('\n'))
        .unwrap_or(after);
    let fm: Frontmatter =
        serde_yml::from_str(yaml).map_err(|err| format!("frontmatter parse error: {err}"))?;
    Ok(Skill {
        name: fm.name,
        engine: fm.engine,
        description: fm.description,
        triggers: fm.triggers,
        params: fm.params,
        command: fm.command,
        caveats: fm.caveats,
        body: body.trim_end().to_string(),
        in_table: fm.in_table,
        source,
    })
}

/// The compiled-in method KB, lifted into [`Skill`] values under
/// [`SkillSource::Builtin`]. Phase 1 keeps the KB as the built-in source; a later
/// phase migrates it to on-disk `SKILL.md` files.
pub fn builtin_skills() -> Vec<Skill> {
    crate::engines::methods::rules()
        .iter()
        .map(|rule| Skill {
            name: rule.name.clone(),
            engine: Some(rule.engine),
            description: rule.description.clone(),
            triggers: rule.triggers.clone(),
            params: Vec::new(),
            command: rule.command.clone(),
            caveats: rule.caveats.clone(),
            body: rule.body.clone(),
            in_table: rule.in_table,
            source: SkillSource::Builtin,
        })
        .collect()
}

/// Validate a skill against the safety floor, generalizing
/// [`crate::engines::methods::validate_rule`]. Structural checks plus, for
/// engine-bearing skills, the same `--method`/`--basis` vetting the built-in KB
/// uses. This does NOT parse the `.sls` grammar — that check lives in the crate
/// that owns the grammar (see the `silicolab` skills bridge).
pub fn validate_skill(skill: &Skill) -> Result<(), String> {
    let name = &skill.name;
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(format!(
            "skill name `{name}` must be kebab-case, 1..=64 chars"
        ));
    }
    if skill.description.trim().is_empty() || skill.description.chars().count() > 200 {
        return Err(format!("{name}: description must be 1..=200 chars"));
    }
    if skill.triggers.is_empty() {
        return Err(format!("{name}: needs at least one trigger keyword"));
    }
    let declared: HashSet<&str> = skill.params.iter().map(|p| p.name.as_str()).collect();
    let mut pins_method_or_basis = false;
    for line in &skill.command {
        for placeholder in placeholders(line) {
            if !declared.contains(placeholder.as_str()) {
                return Err(format!(
                    "{name}: command references undeclared placeholder `{{{placeholder}}}`"
                ));
            }
        }
        pins_method_or_basis |= validate_command_line(skill, line)?;
    }
    if skill.engine == Some(Engine::Qm) && pins_method_or_basis && skill.caveats.is_empty() {
        return Err(format!(
            "{name}: a QM skill that pins --method/--basis must carry at least one caveat"
        ));
    }
    Ok(())
}

/// Validate one command line; returns whether it pins a `--method`/`--basis`.
fn validate_command_line(skill: &Skill, command: &str) -> Result<bool, String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let verb = tokens.first().copied().unwrap_or_default();
    if let Some(engine) = skill.engine
        && !engine.verbs().contains(&verb)
    {
        return Err(format!(
            "{}: command `{command}` starts with `{verb}`, invalid for engine {:?}",
            skill.name, engine
        ));
    }
    let mut pins = false;
    let mut index = 0;
    while index < tokens.len() {
        let flag = tokens[index];
        if flag == "--method" || flag == "--basis" {
            pins = true;
            if skill.engine.is_none() {
                return Err(format!(
                    "{}: an engine-agnostic skill may not pin `{flag}` (no engine to vet against)",
                    skill.name
                ));
            }
            let value = tokens.get(index + 1).copied().unwrap_or_default();
            // A placeholder value is chosen at run time; it can't be vetted here.
            if skill.engine == Some(Engine::Qm) && !is_placeholder(value) {
                let vetted = if flag == "--method" {
                    crate::engines::methods::is_vetted_qm_method(value)
                } else {
                    crate::engines::methods::is_vetted_qm_basis(value)
                };
                if !vetted {
                    return Err(format!(
                        "{}: `{flag} {value}` is not on the vetted list",
                        skill.name
                    ));
                }
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(pins)
}

fn is_placeholder(token: &str) -> bool {
    token.starts_with('{') && token.ends_with('}') && token.len() >= 2
}

/// Extract the `{name}` placeholder names referenced in a command line.
fn placeholders(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'{'
            && let Some(close) = line[index + 1..].find('}')
        {
            let name = &line[index + 1..index + 1 + close];
            if !name.is_empty() {
                out.push(name.to_string());
            }
            index = index + 1 + close + 1;
            continue;
        }
        index += 1;
    }
    out
}

/// The always-on decision manifest injected into the (cacheable) system prompt.
/// Generalizes [`crate::engines::methods::kb_table`]: engine-bearing skills are
/// grouped under their engine heading; engine-agnostic skills under "Workflows".
/// Only `in_table` skills appear, keeping the per-turn cost bounded. Non-builtin
/// skills are annotated with their source for provenance.
pub fn skills_manifest(skills: &[Skill]) -> String {
    // The manifest is spliced into every system prompt, so its size must stay
    // bounded no matter how many `in_table` skills are loaded from disk. Enforce
    // the same budget the prompt was designed around at render time; the long
    // tail that does not fit is dropped with a pointer to `recommend_method`.
    const MAX_LINES: usize = 100;
    const MAX_CHARS: usize = 6000;
    // Reserve headroom for the one-line overflow tail so the final string is
    // guaranteed to stay strictly within the budget.
    const SOFT_LINES: usize = MAX_LINES - 2;
    const SOFT_CHARS: usize = MAX_CHARS - 120;

    let mut out = String::from(
        "Skills — method guidance and reusable workflows: pick one and fill its \
         template, then act. Reach for QM (`qm …`) for electronic structure, \
         energies, and spectra of molecules and small systems; MD (`md …`) for \
         dynamics, solvation, and large flexible systems; docking (`dock …`) for \
         ligand–receptor poses; periodic QM (`qm periodic`) for crystals. Call the \
         `recommend_method` tool for the full details/caveats of any line, and \
         `qm recommend <task>` for the curated QM level of theory.\n",
    );

    // Ordered sections: one per engine (built-in KB + engine-tagged skills), then
    // engine-agnostic workflows last, so truncation drops the tail — not the core
    // method guidance.
    let mut sections: Vec<(String, Vec<&Skill>)> = Vec::new();
    for engine in Engine::ALL {
        let group: Vec<&Skill> = skills
            .iter()
            .filter(|s| s.engine == Some(engine) && s.in_table)
            .collect();
        if !group.is_empty() {
            sections.push((format!("{}:", engine.heading()), group));
        }
    }
    let workflows: Vec<&Skill> = skills
        .iter()
        .filter(|s| s.engine.is_none() && s.in_table)
        .collect();
    if !workflows.is_empty() {
        sections.push(("Workflows:".to_string(), workflows));
    }
    let total: usize = sections.iter().map(|(_, group)| group.len()).sum();

    let mut emitted = 0usize;
    for (heading, group) in &sections {
        let mut heading_written = false;
        for skill in group {
            // Format the entry (plus its heading, if this is the section's first)
            // into a chunk, then check the projected size before committing it.
            let mut chunk = String::new();
            if !heading_written {
                chunk.push('\n');
                chunk.push_str(heading);
                chunk.push('\n');
            }
            write_manifest_entry(&mut chunk, skill);

            let projected_chars = out.chars().count() + chunk.chars().count();
            let projected_lines = count_lines(&out) + count_lines(&chunk);
            // Always emit at least one entry so the core is never empty; past
            // that, stop before the soft budget and append the overflow tail.
            // The unconditional first entry stays within the hard budget because
            // `write_manifest_entry` clamps each entry on its own.
            if emitted > 0 && (projected_chars > SOFT_CHARS || projected_lines > SOFT_LINES) {
                let remaining = total - emitted;
                out.push_str(&format!(
                    "… +{remaining} more — use recommend_method to surface them.\n"
                ));
                return out;
            }
            out.push_str(&chunk);
            heading_written = true;
            emitted += 1;
        }
    }
    out
}

/// Count the newline-terminated lines in `s`. For the manifest — whose every
/// line ends in `\n` — this matches `str::lines().count()`, the metric the
/// budget test asserts against.
fn count_lines(s: &str) -> usize {
    s.bytes().filter(|&b| b == b'\n').count()
}

fn write_manifest_entry(out: &mut String, skill: &Skill) {
    // Per-entry bounds. `skills_manifest` always emits the first entry even
    // when it alone would blow the soft budget (the manifest must never be
    // empty), so a single pathological `in_table` skill has to be clamped
    // here for the hard budget to hold.
    const MAX_ENTRY_LINES: usize = 12;
    const MAX_ENTRY_CHARS: usize = 1200;

    let mut entry = String::from("- ");
    // Validation caps descriptions at 200 chars, but the manifest must stay
    // bounded even for skills that never went through `validate_skill`.
    for c in skill.description.chars().take(200) {
        entry.push(if c == '\n' { ' ' } else { c });
    }
    match skill.source {
        SkillSource::Builtin => {}
        SkillSource::User => entry.push_str(" [user]"),
        SkillSource::Project => entry.push_str(" [project]"),
    }
    entry.push('\n');
    for line in &skill.command {
        let projected_chars = entry.chars().count() + line.chars().count() + 5;
        let projected_lines = count_lines(&entry) + 1 + line.matches('\n').count();
        if projected_chars > MAX_ENTRY_CHARS || projected_lines > MAX_ENTRY_LINES {
            entry.push_str("    … (recommend_method has the full template)\n");
            break;
        }
        entry.push_str("    ");
        entry.push_str(line);
        entry.push('\n');
    }
    out.push_str(&entry);
}

/// On-demand guidance for a free-text task: keyword-scores every skill and
/// returns the best one or two in full. Generalizes
/// [`crate::engines::methods::recommend`]. Never errors; an unmatched task
/// returns the full manifest.
pub fn recommend(skills: &[Skill], task: &str) -> String {
    let query = task.to_ascii_lowercase();
    let terms: Vec<&str> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| term.len() > 2)
        .collect();

    let mut scored: Vec<(usize, &Skill)> = skills
        .iter()
        .map(|skill| (score_skill(skill, &terms), skill))
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));

    if scored.is_empty() {
        return format!(
            "No specific skill matched \u{201c}{task}\u{201d}. The full guide:\n\n{}",
            skills_manifest(skills)
        );
    }
    let mut out = String::new();
    for (_, skill) in scored.iter().take(2) {
        out.push_str(&render_detail(skill));
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn score_skill(skill: &Skill, terms: &[&str]) -> usize {
    let haystack = format!(
        "{} {} {}",
        skill.name,
        skill.description,
        skill.triggers.join(" ")
    )
    .to_ascii_lowercase();
    terms
        .iter()
        .map(|term| {
            if skill
                .triggers
                .iter()
                .any(|tr| tr.eq_ignore_ascii_case(term))
            {
                3
            } else if haystack.contains(term) {
                1
            } else {
                0
            }
        })
        .sum()
}

fn render_detail(skill: &Skill) -> String {
    let mut out = format!("## {}\n{}\n", skill.name, skill.description);
    if !skill.params.is_empty() {
        out.push_str("params: ");
        let names: Vec<String> = skill
            .params
            .iter()
            .map(|p| {
                if p.required {
                    format!("{} (required)", p.name)
                } else {
                    p.name.clone()
                }
            })
            .collect();
        out.push_str(&names.join(", "));
        out.push('\n');
    }
    if !skill.command.is_empty() {
        out.push_str("run:\n");
        for line in &skill.command {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
    for caveat in &skill.caveats {
        out.push_str("- caveat: ");
        out.push_str(caveat);
        out.push('\n');
    }
    if !skill.body.trim().is_empty() {
        out.push('\n');
        out.push_str(skill.body.trim());
        out.push('\n');
    }
    out
}

/// Load skills from all sources, merged with precedence
/// `Project` > `User` > `Builtin`. Each source directory is scanned for
/// `<name>/SKILL.md` files. A file that fails to parse or validate is skipped
/// with a warning (never a panic). The built-in KB is always included.
pub fn load_skills(user_dir: Option<&Path>, project_dir: Option<&Path>) -> Vec<Skill> {
    use std::collections::BTreeMap;
    // BTreeMap keyed by name gives a stable, sorted order and last-write-wins.
    let mut by_name: BTreeMap<String, Skill> = BTreeMap::new();
    for skill in builtin_skills() {
        by_name.insert(skill.name.clone(), skill);
    }
    if let Some(dir) = user_dir {
        for skill in scan_dir(dir, SkillSource::User) {
            by_name.insert(skill.name.clone(), skill);
        }
    }
    if let Some(dir) = project_dir {
        for skill in scan_dir(dir, SkillSource::Project) {
            by_name.insert(skill.name.clone(), skill);
        }
    }
    by_name.into_values().collect()
}

/// Scan one source directory for `<name>/SKILL.md`, parsing and validating each.
/// Invalid entries are logged and skipped.
fn scan_dir(dir: &Path, source: SkillSource) -> Vec<Skill> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return out, // missing dir is normal, not an error
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!("skills: skipping unreadable dir entry: {err}");
                continue;
            }
        };
        let path = entry.path().join("SKILL.md");
        if !path.is_file() {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("skills: could not read {}: {err}", path.display());
                continue;
            }
        };
        match parse_skill_md(&text, source) {
            Ok(skill) => match validate_skill(&skill) {
                Ok(()) => out.push(skill),
                Err(reason) => eprintln!("skills: skipping {}: {reason}", path.display()),
            },
            Err(reason) => eprintln!("skills: skipping {}: {reason}", path.display()),
        }
    }
    out
}

#[cfg(test)]
mod tests;
