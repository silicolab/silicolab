use super::*;
use crate::io::llm::types::ToolCall;

fn call(command: &str) -> ToolCall {
    ToolCall {
        id: "t".to_string(),
        name: "run_command".to_string(),
        input: json!({ "command": command }),
    }
}

fn save_skill_call() -> ToolCall {
    ToolCall {
        id: "s".to_string(),
        name: "save_skill".to_string(),
        input: json!({
            "name": "demo-skill",
            "description": "A demo skill for tests.",
            "triggers": ["demo"],
            "commands": ["representation cartoon"]
        }),
    }
}

fn gated(command: &str) -> bool {
    needs_confirmation(
        &call(command),
        ApprovalMode::AutoSafe,
        &HashSet::new(),
        &HashSet::new(),
    )
}

#[test]
fn risk_classification_matches_the_grammar() {
    assert_eq!(
        risk_of_call(&call("view background white")),
        RiskLevel::ReadOnly
    );
    assert_eq!(
        risk_of_call(&call("color chain A red")),
        RiskLevel::ReadOnly
    );
    assert_eq!(risk_of_call(&call("open 1abc.pdb")), RiskLevel::ReadOnly);
    assert_eq!(risk_of_call(&call("hydrogen add")), RiskLevel::ReadOnly);
    assert_eq!(
        risk_of_call(&call("qm recommend thermochemistry")),
        RiskLevel::ReadOnly
    );
    assert_eq!(
        risk_of_call(&call("phosphorylate --protein active --at A:1")),
        RiskLevel::Mutating
    );
    assert_eq!(
        risk_of_call(&call("save image out.png")),
        RiskLevel::FileWrite
    );
    assert_eq!(risk_of_call(&save_skill_call()), RiskLevel::FileWrite);
    assert_eq!(risk_of_call(&call("md build")), RiskLevel::Expensive);
    assert_eq!(
        risk_of_call(&call("qm energy --method r2scan-3c")),
        RiskLevel::Expensive
    );
    assert_eq!(
        risk_of_call(&call("dock --receptor active --ligand 2")),
        RiskLevel::Expensive
    );
    assert_eq!(
        risk_of_call(&call("score --receptor active")),
        RiskLevel::Expensive
    );
    assert_eq!(risk_of_call(&call("run setup.sls")), RiskLevel::Destructive);
    assert_eq!(risk_of_call(&call("setup.sls")), RiskLevel::Destructive);
    assert_eq!(
        risk_of_call(&call("delete chain A")),
        RiskLevel::Destructive
    );
}

#[test]
fn structure_editing_commands_are_never_read_only() {
    for command in [
        "phosphorylate --protein active --at A:84",
        "acetylate --protein active --at A:120",
        "methylate --protein active --at A:9",
        "lipidate --protein active --at A:3",
        "ubiquitinate --protein active --at A:48",
        "glycosylate --protein active",
        "glycan GlcNAc",
    ] {
        assert_eq!(
            risk_of_call(&call(command)),
            RiskLevel::Mutating,
            "{command}"
        );
        assert!(
            gated_in(command, ApprovalMode::Manual),
            "{command} must gate in Manual"
        );
    }
    assert_eq!(
        risk_of_call(&call("save image out.png")),
        RiskLevel::FileWrite
    );
    assert!(gated_in("save image out.png", ApprovalMode::Manual));
    assert_eq!(
        risk_of_call(&call("delete chain A")),
        RiskLevel::Destructive
    );
}

fn gated_in(command: &str, mode: ApprovalMode) -> bool {
    needs_confirmation(&call(command), mode, &HashSet::new(), &HashSet::new())
}

#[test]
fn autosafe_runs_edits_but_gates_writes_compute_and_destructive() {
    assert!(!gated("view background white"));
    assert!(!gated("phosphorylate --protein active --at A:1"));
    assert!(gated("save image out.png"));
    assert!(gated("qm energy"));
    assert!(gated("delete chain A"));
    assert!(needs_confirmation(
        &save_skill_call(),
        ApprovalMode::AutoSafe,
        &HashSet::new(),
        &HashSet::new(),
    ));
}

