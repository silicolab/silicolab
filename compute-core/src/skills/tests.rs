use super::*;

#[test]
fn parses_frontmatter_and_body() {
    let text = "---\n\
name: dock-a-ligand\n\
engine: docking\n\
description: Dock a ligand into the active receptor.\n\
triggers: [dock, ligand]\n\
params:\n\
  - {name: ligand, required: true}\n\
command:\n\
  - \"dock --receptor active --ligand {ligand}\"\n\
---\n\
# Dock a ligand\n\
\n\
Body text.\n";
    let skill = parse_skill_md(text, SkillSource::User).expect("parses");
    assert_eq!(skill.name, "dock-a-ligand");
    assert_eq!(skill.engine, Some(Engine::Docking));
    assert_eq!(skill.triggers, vec!["dock", "ligand"]);
    assert_eq!(skill.params.len(), 1);
    assert_eq!(skill.params[0].name, "ligand");
    assert!(skill.params[0].required);
    assert_eq!(
        skill.command,
        vec!["dock --receptor active --ligand {ligand}"]
    );
    assert!(skill.body.starts_with("# Dock a ligand"));
    assert!(skill.in_table, "in_table defaults to true");
    assert_eq!(skill.source, SkillSource::User);
}

#[test]
fn missing_fence_is_an_error() {
    assert!(parse_skill_md("no frontmatter here", SkillSource::User).is_err());
}

fn skill(engine: Option<Engine>, command: Vec<&str>, caveats: Vec<&str>) -> Skill {
    Skill {
        name: "s".into(),
        engine,
        description: "d".into(),
        triggers: vec!["t".into()],
        params: vec![],
        command: command.into_iter().map(str::to_string).collect(),
        caveats: caveats.into_iter().map(str::to_string).collect(),
        body: String::new(),
        in_table: true,
        source: SkillSource::User,
    }
}

#[test]
fn rejects_bad_name_and_description() {
    let mut s = skill(None, vec![], vec![]);
    s.name = "Not Kebab".into();
    assert!(validate_skill(&s).is_err());
    let mut s = skill(None, vec![], vec![]);
    s.description = "x".repeat(201);
    assert!(validate_skill(&s).is_err());
    let mut s = skill(None, vec![], vec![]);
    s.triggers = vec![];
    assert!(validate_skill(&s).is_err());
}

#[test]
fn rejects_unvetted_method_and_missing_caveat() {
    let bad = skill(
        Some(Engine::Qm),
        vec!["qm energy --method notreal"],
        vec!["c"],
    );
    assert!(validate_skill(&bad).is_err());
    let no_caveat = skill(
        Some(Engine::Qm),
        vec!["qm energy --method r2scan-3c"],
        vec![],
    );
    assert!(validate_skill(&no_caveat).is_err());
    let ok = skill(
        Some(Engine::Qm),
        vec!["qm energy --method r2scan-3c"],
        vec!["slow but accurate"],
    );
    assert!(validate_skill(&ok).is_ok());
}

#[test]
fn engine_agnostic_may_not_pin_method_but_may_run_commands() {
    let pins = skill(None, vec!["qm energy --method b3lyp"], vec!["c"]);
    assert!(
        validate_skill(&pins).is_err(),
        "no engine to vet a method against"
    );
    let plain = skill(None, vec!["open receptor.pdb"], vec![]);
    assert!(validate_skill(&plain).is_ok());
}

#[test]
fn rejects_undeclared_placeholder() {
    let mut s = skill(
        Some(Engine::Docking),
        vec!["dock --receptor active --ligand {ligand}"],
        vec![],
    );
    s.params = vec![]; // {ligand} not declared
    assert!(validate_skill(&s).is_err());
    s.params = vec![SkillParam {
        name: "ligand".into(),
        required: true,
    }];
    assert!(validate_skill(&s).is_ok());
}

#[test]
fn builtin_skills_are_nonempty_and_valid() {
    let skills = builtin_skills();
    assert!(!skills.is_empty(), "built-in KB produced no skills");
    for s in &skills {
        assert_eq!(s.source, SkillSource::Builtin);
        assert!(s.engine.is_some(), "every KB rule has an engine");
        validate_skill(s).unwrap_or_else(|e| panic!("invalid built-in skill: {e}"));
    }
}

