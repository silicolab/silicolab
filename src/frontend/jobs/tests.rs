use super::*;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn qm_cancel(flag: std::sync::Arc<std::sync::atomic::AtomicBool>) -> crate::wire::JobCancelHandle {
    crate::wire::JobCancelHandle::from_flag(flag)
}

#[test]
fn apply_metrics_sampler_starts_and_stops() {
    let mut jobs = JobManager::default();
    let interval = Some(Duration::from_millis(500));
    apply_metrics_sampler(&mut jobs, true, interval);
    assert!(
        jobs.metrics.is_some(),
        "turning on should spawn the sampler"
    );
    apply_metrics_sampler(&mut jobs, true, interval); // idempotent — no second sampler
    assert!(jobs.metrics.is_some());
    apply_metrics_sampler(&mut jobs, false, None);
    assert!(
        jobs.metrics.is_none(),
        "turning off should drop the sampler"
    );
}

#[test]
fn refresh_interval_maps_rates_and_pauses() {
    use crate::backend::config::MonitorRefresh;
    assert_eq!(
        refresh_interval(MonitorRefresh::High),
        Some(Duration::from_millis(500))
    );
    assert_eq!(
        refresh_interval(MonitorRefresh::Standard),
        Some(Duration::from_millis(1000))
    );
    assert_eq!(
        refresh_interval(MonitorRefresh::Low),
        Some(Duration::from_secs(4))
    );
    assert_eq!(refresh_interval(MonitorRefresh::Pause), None);
}

#[test]
fn gpu_interval_floors_and_backs_off_when_idle() {
    use crate::frontend::gpu_monitor::GpuSample;
    let sample = |util: Option<f32>| GpuSample {
        pci_bus_id: "01:00.0".into(),
        util_pct: util,
        vram_used_bytes: None,
        vram_total_bytes: None,
        temp_c: None,
    };
    // No readings yet: hold the floor so cards are still discovered promptly.
    assert_eq!(
        gpu_interval(Duration::from_millis(500), &[]),
        GPU_MIN_INTERVAL
    );
    // A busy card: floored to the minimum even at the fastest base rate.
    assert_eq!(
        gpu_interval(Duration::from_millis(500), &[sample(Some(73.0))]),
        GPU_MIN_INTERVAL
    );
    // An idle card: stretched to the longer back-off interval.
    assert_eq!(
        gpu_interval(Duration::from_millis(500), &[sample(Some(0.0))]),
        GPU_IDLE_INTERVAL
    );
    // A slow base rate still wins when it exceeds the floor.
    assert_eq!(
        gpu_interval(Duration::from_secs(30), &[sample(Some(90.0))]),
        Duration::from_secs(30)
    );
}

#[test]
fn parse_model_ids_reads_data_id_list() {
    let json = json!({ "data": [{ "id": "x" }, { "id": "y" }] });
    assert_eq!(parse_model_ids(&json), vec!["x", "y"]);
}

#[test]
fn parse_model_ids_ignores_garbage() {
    // Wrong shape, missing `data`, or non-object items all yield nothing.
    assert!(parse_model_ids(&json!({ "models": ["x"] })).is_empty());
    assert!(parse_model_ids(&json!([1, 2, 3])).is_empty());
    assert!(parse_model_ids(&json!("nope")).is_empty());
    // Items without a string `id` are skipped, not faked.
    assert_eq!(
        parse_model_ids(&json!({ "data": [{ "id": "ok" }, { "name": "no-id" }] })),
        vec!["ok"]
    );
}