#[test]
fn manual_gates_everything_but_read_only() {
    assert!(!gated_in("view background white", ApprovalMode::Manual));
    assert!(gated_in("save image out.png", ApprovalMode::Manual));
    assert!(gated_in("qm energy", ApprovalMode::Manual));
    assert!(gated_in("delete chain A", ApprovalMode::Manual));
}

#[test]
fn auto_runs_everything_except_destructive() {
    assert!(!gated_in("qm energy", ApprovalMode::Auto));
    assert!(!gated_in("save image out.png", ApprovalMode::Auto));
    assert!(gated_in("delete chain A", ApprovalMode::Auto));
}

#[test]
fn destructive_always_confirms_and_allow_set_cannot_bypass_it() {
    let mut verbs = HashSet::new();
    verbs.insert("delete".to_string());
    let mut risks = HashSet::new();
    risks.insert(RiskLevel::Destructive);
    for mode in ApprovalMode::all() {
        if mode == ApprovalMode::Plan {
            continue;
        }
        for command in ["delete chain A", "run setup.sls", "setup.sls"] {
            assert!(
                needs_confirmation(&call(command), mode, &verbs, &risks),
                "{command} must confirm in {mode:?} even with an allow-set"
            );
        }
    }
}

#[test]
fn allow_set_suppresses_gating_for_verb_and_risk() {
    let mut verbs = HashSet::new();
    verbs.insert("qm".to_string());
    assert!(!needs_confirmation(
        &call("qm energy"),
        ApprovalMode::AutoSafe,
        &verbs,
        &HashSet::new()
    ));

    let mut risks = HashSet::new();
    risks.insert(RiskLevel::Expensive);
    assert!(!needs_confirmation(
        &call("dock --receptor active"),
        ApprovalMode::AutoSafe,
        &HashSet::new(),
        &risks,
    ));
}

#[test]
fn unparseable_command_is_read_only_so_it_runs_and_self_reports_the_error() {
    assert_eq!(risk_of_call(&call("inspect")), RiskLevel::ReadOnly);
    assert!(!gated("frobnicate the molecule"));
}

#[test]
fn perception_tools_are_never_gated() {
    let inspect_call = ToolCall {
        id: "t".to_string(),
        name: "inspect".to_string(),
        input: json!({}),
    };
    let recommend = ToolCall {
        id: "t".to_string(),
        name: "recommend_method".to_string(),
        input: json!({ "task": "thermochemistry" }),
    };
    for c in [&inspect_call, &recommend] {
        assert_eq!(risk_of_call(c), RiskLevel::ReadOnly);
        assert!(!needs_confirmation(
            c,
            ApprovalMode::Manual,
            &HashSet::new(),
            &HashSet::new()
        ));
    }
}

/// Point `state.workspace` at a temp-dir project so `save_skill_tool` writes
/// under a disposable `skills/` directory instead of the real `~/.silicolab`.
fn state_with_temp_project(tag: &str) -> (AppState, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "sl-save-skill-test-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut state = AppState::scratch(Default::default(), Vec::new());
    state.workspace = crate::backend::project::WorkspaceSession::Project(
        crate::backend::project::ProjectSession::from_root(root.clone(), "test".to_string()),
    );
    (state, root)
}