#[test]
fn manifest_is_bounded_and_points_at_qm_recommend() {
    let manifest = skills_manifest(&builtin_skills());
    assert!(manifest.lines().count() < 100, "manifest exceeds 100 lines");
    assert!(
        manifest.chars().count() < 6000,
        "manifest exceeds 6000 chars"
    );
    assert!(manifest.contains("qm recommend"));
}

#[test]
fn manifest_budget_holds_for_a_large_disk_skill_set() {
    // The builtin-only bound test above cannot catch runaway growth from
    // disk skills, since the live prompt renders `state.ui.agent.skills`.
    // A big pile of `in_table` skills must still stay within budget: the
    // manifest truncates the long tail with a pointer to recommend_method.
    let mut skills: Vec<Skill> = Vec::new();
    for i in 0..200 {
        skills.push(Skill {
            name: format!("workflow-{i:03}"),
            engine: None,
            description: format!(
                "Reusable workflow number {i} for stress-testing the manifest budget."
            ),
            triggers: vec![format!("trigger{i}")],
            params: vec![],
            command: vec!["representation cartoon".into()],
            caveats: vec![],
            body: String::new(),
            in_table: true,
            source: SkillSource::User,
        });
    }
    let manifest = skills_manifest(&skills);
    assert!(
        manifest.lines().count() < 100,
        "manifest exceeds 100 lines: {}",
        manifest.lines().count()
    );
    assert!(
        manifest.chars().count() < 6000,
        "manifest exceeds 6000 chars: {}",
        manifest.chars().count()
    );
    assert!(
        manifest.contains("more — use recommend_method"),
        "overflow must point at recommend_method; got:\n{manifest}"
    );
}

#[test]
fn manifest_groups_workflows_and_annotates_source() {
    let mut skills = builtin_skills();
    skills.push(Skill {
        name: "prep-active-site".into(),
        engine: None,
        description: "Prep the active site before docking.".into(),
        triggers: vec!["prep".into()],
        params: vec![],
        command: vec!["representation cartoon".into()],
        caveats: vec![],
        body: String::new(),
        in_table: true,
        source: SkillSource::Project,
    });
    let manifest = skills_manifest(&skills);
    assert!(
        manifest.contains("Workflows"),
        "engine-agnostic skills get a Workflows group"
    );
    assert!(
        manifest.contains("[project]"),
        "non-builtin skills are annotated with their source"
    );
}

#[test]
fn recommend_matches_triggers_and_never_empty() {
    let skills = builtin_skills();
    let out = recommend(&skills, "free energy and thermochemistry of a reaction");
    assert!(
        out.to_lowercase().contains("freq") || out.contains("qm recommend"),
        "got: {out}"
    );
    assert!(!recommend(&skills, "zzzzz qqqqq").is_empty());
}

#[test]
fn placeholder_method_value_skips_vetting() {
    let mut s = skill(
        Some(Engine::Qm),
        vec!["qm energy --method {method}"],
        vec!["user picks the method"],
    );
    s.params = vec![SkillParam {
        name: "method".into(),
        required: true,
    }];
    assert!(
        validate_skill(&s).is_ok(),
        "a placeholder method can't be vetted at authoring time"
    );
}