#[test]
fn interpret_models_response_reads_ids_on_ok() {
    assert_eq!(
        interpret_models_response(200, r#"{"data":[{"id":"x"},{"id":"y"}]}"#),
        Ok(vec!["x".to_string(), "y".to_string()])
    );
}

#[test]
fn interpret_models_response_html_points_at_base_url() {
    // The exact symptom the user hit: Base URL without `/v1` returns the
    // relay's web page, not JSON. The error must read like the assistant path —
    // name the HTML page and point at the `/v1` API root, not raw serde.
    let err = interpret_models_response(200, "<!doctype html><html></html>").unwrap_err();
    assert!(err.contains("HTML"), "got: {err}");
    assert!(err.contains("/v1"), "got: {err}");
    assert!(!err.contains("malformed"), "leaks serde wording: {err}");
}

#[test]
fn interpret_models_response_empty_body_flags_base_url() {
    let err = interpret_models_response(200, "   ").unwrap_err();
    assert!(err.contains("empty"), "got: {err}");
}

#[test]
fn interpret_models_response_non_json_error_page_hints_url_regardless_of_status() {
    // A wrong Base URL can 404 to an HTML page too; that is still a
    // wrong-URL signal, so it gets the same hint rather than a bare status.
    let err = interpret_models_response(404, "<html>not found</html>").unwrap_err();
    assert!(err.contains("HTML"), "got: {err}");
}

#[test]
fn interpret_models_response_json_error_reports_status() {
    // A valid JSON body with a non-200 status is a real API error, not a
    // wrong URL — surface the status.
    let err = interpret_models_response(503, r#"{"error":"nope"}"#).unwrap_err();
    assert!(err.contains("503"), "got: {err}");
}

#[test]
fn interpret_models_response_json_error_reports_message() {
    let err = interpret_models_response(
        401,
        r#"{"code":"API_KEY_REQUIRED","message":"API key is required"}"#,
    )
    .unwrap_err();
    assert!(err.contains("401"), "got: {err}");
    assert!(err.contains("API key is required"), "got: {err}");
}

#[test]
fn local_job_snapshots_enumerate_live_slots() {
    let mut jobs = JobManager::default();
    let (_qm_tx, qm_rx) = std::sync::mpsc::channel();
    jobs.qm = Some(RunningQmJob {
        cancel: qm_cancel(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            false,
        ))),
        receiver: qm_rx,
        latest_stage: Some("SCF".to_string()),
        cancel_requested: false,
    });
    let (_engine_tx, engine_rx) = std::sync::mpsc::channel();
    jobs.engine = Some(RunningEngineJob {
        engine: "gromacs",
        job_kind: "run-md",
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        receiver: engine_rx,
        latest_stage: Some("nvt".to_string()),
        log_tail: Vec::new(),
    });

    let snapshots = jobs.list_live_snapshots(Some(42));

    assert_eq!(snapshots.len(), 2);
    assert!(snapshots.iter().any(|snapshot| {
        snapshot.id == JobControlId::Local(LocalJobSlot::Qm)
            && snapshot.kind == JobKind::Qm
            && snapshot.stage.as_deref() == Some("SCF")
            && snapshot.task_run_id == Some(42)
    }));
    assert!(snapshots.iter().any(|snapshot| {
        snapshot.id == JobControlId::Local(LocalJobSlot::Engine)
            && snapshot.engine_id.as_deref() == Some("gromacs")
            && snapshot.job_kind.as_deref() == Some("run-md")
            && snapshot.stage.as_deref() == Some("nvt")
    }));
}

#[test]
fn agent_job_cancel_routes_through_control_plane() {
    let mut state = crate::frontend::state::AppState::scratch(Default::default(), Vec::new());
    let controller = crate::backend::tasks::task_controller_by_id("qm-energy")
        .copied()
        .unwrap();
    let task_run_id = state.tasks.create_task_run(controller);
    state
        .tasks
        .mark_status(task_run_id, crate::backend::tasks::TaskStatus::Running);
    let (_tx, rx) = std::sync::mpsc::channel();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    state.jobs.agent_jobs.push(TrackedAgentJob {
        id: 7,
        conversation: state.ui.agent.active_conversation,
        label: "qm energy".to_string(),
        task_run_id,
        job: AgentHeavyJob::Qm(RunningQmJob {
            cancel: qm_cancel(std::sync::Arc::clone(&cancel)),
            receiver: rx,
            latest_stage: None,
            cancel_requested: false,
        }),
    });

    let outcome = cancel_controlled_job(&mut state, &JobControlId::Agent(7)).unwrap();

    assert!(matches!(
        outcome,
        CancelOutcome::Requested {
            id: JobControlId::Agent(7),
            task_run_id: Some(id),
        } if id == task_run_id
    ));
    assert!(cancel.load(Ordering::Relaxed));
    assert_eq!(state.jobs.agent_jobs.len(), 1);
    assert_eq!(
        state.tasks.task_run(task_run_id).unwrap().status,
        crate::backend::tasks::TaskStatus::Cancelling
    );
}