/// A workspace change (project open/close/switch) routes through
/// `reset_transient_state`, which must invalidate the agent's skills cache so
/// the next turn reloads project-scoped skills for the new root. Without this,
/// opening a project mid-session leaves its `skills/` undiscovered.
#[test]
fn reset_transient_state_invalidates_skills_cache() {
    let (mut state, root) = state_with_temp_project("skills-invalidate");
    state.ui.agent.skills_loaded = true;
    crate::frontend::dispatcher::reset_transient_state(&mut state);
    assert!(
        !state.ui.agent.skills_loaded,
        "workspace change must invalidate the skills cache"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn save_skill_writes_valid_and_rejects_invalid() {
    let (mut state, root) = state_with_temp_project("valid-invalid");

    // Valid skill writes and reports success.
    let ok = save_skill_tool(
        &mut state,
        &json!({
            "name": "demo-skill",
            "description": "Set a cartoon representation.",
            "triggers": ["demo", "cartoon"],
            "commands": ["representation cartoon"]
        }),
    );
    assert!(!ok.is_error, "valid skill should save: {}", ok.content);
    let written = root.join("skills").join("demo-skill").join("SKILL.md");
    assert!(written.is_file(), "expected {} to exist", written.display());

    // The written file round-trips through the parser/validator.
    let text = std::fs::read_to_string(&written).unwrap();
    let parsed = crate::skills::parse_skill_md(&text, crate::skills::SkillSource::User)
        .expect("written SKILL.md parses");
    crate::skills::validate_skill(&parsed).expect("written skill validates");
    assert_eq!(parsed.name, "demo-skill");
    // Saved skills default to lookup-only so they don't inflate the always-on
    // manifest; they surface through recommend_method / triggers instead.
    assert!(
        !parsed.in_table,
        "saved skills must default to in_table: false"
    );

    // Invalid: bad name (not kebab-case) is rejected, nothing panics, and no
    // file is written for it.
    let bad = save_skill_tool(
        &mut state,
        &json!({
            "name": "Bad Name",
            "description": "x",
            "triggers": ["demo"],
            "commands": ["representation cartoon"]
        }),
    );
    assert!(bad.is_error, "non-kebab name must be rejected");
    assert!(!root.join("skills").join("Bad Name").exists());

    std::fs::remove_dir_all(&root).ok();
}

/// Regression for the path-traversal finding: `render_skill_md` used to emit
/// the `name:` line unquoted, so a value like `good-name #/../../evil` was
/// truncated by YAML's comment rule to `good-name` when read back — passing
/// `validate_skill` — while the write path was built from the RAW,
/// unvalidated input, so `dir = base.join(name)` resolved the `../../` and
/// escaped the skills tree. `name:` is now quoted via `yaml_scalar` (so the
/// full raw string round-trips and a non-kebab name is correctly rejected),
/// and the write path is built from the validated `skill.name`, never the
/// raw input. Every listed name must be rejected with nothing written
/// anywhere, in or out of the temp skills tree.
#[test]
fn save_skill_rejects_path_traversal_names_and_writes_nothing() {
    let (mut state, root) = state_with_temp_project("traversal");

    for traversal_name in [
        "good-name #/../../evil",
        "../../evil",
        "Bad Name",
        "../evil",
    ] {
        let result = save_skill_tool(
            &mut state,
            &json!({
                "name": traversal_name,
                "description": "Attempted path traversal via yaml comment truncation.",
                "triggers": ["evil"],
                "commands": ["representation cartoon"]
            }),
        );
        assert!(
            result.is_error,
            "traversal-y name `{traversal_name}` must be rejected: {}",
            result.content
        );

        // Nothing escaped the temp project root.
        let outside = root.parent().unwrap().join("evil");
        assert!(
            !outside.exists(),
            "`{traversal_name}` must not write outside the skills tree"
        );
    }

    // Nothing was written under the skills dir at all — every attempt failed
    // validation before any filesystem side effect.
    let skills_dir = root.join("skills");
    if skills_dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(&skills_dir).unwrap().collect();
        assert!(
            entries.is_empty(),
            "skills dir must have no stray entries from rejected names"
        );
    }

    std::fs::remove_dir_all(&root).ok();
}

/// Regression for the `params`/`{placeholder}` finding: the tool description
/// tells the model to declare placeholders in `params`, but the input schema
/// had no such property, so `params` was always empty and `validate_skill`
/// rejected any command referencing a `{placeholder}`. `save_skill` now reads
/// `params` from the tool input and `render_skill_md` emits a `params:`
/// block; this must round-trip through `parse_skill_md`/`validate_skill`.
#[test]
fn save_skill_with_declared_params_round_trips_placeholder_command() {
    let (mut state, root) = state_with_temp_project("params");

    let ok = save_skill_tool(
        &mut state,
        &json!({
            "name": "dock-with-params",
            "description": "Dock a ligand into a receptor using run-time placeholders.",
            "triggers": ["dock", "ligand"],
            "commands": ["dock --receptor {receptor} --ligand {ligand}"],
            "params": [
                { "name": "receptor", "required": true },
                { "name": "ligand", "required": true }
            ]
        }),
    );
    assert!(
        !ok.is_error,
        "a skill with declared params for its placeholders should validate and save: {}",
        ok.content
    );

    let written = root
        .join("skills")
        .join("dock-with-params")
        .join("SKILL.md");
    assert!(written.is_file(), "expected {} to exist", written.display());

    let text = std::fs::read_to_string(&written).unwrap();
    let parsed = crate::skills::parse_skill_md(&text, crate::skills::SkillSource::User)
        .expect("written SKILL.md parses");
    crate::skills::validate_skill(&parsed).expect("written skill validates");

    assert_eq!(parsed.params.len(), 2);
    assert!(
        parsed
            .params
            .iter()
            .any(|p| p.name == "receptor" && p.required)
    );
    assert!(
        parsed
            .params
            .iter()
            .any(|p| p.name == "ligand" && p.required)
    );
    assert_eq!(
        parsed.command,
        vec!["dock --receptor {receptor} --ligand {ligand}"]
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn saved_skill_is_discoverable_via_recommend_method() {
    let (mut state, root) = state_with_temp_project("recommend");

    let ok = save_skill_tool(
        &mut state,
        &json!({
            "name": "quantum-zzyzx-workflow",
            "description": "A very specific made-up workflow for the test.",
            "triggers": ["zzyzx"],
            "commands": ["representation cartoon"]
        }),
    );
    assert!(!ok.is_error, "expected success: {}", ok.content);

    // save_skill_tool must have reloaded the session's skills.
    assert!(
        state
            .ui
            .agent
            .skills
            .iter()
            .any(|s| s.name == "quantum-zzyzx-workflow"),
        "reload_skills should pick up the newly saved skill"
    );

    let out = execute_tool(
        &mut state,
        &ToolCall {
            id: "r".into(),
            name: "recommend_method".into(),
            input: json!({ "task": "zzyzx" }),
        },
    );
    assert!(!out.is_error);
    assert!(
        out.content.contains("quantum-zzyzx-workflow"),
        "recommend_method should surface the saved skill: {}",
        out.content
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn list_jobs_and_cancel_job_control_assistant_qm_job() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let controller = crate::backend::tasks::task_controller_by_id("qm-optimize")
        .copied()
        .unwrap();
    let task_run_id = state.tasks.create_task_run(controller);
    state
        .tasks
        .mark_status(task_run_id, crate::backend::tasks::TaskStatus::Running);
    let (_tx, rx) = std::sync::mpsc::channel();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    state
        .jobs
        .agent_jobs
        .push(crate::frontend::jobs::TrackedAgentJob {
            id: 7,
            conversation: state.ui.agent.active_conversation,
            label: "qm optimize".to_string(),
            task_run_id,
            job: crate::frontend::jobs::AgentHeavyJob::Qm(crate::frontend::jobs::RunningQmJob {
                cancel: crate::wire::JobCancelHandle::from_flag(std::sync::Arc::clone(&cancel)),
                receiver: rx,
                latest_stage: Some("SCF".to_string()),
                cancel_requested: false,
            }),
        });

    let list = execute_tool(
        &mut state,
        &ToolCall {
            id: "list".into(),
            name: "list_jobs".into(),
            input: json!({}),
        },
    );
    assert!(!list.is_error, "{}", list.content);
    assert!(
        list.content.contains("\"id\": \"agent:7\""),
        "{}",
        list.content
    );
    assert!(
        list.content.contains("\"label\": \"qm optimize\""),
        "{}",
        list.content
    );

    let cancel_out = execute_tool(
        &mut state,
        &ToolCall {
            id: "cancel".into(),
            name: "cancel_job".into(),
            input: json!({ "id": "agent:7" }),
        },
    );

    assert!(!cancel_out.is_error, "{}", cancel_out.content);
    assert!(
        cancel_out.content.contains("Cancel requested"),
        "{}",
        cancel_out.content
    );
    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(state.jobs.agent_jobs.len(), 1);
    assert_eq!(
        state.tasks.task_run(task_run_id).unwrap().status,
        crate::backend::tasks::TaskStatus::Cancelling
    );
}

#[test]
fn clamp_truncates_large_output() {
    let big = "x".repeat(MAX_RESULT_CHARS + 50);
    let clamped = clamp_result(&big);
    assert!(clamped.ends_with("(truncated)"));
    assert!(clamped.chars().count() < big.chars().count());
}