#[test]
fn load_merges_sources_with_project_precedence() {
    let tmp = std::env::temp_dir().join(format!("sl-skills-test-{}", std::process::id()));
    let user_dir = tmp.join("user");
    let project_dir = tmp.join("project");
    let make = |root: &std::path::Path, name: &str, desc: &str| {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let md = format!(
            "---\nname: {name}\ndescription: {desc}\ntriggers: [demo]\ncommand:\n  - \"representation cartoon\"\n---\nBody.\n"
        );
        std::fs::write(dir.join("SKILL.md"), md).unwrap();
    };
    make(&user_dir, "demo-skill", "user version");
    make(&project_dir, "demo-skill", "project version");
    make(&user_dir, "user-only", "only in user");

    let skills = load_skills(Some(&user_dir), Some(&project_dir));

    let demo = skills
        .iter()
        .find(|s| s.name == "demo-skill")
        .expect("demo present");
    assert_eq!(
        demo.description, "project version",
        "project overrides user"
    );
    assert_eq!(demo.source, SkillSource::Project);
    assert!(
        skills.iter().any(|s| s.name == "user-only"),
        "user-only survives"
    );
    assert!(
        skills.iter().any(|s| s.source == SkillSource::Builtin),
        "builtin merged in"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn load_skips_invalid_files_without_panicking() {
    let tmp = std::env::temp_dir().join(format!("sl-skills-bad-{}", std::process::id()));
    let user_dir = tmp.join("user");
    let dir = user_dir.join("broken");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "not valid frontmatter").unwrap();
    // Should not panic; broken skill simply absent.
    let skills = load_skills(Some(&user_dir), None);
    assert!(skills.iter().all(|s| s.name != "broken"));
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn load_skips_files_that_fail_validation() {
    // Well-formed YAML/all required fields present, so `parse_skill_md`
    // succeeds; `Bad_Name` is not kebab-case, so `validate_skill` rejects it.
    // This exercises the DISTINCT `Err` arm from a parse failure.
    let text = "---\n\
name: Bad_Name\n\
description: A skill with a non-kebab-case name.\n\
triggers: [demo]\n\
---\n\
Body.\n";

    // Confirm the fixture genuinely reaches the validate arm before ever
    // touching disk: parsing must succeed, and validation must fail.
    let parsed = parse_skill_md(text, SkillSource::User).expect("frontmatter parses successfully");
    assert_eq!(parsed.name, "Bad_Name");
    let validation_err = validate_skill(&parsed).expect_err("non-kebab-case name fails validation");
    assert!(
        validation_err.contains("kebab-case"),
        "got: {validation_err}"
    );

    let tmp = std::env::temp_dir().join(format!(
        "sl-skills-invalid-validation-{}",
        std::process::id()
    ));
    let user_dir = tmp.join("user");
    let dir = user_dir.join("bad-name");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), text).unwrap();

    // Should not panic; the invalid skill is skipped, leaving only built-ins.
    let skills = load_skills(Some(&user_dir), None);
    assert!(skills.iter().all(|s| s.name != "Bad_Name"));
    assert_eq!(
        skills.len(),
        builtin_skills().len(),
        "only built-ins survive when the sole user skill fails validation"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn load_with_nonexistent_dir_is_noop() {
    let tmp = std::env::temp_dir().join(format!("sl-skills-does-not-exist-{}", std::process::id()));
    // Deliberately do NOT create `tmp`; `Some(&tmp)` must still short-circuit
    // cleanly through `scan_dir`'s `read_dir` `Err(_) => return out` branch,
    // not the `None`-guard short-circuit.
    assert!(!tmp.exists(), "fixture precondition: dir must not exist");

    let skills = load_skills(Some(&tmp), None);
    assert_eq!(
        skills.len(),
        builtin_skills().len(),
        "a nonexistent dir contributes nothing beyond the built-ins"
    );
}

#[test]
fn manifest_clamps_a_single_pathological_entry() {
    // The first entry is always emitted even when it alone exceeds the soft
    // budget (the manifest must never be empty), so the per-entry clamp in
    // `write_manifest_entry` is what keeps a hand-authored `in_table` skill
    // with a huge description or template within the hard budget.
    let mut s = skill(None, vec![], vec![]);
    s.description = "d".repeat(1000); // far past the validated 200-char cap
    s.command = (0..500)
        .map(|i| format!("representation cartoon --entry {i}"))
        .collect();
    let manifest = skills_manifest(&[s]);
    assert!(
        manifest.lines().count() < 100,
        "manifest exceeds 100 lines: {}",
        manifest.lines().count()
    );
    assert!(
        manifest.chars().count() < 6000,
        "manifest exceeds 6000 chars: {}",
        manifest.chars().count()
    );
    assert!(
        manifest.contains("recommend_method has the full template"),
        "clamped entry must point at recommend_method; got:\n{manifest}"
    );
}

#[test]
fn substitute_placeholders_replaces_groups_and_keeps_utf8_intact() {
    assert_eq!(
        substitute_placeholders("dock --ligand {ligand}", "0"),
        "dock --ligand 0"
    );
    assert_eq!(
        substitute_placeholders("translate {dx} {dy} {dz}", "0"),
        "translate 0 0 0"
    );
    // Regression: a byte-cast copy turned "café" into "cafÃ©", making a
    // placeholder-free line compare unequal to its substituted form.
    assert_eq!(
        substitute_placeholders("open café.pdb", "0"),
        "open café.pdb"
    );
    assert_eq!(
        substitute_placeholders("label {text} — café", "0"),
        "label 0 — café"
    );
}