#[test]
fn cancel_transient_jobs_routes_local_slots_through_control_plane() {
    let mut state = crate::frontend::state::AppState::scratch(Default::default(), Vec::new());
    let controller = crate::backend::tasks::task_controller_by_id("qm-energy")
        .copied()
        .unwrap();
    let task_run_id = state.tasks.create_task_run(controller);
    state.active_task_run = Some(task_run_id);
    state
        .tasks
        .mark_status(task_run_id, crate::backend::tasks::TaskStatus::Running);
    let (_tx, rx) = std::sync::mpsc::channel();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    state.jobs.qm = Some(RunningQmJob {
        cancel: qm_cancel(std::sync::Arc::clone(&cancel)),
        receiver: rx,
        latest_stage: None,
        cancel_requested: false,
    });

    state.cancel_transient_jobs();

    assert!(cancel.load(Ordering::Relaxed));
    assert!(state.jobs.qm.is_some());
    assert_eq!(
        state.tasks.task_run(task_run_id).unwrap().status,
        crate::backend::tasks::TaskStatus::Cancelling
    );
    assert_eq!(state.active_task_run, Some(task_run_id));
}

#[test]
fn agent_job_snapshots_include_latest_stage() {
    let mut jobs = JobManager::default();
    let (_tx, rx) = std::sync::mpsc::channel();
    jobs.agent_jobs.push(TrackedAgentJob {
        id: 9,
        conversation: crate::frontend::agent::AssistantConversationId::new(1),
        label: "qm energy".to_string(),
        task_run_id: 44,
        job: AgentHeavyJob::Qm(RunningQmJob {
            cancel: qm_cancel(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                false,
            ))),
            receiver: rx,
            latest_stage: Some("SCF".to_string()),
            cancel_requested: false,
        }),
    });

    let snapshots = jobs.list_live_snapshots(None);

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].id, JobControlId::Agent(9));
    assert_eq!(snapshots[0].kind, JobKind::AssistantQm);
    assert_eq!(snapshots[0].stage.as_deref(), Some("SCF"));
    assert_eq!(snapshots[0].task_run_id, Some(44));
}

#[test]
fn remote_job_snapshot_maps_last_known_registry_state() {
    let row = remote_row(
        "run-1",
        crate::backend::storage::jobs::RemoteJobStatus::Running,
    );

    let snapshot = remote_job_snapshot(&row);

    assert_eq!(snapshot.id, JobControlId::Remote("run-1".to_string()));
    assert_eq!(snapshot.backend, JobBackend::RemoteRegistry);
    assert_eq!(snapshot.kind, JobKind::RemoteEngine);
    assert_eq!(snapshot.status, JobStatus::Running);
    assert_eq!(snapshot.cancel, crate::job::CancelCapability::Preemptive);
    assert_eq!(snapshot.engine_id.as_deref(), Some("hartree"));
    assert_eq!(snapshot.job_kind.as_deref(), Some("qm-energy"));
    assert_eq!(snapshot.host_label.as_deref(), Some("Cluster"));
}

