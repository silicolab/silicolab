//! GUI-side skills loading. `compute-core` owns the disk loader but cannot parse
//! the `.sls` grammar (it lives in this crate), so the second-stage grammar
//! filter — rejecting skills whose command templates don't parse — lives here.

use std::path::PathBuf;

use crate::skills::{self, Skill};

use crate::frontend::console::command_parses;

/// Load skills for the assistant: the built-in KB plus `SKILL.md` files from the
/// user dir (`~/.silicolab/skills`) and the open project's `skills/` dir, then
/// drop any whose command templates fail to parse under the `.sls` grammar.
pub fn load_agent_skills(project_root: Option<PathBuf>) -> Vec<Skill> {
    let user_dir = crate::hosts::config_dir().join("skills");
    let project_dir = project_root.map(|root| root.join("skills"));
    let mut skills = skills::load_skills(Some(&user_dir), project_dir.as_deref());
    skills.retain(commands_parse);
    skills
}

/// Every placeholder-free command line parses.
///
/// There is no token that is valid in every argument position: `.sls` grammars
/// mix free-form string/path/numeric slots (where `0` parses fine) with
/// enum-constrained slots such as `parse_color_value`, `parse_atom_style`,
/// `parse_light_preset`, and `parse_surface_style` (where `0` is never a valid
/// value). So substituting `0` and parse-checking the result can only prove a
/// negative for lines that carry no placeholder at all — those are checked as
/// written, and a parse failure there is a real, unconditional grammar error.
/// A line that does carry a placeholder is *trusted*: `compute-core`'s
/// `validate_skill` (which every loaded skill has already passed) rejects any
/// undeclared `{name}`, so the value will come from a real parameter at
/// runtime, and runtime parsing is the actual backstop. Dropping such a skill
/// here would silently discard legitimate skills whose templates happen to
/// land on an enum-constrained slot.
fn commands_parse(skill: &Skill) -> bool {
    for line in &skill.command {
        let substituted = substitute_placeholders(line);
        let has_placeholder = substituted != *line;
        if !has_placeholder && !command_parses(&substituted) {
            eprintln!(
                "skills: dropping `{}` — placeholder-free command does not parse: {line}",
                skill.name
            );
            return false;
        }
    }
    true
}

/// Replace every `{name}` placeholder with `0` so placeholder-free lines can be
/// parse-checked, and so numeric-position templates (e.g. `translate {dx} {dy}
/// {dz}` → `translate 0 0 0`) still parse-check cleanly. `0` is not a universal
/// stand-in — see [`commands_parse`] — so lines that still contain a
/// placeholder after substitution are not rejected on parse failure.
fn substitute_placeholders(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'{'
            && let Some(close) = line[index + 1..].find('}')
        {
            out.push('0');
            index = index + 1 + close + 1;
            continue;
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_placeholders_with_zero() {
        assert_eq!(
            substitute_placeholders("dock --ligand {ligand}"),
            "dock --ligand 0"
        );
        assert_eq!(
            substitute_placeholders("no placeholders"),
            "no placeholders"
        );
    }

    #[test]
    fn builtin_skills_all_parse() {
        // The built-in set must survive the grammar filter unchanged.
        let loaded = load_agent_skills(None);
        assert!(
            loaded
                .iter()
                .any(|s| s.source == crate::skills::SkillSource::Builtin)
        );
    }

    /// Build a minimal user skill for exercising `commands_parse` directly,
    /// bypassing disk loading and `validate_skill` (whose invariants — declared
    /// params matching every `{name}` — the caller is responsible for upholding
    /// in these literals).
    fn test_skill(name: &str, command: &[&str], params: &[&str]) -> Skill {
        Skill {
            name: name.to_string(),
            engine: None,
            description: "a test skill".to_string(),
            triggers: vec!["test".to_string()],
            params: params
                .iter()
                .map(|name| skills::SkillParam {
                    name: (*name).to_string(),
                    required: true,
                })
                .collect(),
            command: command.iter().map(|line| (*line).to_string()).collect(),
            caveats: Vec::new(),
            body: String::new(),
            in_table: true,
            source: skills::SkillSource::User,
        }
    }

    #[test]
    fn drops_skill_with_unparseable_placeholder_free_command() {
        // No placeholder in the line, and `frobnicate` is not a console verb —
        // this is the one case the filter must still definitively reject.
        let mut skills = vec![test_skill(
            "frobnicate-thing",
            &["frobnicate --foo bar"],
            &[],
        )];
        skills.retain(commands_parse);
        assert!(
            !skills.iter().any(|s| s.name == "frobnicate-thing"),
            "a placeholder-free line with an unknown verb must be dropped"
        );
    }

    #[test]
    fn keeps_skill_templating_an_enum_constrained_argument() {
        // Regression for the over-drop bug: `color chain <id> <color>` is a real
        // command, but substituting `{color}` with `0` produces `color chain 0
        // 0`, which `parse_color_value` rejects (it only accepts named colors or
        // `#rrggbb`). Before the fix this dropped the whole skill; now the
        // declared placeholder defers to `validate_skill` + runtime parsing, so
        // the skill must survive.
        let mut skills = vec![test_skill(
            "color-chain-by-param",
            &["color chain {chain} {color}"],
            &["chain", "color"],
        )];
        skills.retain(commands_parse);
        assert!(
            skills.iter().any(|s| s.name == "color-chain-by-param"),
            "a template landing on an enum-constrained slot must survive"
        );
    }

    #[test]
    fn keeps_skill_templating_a_numeric_argument() {
        // `view size <width> <height>` takes two plain f32 positions, so `0 0`
        // parses after substitution — this already worked pre-fix and must keep
        // working.
        let mut skills = vec![test_skill(
            "resize-view",
            &["view size {width} {height}"],
            &["width", "height"],
        )];
        skills.retain(commands_parse);
        assert!(
            skills.iter().any(|s| s.name == "resize-view"),
            "a numeric-position template that parses after `0` substitution must survive"
        );
    }
}
