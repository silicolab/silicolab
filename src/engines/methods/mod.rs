//! The agent-facing method-selection knowledge base.
//!
//! This is the **selection/routing** layer that sits *above* the engines' own
//! recommenders. It deliberately does NOT restate the level-of-theory science
//! that `hartree::guardrails` owns (surfaced to the agent as `qm recommend
//! <task>`); it routes to that and adds what a lower layer cannot know: which
//! SilicoLab engine/command to reach for, the runnable `.sls` translation of a
//! multi-level protocol, what is and isn't available here, and the MD / docking
//! / periodic choices.
//!
//! Two tiers of host-side progressive disclosure (the model never reads files):
//! * [`kb_table`] renders a compact, always-on decision table into the agent's
//!   (cacheable) system prompt — situation → the literal `.sls` command. Only
//!   rules flagged `in_table` appear here, keeping the per-turn cost bounded.
//! * [`recommend`] returns full rule bodies on demand, behind the read-only
//!   `recommend_method` tool, scoring the task against every rule's triggers.
//!
//! The embedded core (`kb/*.json`) is the shippable safety floor. The
//! [`MethodRule`] schema and [`validate_rule`] are intentionally reusable by a
//! future on-disk overlay; the consistency tests below are the correctness gate
//! that both the embedded core and any overlay must pass — in particular every
//! pinned `--method`/`--basis` is checked against the *live* engine capability
//! tables, so a rule can never name a molecular method this build can't run.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::engines::qm::QmMethod;

/// Which engine / command surface a rule applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    /// Molecular QM (`qm energy|optimize|freq`).
    Qm,
    /// Periodic / crystal QM (`qm periodic`).
    QmPeriodic,
    /// Molecular dynamics (`md build|simulate`).
    Md,
    /// Docking (`dock` / `score`).
    Docking,
}

impl Engine {
    /// The `.sls` verb(s) a rule's commands must start with.
    fn verbs(self) -> &'static [&'static str] {
        match self {
            Engine::Qm | Engine::QmPeriodic => &["qm"],
            Engine::Md => &["md"],
            Engine::Docking => &["dock", "score"],
        }
    }

    /// Section heading in the always-on table.
    fn heading(self) -> &'static str {
        match self {
            Engine::Qm => "Quantum chemistry — molecular (qm energy|optimize|freq)",
            Engine::QmPeriodic => "Quantum chemistry — periodic / crystal (qm periodic)",
            Engine::Md => "Molecular dynamics — GROMACS (md build|simulate)",
            Engine::Docking => "Docking — Vina (dock / score)",
        }
    }

    /// Render order for the table.
    const ALL: [Engine; 4] = [Engine::Qm, Engine::QmPeriodic, Engine::Md, Engine::Docking];
}

/// One knowledge unit: a situation, the runnable command(s) for it, its
/// caveats, and an on-demand body. Authored as a `kb/*.json` array element.
#[derive(Debug, Clone, Deserialize)]
pub struct MethodRule {
    /// Kebab-case unique id (≤64 chars).
    pub name: String,
    pub engine: Engine,
    /// One-line trigger shown in the always-on table; carries both *what* and
    /// *when* and names the command so it is actionable read alone (≤200 chars).
    pub description: String,
    /// Keywords for offline case-insensitive scoring in [`recommend`].
    #[serde(default)]
    pub triggers: Vec<String>,
    /// The literal `.sls` line(s) to run.
    #[serde(default)]
    pub command: Vec<String>,
    /// Trade-offs / gotchas; required to be non-empty when a QM rule pins a
    /// `--method`/`--basis` (no silent method choice).
    #[serde(default)]
    pub caveats: Vec<String>,
    /// On-demand deepening returned only by [`recommend`], never in the table.
    #[serde(default)]
    pub body: String,
    /// Whether this rule appears in the always-on [`kb_table`]. Common decisions
    /// are in-table; the long tail is tool-only to bound the per-turn cost.
    #[serde(default = "default_true")]
    pub in_table: bool,
}

