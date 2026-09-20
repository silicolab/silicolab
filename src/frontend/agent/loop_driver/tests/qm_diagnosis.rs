use super::*;
use crate::backend::run_attempt::{ArtifactStatus, Placement, QmResult};
use crate::backend::tasks::task_controller_by_id;
use crate::frontend::agent::session::AgentSession;
use crate::frontend::jobs::RunningAgentTurn;

fn completed_job(state: &mut AppState, converged: bool) -> (crate::job::JobId, std::path::PathBuf) {
    let task = state
        .tasks
        .create_task_run(*task_controller_by_id("qm-energy").unwrap());
    let dir = std::env::temp_dir().join(format!("silicolab-diagnosis-{}", uuid::Uuid::new_v4()));
    state.tasks.set_run_dir(task, dir.clone());
    let job_id =
        state
            .tasks
            .runs
            .begin_execution(task, Placement::Local, Some("qm-energy".into()), 0);
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(QmWorkerMessage::Finished(Box::new(
        crate::engines::qm::QmOutcome {
            energy_hartree: -1.0,
            converged,
            optimized_structure: None,
            summary: "raw evidence".into(),
            scf_trace: vec![],
            opt_trace: vec![],
            frequencies: vec![],
        },
    )))
    .unwrap();
    let mut tracked = fake_qm_job(1, state.ui.agent.active_conversation, rx);
    tracked.task_run_id = task;
    tracked.job_id = job_id;
    state.jobs.agent_jobs.push(tracked);
    (job_id, dir)
}

fn offline_state() -> AppState {
    let mut state = enabled_state();
    state.ui.agent.selection.provider = "test-no-provider".into();
    state
}

