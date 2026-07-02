use super::*;
use crate::io::llm::types::ToolCall;

fn call(command: &str) -> ToolCall {
    ToolCall {
        id: "t".to_string(),
        name: "run_command".to_string(),
        input: json!({ "command": command }),
    }
}

fn save_script_call() -> ToolCall {
    ToolCall {
        id: "s".to_string(),
        name: "save_script".to_string(),
        input: json!({ "filename": "wf", "commands": ["open x.pdb"] }),
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
    assert_eq!(risk_of_call(&save_script_call()), RiskLevel::FileWrite);
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
        &save_script_call(),
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

#[test]
fn save_script_writes_and_rejects_paths() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ok = save_script_tool(
        &mut state,
        &json!({ "filename": "demo", "commands": ["fetch 4hhb", "color hetero"] }),
    );
    assert!(!ok.is_error, "expected success: {}", ok.content);
    assert!(ok.content.contains("demo.sls"));

    let bad = save_script_tool(
        &mut state,
        &json!({ "filename": "../evil", "commands": [] }),
    );
    assert!(bad.is_error);
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