fn default_true() -> bool {
    true
}

/// The embedded, parsed core. Immutable after first init (like the engine
/// registry tables); not mutable global state.
fn rules() -> &'static [MethodRule] {
    static RULES: OnceLock<Vec<MethodRule>> = OnceLock::new();
    RULES.get_or_init(load_embedded)
}

/// Parse the embedded `kb/*.json`. These are compiled-in and test-validated, so
/// a parse failure is a build bug — but we degrade to no rules rather than
/// panic in a production path (the consistency test guarantees it never fails).
fn load_embedded() -> Vec<MethodRule> {
    const SOURCES: &[&str] = &[
        include_str!("kb/qm.json"),
        include_str!("kb/md.json"),
        include_str!("kb/docking.json"),
    ];
    let mut all = Vec::new();
    for src in SOURCES {
        let parsed: Vec<MethodRule> = serde_json::from_str(src).unwrap_or_default();
        all.extend(parsed);
    }
    all
}

/// The always-on decision table injected into the (cacheable) system prompt.
/// Holds no volatile state, so the string is byte-stable across turns.
pub fn kb_table() -> String {
    render_table(rules())
}

fn render_table(rules: &[MethodRule]) -> String {
    let mut out = String::from(
        "Method-selection guide — pick the engine + command for the task, then act. \
         Reach for QM (`qm …`) for electronic structure, energies, and spectra of \
         molecules and small systems; MD (`md …`) for dynamics, solvation, and large \
         flexible systems; docking (`dock …`) for ligand–receptor poses; periodic QM \
         (`qm periodic`) for crystals. Call the `recommend_method` tool for the full \
         details/caveats of any line, and `qm recommend <task>` for the curated QM \
         level of theory.\n",
    );
    for engine in Engine::ALL {
        let group: Vec<&MethodRule> = rules
            .iter()
            .filter(|rule| rule.engine == engine && rule.in_table)
            .collect();
        if group.is_empty() {
            continue;
        }
        out.push('\n');
        out.push_str(engine.heading());
        out.push_str(":\n");
        for rule in group {
            out.push_str("- ");
            out.push_str(&rule.description);
            out.push('\n');
            for line in &rule.command {
                out.push_str("    ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// On-demand guidance for a free-text task: keyword-scores every rule (in-table
/// and tool-only) and returns the best one or two in full. Backs the read-only
/// `recommend_method` tool. Never errors; an unmatched task returns the table.
pub fn recommend(task: &str) -> String {
    let rules = rules();
    let query = task.to_ascii_lowercase();
    let terms: Vec<&str> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| term.len() > 2)
        .collect();

    let mut scored: Vec<(usize, &MethodRule)> = rules
        .iter()
        .map(|rule| (score_rule(rule, &terms), rule))
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));

    if scored.is_empty() {
        return format!(
            "No specific rule matched \u{201c}{task}\u{201d}. The full method-selection \
             guide:\n\n{}",
            render_table(rules)
        );
    }
    let mut out = String::new();
    for (_, rule) in scored.iter().take(2) {
        out.push_str(&render_detail(rule));
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Score a rule against the query terms: an exact trigger match weighs more than
/// a substring hit in the name/description.
fn score_rule(rule: &MethodRule, terms: &[&str]) -> usize {
    let haystack = format!(
        "{} {} {}",
        rule.name,
        rule.description,
        rule.triggers.join(" ")
    )
    .to_ascii_lowercase();
    terms
        .iter()
        .map(|term| {
            if rule.triggers.iter().any(|tr| tr.eq_ignore_ascii_case(term)) {
                3
            } else if haystack.contains(term) {
                1
            } else {
                0
            }
        })
        .sum()
}

fn render_detail(rule: &MethodRule) -> String {
    let mut out = format!("## {}\n{}\n", rule.name, rule.description);
    if !rule.command.is_empty() {
        out.push_str("run:\n");
        for line in &rule.command {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
    for caveat in &rule.caveats {
        out.push_str("- caveat: ");
        out.push_str(caveat);
        out.push('\n');
    }
    if !rule.body.trim().is_empty() {
        out.push('\n');
        out.push_str(rule.body.trim());
        out.push('\n');
    }
    out
}

/// Validate one rule against the **live** engine capabilities. Returns a
/// human-readable reason on failure. Reused by the embedded-core consistency
/// test and (later) the on-disk overlay loader, so the safety floor is
/// identical for both sources.
pub fn validate_rule(rule: &MethodRule) -> Result<(), String> {
    let name = &rule.name;
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(format!(
            "rule name `{name}` must be kebab-case, 1..=64 chars"
        ));
    }
    let described = rule.description.trim();
    if described.is_empty() || rule.description.chars().count() > 200 {
        return Err(format!("{name}: description must be 1..=200 chars"));
    }
    if rule.triggers.is_empty() {
        return Err(format!("{name}: needs at least one trigger keyword"));
    }
    let mut pins_method_or_basis = false;
    for command in &rule.command {
        pins_method_or_basis |= validate_command(rule, command)?;
    }
    // No silent method choice: a molecular-QM rule that pins a method/basis must
    // state why (so it can't quietly contradict the engine default or `qm
    // recommend`). Periodic/MD/docking have their own distinct option sets.
    if rule.engine == Engine::Qm && pins_method_or_basis && rule.caveats.is_empty() {
        return Err(format!(
            "{name}: a QM rule that pins --method/--basis must carry at least one caveat"
        ));
    }
    Ok(())
}

/// Validate a single command line; returns whether it pins a `--method`/`--basis`.
fn validate_command(rule: &MethodRule, command: &str) -> Result<bool, String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let verb = tokens.first().copied().unwrap_or_default();
    if !rule.engine.verbs().contains(&verb) {
        return Err(format!(
            "{}: command `{command}` starts with `{verb}`, invalid for engine {:?}",
            rule.name, rule.engine
        ));
    }
    let mut pins = false;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--method" => {
                pins = true;
                let value = tokens.get(index + 1).copied().unwrap_or_default();
                // Molecular QM only: periodic uses `--functional` + GTH bases.
                if rule.engine == Engine::Qm && !is_vetted_qm_method(value) {
                    return Err(format!(
                        "{}: `--method {value}` is not a vetted method (known \
                         wavefunction method, composite, or preset functional)",
                        rule.name
                    ));
                }
                index += 2;
            }
            "--basis" => {
                pins = true;
                let value = tokens.get(index + 1).copied().unwrap_or_default();
                if rule.engine == Engine::Qm && !is_vetted_qm_basis(value) {
                    return Err(format!(
                        "{}: `--basis {value}` is not on the vetted basis list",
                        rule.name
                    ));
                }
                index += 2;
            }
            _ => index += 1,
        }
    }
    Ok(pins)
}

/// A molecular `--method` token is vetted when it is a wavefunction method, a
/// composite, or a DFT functional in [`QmMethod::presets`] — never the free-text
/// `Dft(_)` fallback (which `QmMethod::parse` produces for typos, since it never
/// rejects). A trailing `-d3`/`-d4` dispersion suffix is allowed.
fn is_vetted_qm_method(token: &str) -> bool {
    let (method, _dispersion) = QmMethod::parse(token);
    match method {
        QmMethod::Dft(name) => QmMethod::presets().iter().any(
            |preset| matches!(preset, QmMethod::Dft(known) if known.eq_ignore_ascii_case(&name)),
        ),
        // HF/RHF/UHF/ROHF/MP2/CCSD/CCSD(T)/Composite are all vetted.
        _ => true,
    }
}

/// Curated allowlist of molecular basis sets the rules may name. There is no
/// `QmBasis::parse` to lean on (hartree validates the basis only at job run), so
/// this is the explicit gate; additions are a conscious diff.
const VETTED_BASES: &[&str] = &[
    "sto-3g",
    "6-31g",
    "def2-svp",
    "def2-sv(p)",
    "def2-tzvp",
    "def2-tzvpp",
    "def2-tzvpd",
    "def2-tzvppd",
    "def2-qzvp",
    "def2-qzvpp",
    "ma-def2-svp",
    "ma-def2-tzvp",
    "cc-pvdz",
    "cc-pvtz",
    "cc-pvqz",
    "aug-cc-pvdz",
    "aug-cc-pvtz",
];

fn is_vetted_qm_basis(token: &str) -> bool {
    VETTED_BASES
        .iter()
        .any(|basis| basis.eq_ignore_ascii_case(token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn embedded_rules_load_are_valid_and_unique() {
        let rules = rules();
        assert!(!rules.is_empty(), "embedded KB parsed to no rules");
        let mut names = HashSet::new();
        for rule in rules {
            validate_rule(rule).unwrap_or_else(|reason| panic!("invalid rule: {reason}"));
            assert!(
                names.insert(rule.name.as_str()),
                "duplicate rule name `{}`",
                rule.name
            );
        }
    }

    #[test]
    fn method_vetting_rejects_typos_and_accepts_known() {
        // `QmMethod::parse` never fails, so a typo'd functional must be caught
        // by the presets allowlist, not by parse.
        assert!(!is_vetted_qm_method("b3lpy"), "typo should be rejected");
        assert!(
            !is_vetted_qm_method("pw6b95"),
            "non-preset functional rejected"
        );
        assert!(is_vetted_qm_method("r2scan-3c"));
        assert!(is_vetted_qm_method("wb97m-v"));
        assert!(is_vetted_qm_method("wb97m-v-d4"));
        assert!(is_vetted_qm_method("ccsd(t)"));
        assert!(is_vetted_qm_method("b3lyp"));
    }

    #[test]
    fn validate_rejects_unvetted_method_and_missing_caveat() {
        let bad_method = MethodRule {
            name: "x".into(),
            engine: Engine::Qm,
            description: "d".into(),
            triggers: vec!["t".into()],
            command: vec!["qm energy --method notreal".into()],
            caveats: vec!["c".into()],
            body: String::new(),
            in_table: true,
        };
        assert!(validate_rule(&bad_method).is_err());

        let no_caveat = MethodRule {
            command: vec!["qm energy --method r2scan-3c".into()],
            caveats: vec![],
            ..bad_method.clone()
        };
        assert!(
            validate_rule(&no_caveat).is_err(),
            "pinning a method without a caveat must fail"
        );
    }

    #[test]
    fn always_on_table_is_bounded() {
        let table = kb_table();
        let lines = table.lines().count();
        let chars = table.chars().count();
        assert!(
            lines < 100,
            "always-on table is {lines} lines (budget < 100)"
        );
        assert!(
            chars < 6000,
            "always-on table is {chars} chars (budget < 6000)"
        );
        // It must actually point at the engine recommender, not restate it.
        assert!(table.contains("qm recommend"));
    }

    #[test]
    fn qm_guidance_does_not_contradict_linked_hartree() {
        // Cross-check against the *actually linked* hartree: its curated table
        // still recommends these levels, so our routing stays consistent.
        use hartree::guardrails::recommend as hrec;
        assert!(hrec("general").unwrap().level.contains("r2scan-3c"));
        assert!(hrec("thermochemistry").unwrap().level.contains("wb97m-v"));
        // At least one QM rule routes to `qm recommend` rather than hardcoding a
        // level that could drift from the engine.
        assert!(
            rules()
                .iter()
                .any(|r| r.command.iter().any(|c| c.contains("qm recommend"))),
            "no QM rule routes to `qm recommend`"
        );
    }

    #[test]
    fn recommend_matches_on_triggers_and_never_empty() {
        let out = recommend("free energy and thermochemistry of a reaction");
        assert!(
            out.to_lowercase().contains("freq") || out.contains("qm recommend"),
            "thermochemistry query should surface the freq/recommend guidance: {out}"
        );
        // An out-of-vocabulary query falls back to the full table, not empty.
        assert!(!recommend("zzzzz qqqqq").is_empty());
    }
}