#[test]
fn qm_cancel_alias_refuses_multiple_qm_jobs_without_cancelling_any() {
    let mut state = crate::frontend::state::AppState::scratch(Default::default(), Vec::new());
    let local_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (_local_tx, local_rx) = std::sync::mpsc::channel();
    state.jobs.qm = Some(RunningQmJob {
        cancel: qm_cancel(std::sync::Arc::clone(&local_cancel)),
        receiver: local_rx,
        latest_stage: Some("local".into()),
        cancel_requested: false,
    });

    let agent_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (_agent_tx, agent_rx) = std::sync::mpsc::channel();
    let controller = crate::backend::tasks::task_controller_by_id("qm-optimize")
        .copied()
        .unwrap();
    let task_run_id = state.tasks.create_task_run(controller);
    state.jobs.agent_jobs.push(TrackedAgentJob {
        id: 12,
        conversation: state.ui.agent.active_conversation,
        label: "qm optimize".to_string(),
        task_run_id,
        job: AgentHeavyJob::Qm(RunningQmJob {
            cancel: qm_cancel(std::sync::Arc::clone(&agent_cancel)),
            receiver: agent_rx,
            latest_stage: Some("agent".into()),
            cancel_requested: false,
        }),
    });

    let message = cancel_qm_job_alias(&mut state).unwrap();

    assert!(
        message.contains("Multiple QM jobs are running"),
        "{message}"
    );
    assert!(message.contains("local:qm"), "{message}");
    assert!(message.contains("agent:12"), "{message}");
    assert!(!local_cancel.load(Ordering::Relaxed));
    assert!(!agent_cancel.load(Ordering::Relaxed));
    assert!(state.jobs.qm.is_some());
    assert_eq!(state.jobs.agent_jobs.len(), 1);
}

#[test]
fn jobs_status_shows_agent_job_and_remote_status_as_last_known() {
    let mut state = crate::frontend::state::AppState::scratch(Default::default(), Vec::new());
    let (_tx, rx) = std::sync::mpsc::channel();
    let controller = crate::backend::tasks::task_controller_by_id("qm-optimize")
        .copied()
        .unwrap();
    let task_run_id = state.tasks.create_task_run(controller);
    state.jobs.agent_jobs.push(TrackedAgentJob {
        id: 7,
        conversation: state.ui.agent.active_conversation,
        label: "qm optimize".to_string(),
        task_run_id,
        job: AgentHeavyJob::Qm(RunningQmJob {
            cancel: qm_cancel(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                false,
            ))),
            receiver: rx,
            latest_stage: Some("SCF".into()),
            cancel_requested: false,
        }),
    });

    let status = crate::frontend::console::execute_console_line(&mut state, "jobs status").unwrap();
    assert!(status.contains("agent:7"), "{status}");
    assert!(status.contains("qm optimize"), "{status}");

    let remote = remote_job_snapshot(&remote_row(
        "run-3",
        crate::backend::storage::jobs::RemoteJobStatus::Running,
    ));
    assert!(
        format_job_row(&remote).contains("last-known:running"),
        "{}",
        format_job_row(&remote)
    );
}

#[test]
fn jobs_cancel_and_control_plane_cancel_have_matching_qm_message_and_effect() {
    fn state_with_agent_qm() -> (
        crate::frontend::state::AppState,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let mut state = crate::frontend::state::AppState::scratch(Default::default(), Vec::new());
        let (_tx, rx) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let controller = crate::backend::tasks::task_controller_by_id("qm-optimize")
            .copied()
            .unwrap();
        let task_run_id = state.tasks.create_task_run(controller);
        state.jobs.agent_jobs.push(TrackedAgentJob {
            id: 7,
            conversation: state.ui.agent.active_conversation,
            label: "qm optimize".to_string(),
            task_run_id,
            job: AgentHeavyJob::Qm(RunningQmJob {
                cancel: qm_cancel(std::sync::Arc::clone(&cancel)),
                receiver: rx,
                latest_stage: None,
                cancel_requested: false,
            }),
        });
        (state, cancel)
    }

    let (mut console_state, console_cancel) = state_with_agent_qm();
    let console =
        crate::frontend::console::execute_console_line(&mut console_state, "jobs cancel agent:7")
            .unwrap();

    let (mut direct_state, direct_cancel) = state_with_agent_qm();
    let jobs = list_controlled_jobs(&direct_state);
    let job = jobs.iter().find(|job| job.id.token() == "agent:7").cloned();
    let outcome = cancel_controlled_job(&mut direct_state, &JobControlId::Agent(7)).unwrap();
    let direct = format_cancel_outcome_for_job(&outcome, job.as_ref());

    assert_eq!(console, direct);
    assert!(console.contains("current stage may finish before stopping"));
    assert!(console_cancel.load(Ordering::Relaxed));
    assert!(direct_cancel.load(Ordering::Relaxed));
    assert_eq!(console_state.jobs.agent_jobs.len(), 1);
    assert_eq!(direct_state.jobs.agent_jobs.len(), 1);
}