#[test]
fn qm_issue_stops_old_turn_approval_and_queue_even_when_diagnosis_disabled() {
    for phase in [AgentPhase::AwaitingModel, AgentPhase::AwaitingApproval] {
        let mut state = offline_state();
        state.config.assistant.auto_diagnose_qm_issues = false;
        let (job_id, dir) = completed_job(&mut state, false);
        let (_tx, receiver) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        state.jobs.agent = Some(RunningAgentTurn {
            cancel: cancel.clone(),
            receiver,
        });
        state
            .ui
            .agent
            .history
            .push(ChatMessage::user_text("original QM goal"));
        state
            .ui
            .agent
            .history
            .push(fallback_encode(&turn_with_tools(&[
                "sketch C",
                "qm energy",
            ])));
        state
            .ui
            .agent
            .collected_results
            .push(ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "Created methane".into(),
                is_error: false,
            });
        state.ui.agent.phase = phase;
        state.ui.agent.pending_calls.push_back(
            turn_with_tools(&["sketch C", "qm energy"])
                .tool_calls
                .remove(1),
        );
        state.ui.agent.approved_ids.insert("call_1".into());
        state
            .ui
            .agent
            .queued
            .push_back(PendingTurn::UserMessage("old instruction".into()));
        poll_agent_jobs(&mut state, &egui::Context::default());
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert!(state.jobs.agent.is_none());
        assert!(state.ui.agent.qm_diagnostic_only);
        assert!(state.ui.agent.pending_calls.is_empty());
        assert!(state.ui.agent.approved_ids.is_empty());
        assert!(state.ui.agent.queued.is_empty());
        assert_eq!(state.ui.agent.iterations, 0);
        assert!(
            state
                .ui
                .agent
                .transcript
                .iter()
                .any(|e| matches!(e, TranscriptEntry::Notice(n) if n.contains("Discarded 1")))
        );
        let history = format!("{:?}", state.ui.agent.history);
        assert!(history.contains("original QM goal"));
        assert!(history.contains("Created methane"));
        assert!(history.contains("Not executed: QM issue requires read-only diagnosis"));
        assert!(history.contains(&job_id.to_string()));
        assert!(history.contains("raw evidence"));
        assert!(history.contains("not converged"));
        assert!(history.contains("Evidence directory"));
        assert_eq!(
            state
                .tasks
                .runs
                .execution(&job_id.to_string())
                .unwrap()
                .execution_state,
            crate::job::ExecutionState::Succeeded
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn qm_diagnosis_attempts_model_only_when_enabled_and_does_not_unlock() {
    for enabled in [false, true] {
        let mut state = offline_state();
        state.config.assistant.auto_diagnose_qm_issues = enabled;
        let (_, dir) = completed_job(&mut state, false);
        poll_agent_jobs(&mut state, &egui::Context::default());
        let provider_attempted = state.ui.agent.transcript.iter().any(
            |e| matches!(e, TranscriptEntry::Notice(n) if n.contains("Unknown assistant provider")),
        );
        assert_eq!(provider_attempted, enabled);
        assert!(state.ui.agent.qm_diagnostic_only);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn qm_success_continues_without_restriction() {
    let mut state = offline_state();
    let (_, dir) = completed_job(&mut state, true);
    poll_agent_jobs(&mut state, &egui::Context::default());
    assert!(!state.ui.agent.qm_diagnostic_only);
    assert!(state.ui.agent.transcript.iter().any(
        |e| matches!(e, TranscriptEntry::Notice(n) if n.contains("Unknown assistant provider"))
    ));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn qm_cancel_does_not_diagnose_but_execution_failure_does() {
    for cancelled in [false, true] {
        let mut state = offline_state();
        state.config.assistant.auto_diagnose_qm_issues = false;
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(QmWorkerMessage::Failed("engine failed".into()))
            .unwrap();
        let mut tracked = fake_qm_job(1, state.ui.agent.active_conversation, rx);
        if let AgentHeavyJob::Qm(job) = &mut tracked.job {
            job.cancel_requested = cancelled;
        }
        state.jobs.agent_jobs.push(tracked);
        poll_agent_jobs(&mut state, &egui::Context::default());
        assert_eq!(state.ui.agent.qm_diagnostic_only, !cancelled);
        assert!(state.ui.agent.queued.is_empty());
    }
}

#[test]
fn qm_diagnostic_whitelist_precedes_auto_permanent_allow_and_approval() {
    for mode in [
        crate::backend::config::ApprovalMode::Auto,
        crate::backend::config::ApprovalMode::Plan,
    ] {
        for (name, input) in [
            ("run_command", json!({"command": "activate 999"})),
            ("run_command", json!({"command": "open missing.xyz"})),
            ("run_command", json!({"command": "qm energy"})),
            ("run_command", json!({"command": "save forbidden.xyz"})),
            ("save_skill", json!({"name": "forbidden", "content": "no"})),
            ("cancel_job", json!({"id": "1"})),
            ("unknown_tool", json!({})),
        ] {
            let mut state = offline_state();
            state.config.assistant.approval_mode = mode;
            state.ui.agent.qm_diagnostic_only = true;
            let call = ToolCall {
                id: "blocked".into(),
                name: name.into(),
                input,
            };
            state
                .ui
                .agent
                .allowed_verbs
                .insert(crate::frontend::agent::tools::call_allow_key(&call));
            state.ui.agent.approved_ids.insert(call.id.clone());
            state.ui.agent.pending_calls.push_back(call.clone());
            run_tool_batch(&mut state, &egui::Context::default());
            assert!(state.ui.agent.qm_diagnostic_only);
            assert!(state.jobs.agent_jobs.is_empty());
            assert!(state.entries.records.is_empty());
            assert!(state.ui.agent.transcript.iter().any(|e| matches!(e, TranscriptEntry::Tool { is_error: true, result: Some(r), .. } if r.contains("read only"))));
            assert!(crate::frontend::agent::tools::execute_tool(&mut state, &call).is_error);
        }
    }
}

#[test]
fn qm_restriction_survives_snapshot_switch_and_other_completion_until_new_user_message() {
    let mut state = offline_state();
    state.config.assistant.auto_diagnose_qm_issues = false;
    let origin = state.ui.agent.active_conversation;
    state
        .ui
        .agent
        .history
        .push(ChatMessage::user_text("original goal"));
    let (job_id, dir) = completed_job(&mut state, false);
    state.ui.agent.start_new_conversation(Default::default());
    let other = state.ui.agent.active_conversation;
    poll_agent_jobs(&mut state, &egui::Context::default());
    assert!(!state.ui.agent.qm_diagnostic_only);
    assert!(
        state
            .ui
            .agent
            .conversation_mut(origin)
            .unwrap()
            .qm_diagnostic_only
    );
    let json = serde_json::to_string(&state.ui.agent.project_snapshot()).unwrap();
    state.ui.agent = AgentSession::from_project_snapshot(
        serde_json::from_str(&json).unwrap(),
        Default::default(),
    );
    assert_eq!(state.ui.agent.active_conversation, other);
    state.ui.agent.switch_conversation(origin);
    assert!(state.ui.agent.qm_diagnostic_only);
    state.ui.agent.queued.push_back(PendingTurn::JobDone {
        label: "other job".into(),
        summary: "finished".into(),
        is_error: false,
    });
    pump_queue(&mut state, &egui::Context::default());
    assert!(state.ui.agent.qm_diagnostic_only);
    handle_turn_result(
        &mut state,
        Ok(end_turn("Wait for user.")),
        &egui::Context::default(),
    );
    assert!(state.ui.agent.qm_diagnostic_only);
    send_agent_message(
        &mut state,
        "Now inspect the result",
        &egui::Context::default(),
    );
    assert!(!state.ui.agent.qm_diagnostic_only);
    let history = format!("{:?}", state.ui.agent.history);
    assert!(history.contains("original goal"));
    assert!(history.contains("raw evidence"));
    assert!(history.contains("Wait for user."));
    assert!(
        !state
            .tasks
            .runs
            .execution(&job_id.to_string())
            .unwrap()
            .qm_result
            .as_ref()
            .unwrap()
            .converged
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn qm_saved_artifacts_and_convergence_are_independent() {
    for converged in [false, true] {
        for report in [
            ArtifactStatus::Saved,
            ArtifactStatus::Failed("disk full".into()),
        ] {
            let result = QmResult {
                converged,
                report: report.clone(),
                series: ArtifactStatus::NotApplicable,
            };
            assert_eq!(
                result.needs_diagnosis(),
                !converged || report != ArtifactStatus::Saved
            );
        }
    }
}

#[test]
fn qm_diagnosis_keeps_restriction_across_read_only_tool_rounds_and_iteration_limit() {
    let mut state = offline_state();
    state.ui.agent.qm_diagnostic_only = true;
    state.config.assistant.auto_diagnose_qm_issues = false;
    let mut turn = end_turn("Inspecting evidence");
    turn.stop = StopReason::ToolUse;
    turn.tool_calls.push(ToolCall {
        id: "inspect".into(),
        name: "inspect".into(),
        input: json!({}),
    });
    handle_turn_result(&mut state, Ok(turn), &egui::Context::default());
    assert!(state.ui.agent.qm_diagnostic_only);
    assert!(state.ui.agent.transcript.iter().any(|e| matches!(
        e,
        TranscriptEntry::Tool {
            is_error: false,
            result: Some(_),
            ..
        }
    )));
    state.ui.agent.iterations = MAX_ITERATIONS;
    spawn_next_turn(&mut state, &egui::Context::default());
    assert_eq!(state.ui.agent.phase, AgentPhase::Done);
    assert!(state.jobs.agent.is_none());
    assert!(state.ui.agent.qm_diagnostic_only);
}

#[test]
fn qm_completion_is_processed_before_a_simultaneously_ready_model_response() {
    let mut state = offline_state();
    state.config.assistant.approval_mode = crate::backend::config::ApprovalMode::Auto;
    state.config.assistant.auto_diagnose_qm_issues = false;
    state.ui.agent.key_available = Some(false);
    let (_, dir) = completed_job(&mut state, false);
    let (tx, receiver) = std::sync::mpsc::channel();
    tx.send(crate::frontend::jobs::AgentTurnEvent::Done(Ok(
        turn_with_tool("sketch C"),
    )))
    .unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    state.jobs.agent = Some(RunningAgentTurn {
        cancel: cancel.clone(),
        receiver,
    });
    state.ui.agent.phase = AgentPhase::AwaitingModel;
    crate::frontend::dispatcher::poll_jobs(&mut state, &egui::Context::default());
    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert!(state.ui.agent.qm_diagnostic_only);
    assert!(state.entries.records.is_empty());
    assert!(
        !state
            .ui
            .agent
            .transcript
            .iter()
            .any(|e| matches!(e, TranscriptEntry::Tool { .. }))
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn qm_converged_result_with_report_failure_still_requires_diagnosis() {
    let mut state = offline_state();
    state.config.assistant.auto_diagnose_qm_issues = false;
    let (job, dir) = completed_job(&mut state, true);
    std::fs::create_dir_all(dir.join(crate::frontend::dispatcher::QM_OUTPUT_FILE)).unwrap();
    poll_agent_jobs(&mut state, &egui::Context::default());
    assert!(state.ui.agent.qm_diagnostic_only);
    let execution = state.tasks.runs.execution(&job.to_string()).unwrap();
    assert_eq!(
        execution.execution_state,
        crate::job::ExecutionState::Succeeded
    );
    assert!(execution.qm_result.as_ref().unwrap().converged);
    assert!(!execution.qm_result.as_ref().unwrap().artifacts_complete());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn qm_worker_channel_loss_restricts_the_session() {
    let mut state = offline_state();
    state.config.assistant.auto_diagnose_qm_issues = false;
    let (tx, rx) = std::sync::mpsc::channel();
    drop(tx);
    state
        .jobs
        .agent_jobs
        .push(fake_qm_job(1, state.ui.agent.active_conversation, rx));
    poll_agent_jobs(&mut state, &egui::Context::default());
    assert!(state.jobs.agent_jobs.is_empty());
    assert!(state.ui.agent.qm_diagnostic_only);
    assert!(
        state
            .ui
            .agent
            .transcript
            .iter()
            .any(|e| matches!(e, TranscriptEntry::Notice(n) if n.contains("worker stopped")))
    );
}