#[test]
fn task_monitor_action_and_assistant_cancel_have_same_agent_job_effect() {
    fn state_with_agent_qm() -> (
        crate::frontend::state::AppState,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        u64,
    ) {
        let mut state = crate::frontend::state::AppState::scratch(Default::default(), Vec::new());
        let (_tx, rx) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let controller = crate::backend::tasks::task_controller_by_id("qm-optimize")
            .copied()
            .unwrap();
        let task_run_id = state.tasks.create_task_run(controller);
        state
            .tasks
            .mark_status(task_run_id, crate::backend::tasks::TaskStatus::Running);
        state.jobs.agent_jobs.push(TrackedAgentJob {
            id: 7,
            conversation: state.ui.agent.active_conversation,
            label: "qm optimize".to_string(),
            task_run_id,
            job: AgentHeavyJob::Qm(RunningQmJob {
                cancel: qm_cancel(std::sync::Arc::clone(&cancel)),
                receiver: rx,
                latest_stage: None,
                cancel_requested: false,
            }),
        });
        (state, cancel, task_run_id)
    }

    let ctx = eframe::egui::Context::default();
    let (mut monitor_state, monitor_cancel, monitor_task) = state_with_agent_qm();
    crate::frontend::dispatcher::dispatch(
        &mut monitor_state,
        crate::frontend::actions::AppAction::CancelControlledJob(JobControlId::Agent(7)),
        &ctx,
    );

    let (mut assistant_state, assistant_cancel, assistant_task) = state_with_agent_qm();
    let outcome = crate::frontend::agent::tools::execute_tool(
        &mut assistant_state,
        &crate::io::llm::types::ToolCall {
            id: "cancel".into(),
            name: "cancel_job".into(),
            input: serde_json::json!({ "id": "agent:7" }),
        },
    );

    assert!(monitor_cancel.load(Ordering::Relaxed));
    assert!(assistant_cancel.load(Ordering::Relaxed));
    assert_eq!(monitor_state.jobs.agent_jobs.len(), 1);
    assert_eq!(assistant_state.jobs.agent_jobs.len(), 1);
    assert_eq!(
        monitor_state.tasks.task_run(monitor_task).unwrap().status,
        crate::backend::tasks::TaskStatus::Cancelling
    );
    assert_eq!(
        assistant_state
            .tasks
            .task_run(assistant_task)
            .unwrap()
            .status,
        crate::backend::tasks::TaskStatus::Cancelling
    );
    assert_eq!(monitor_state.message, outcome.content);
}

fn remote_row(
    run_uuid: &str,
    status: crate::backend::storage::jobs::RemoteJobStatus,
) -> crate::backend::storage::jobs::RemoteJob {
    crate::backend::storage::jobs::RemoteJob {
        job_id: run_uuid.to_string(),
        host_id: "hpc".to_string(),
        host_label: "Cluster".to_string(),
        remote_dir: format!("~/.silicolab/runs/{run_uuid}"),
        scheduler: "direct".to_string(),
        launch_handle: "12345".to_string(),
        cluster: None,
        engine_id: "hartree".to_string(),
        job_kind: "qm-energy".to_string(),
        project_id: Some("proj-id".to_string()),
        project_root_hint: Some("/work/proj".to_string()),
        local_run_dir: "/tmp/run".to_string(),
        status,
        submitted_at_ms: 1000,
        last_polled_at_ms: None,
        exit_code: None,
        scheduler_state: None,
        reason: None,
        console_offset: 0,
        unknown_since_ms: None,
    }
}
