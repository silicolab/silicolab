use super::*;
use crate::backend::storage::jobs as registry;

fn qm_remote_row(job_id: &str, run_dir: &std::path::Path) -> registry::RemoteJob {
    registry::RemoteJob {
        job_id: job_id.to_string(),
        host_id: "h".to_string(),
        host_label: "H".to_string(),
        remote_dir: "~/.silicolab/runs/x".to_string(),
        scheduler: "direct".to_string(),
        launch_handle: "1".to_string(),
        cluster: None,
        engine_id: "hartree".to_string(),
        job_kind: "qm-energy".to_string(),
        project_id: None,
        project_root_hint: None,
        local_run_dir: run_dir.to_string_lossy().to_string(),
        status: registry::RemoteJobStatus::Done,
        submitted_at_ms: 0,
        last_polled_at_ms: None,
        exit_code: None,
        scheduler_state: None,
        reason: None,
        console_offset: 0,
        unknown_since_ms: None,
    }
}

#[test]
fn remote_qm_report_records_once_and_creates_no_entry() {
    // import cardinality (0 entry): a single-point energy report creates no
    // entry but records a ledger row proving the outcome was applied, so a
    // repeated apply neither duplicates nor is treated as un-imported.
    let run_dir = std::path::PathBuf::from("target/test-qm-report-idempotency");
    let _ = std::fs::remove_dir_all(&run_dir);
    std::fs::create_dir_all(&run_dir).unwrap();
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let row = qm_remote_row("job-qm", &run_dir);
    let outcome = crate::engines::qm::QmOutcome {
        energy_hartree: -1.5,
        converged: true,
        optimized_structure: None,
        summary: "energy -1.5 Eh".to_string(),
        scf_trace: Vec::new(),
        opt_trace: Vec::new(),
        frequencies: Vec::new(),
    };

    apply_remote_qm_outcome(&mut state, &row, outcome.clone());
    assert!(
        state.entries.records.is_empty(),
        "an energy report creates no entry"
    );
    let record = state
        .materializations
        .get("job-qm")
        .expect("the report is recorded in the ledger");
    assert!(record.primary_entry_id.is_none());
    assert!(record.entries.is_empty());

    apply_remote_qm_outcome(&mut state, &row, outcome);
    assert!(state.entries.records.is_empty());
    assert_eq!(state.materializations.len(), 1);
}

#[test]
fn open_project_compensation_imports_present_outcome_and_flags_missing() {
    // terminal compensation: a completed row whose outcome.json is on disk is
    // imported (ledger record); one whose file is gone stays unmaterialized —
    // surfaced as a pending recovery, never silently marked done.
    let root = std::path::PathBuf::from("target/test-compensation");
    let _ = std::fs::remove_dir_all(&root);
    let present_dir = root.join("present");
    std::fs::create_dir_all(&present_dir).unwrap();
    let outcome = crate::wire::EngineOutcome::Qm(crate::engines::qm::QmOutcome {
        energy_hartree: -2.0,
        converged: true,
        optimized_structure: None,
        summary: "energy -2.0 Eh".to_string(),
        scf_trace: Vec::new(),
        opt_trace: Vec::new(),
        frequencies: Vec::new(),
    });
    std::fs::write(
        present_dir.join(crate::engines::remote::launcher::OUTCOME_FILE),
        serde_json::to_vec(&outcome).unwrap(),
    )
    .unwrap();

    let mut state = AppState::scratch(Default::default(), Vec::new());
    let present = qm_remote_row("job-present", &present_dir);
    let missing = qm_remote_row("job-missing", &root.join("missing"));

    import_completed_remote_jobs(&mut state, vec![present, missing]);

    assert!(
        state.materializations.contains("job-present"),
        "the downloaded outcome is imported"
    );
    assert!(
        !state.materializations.contains("job-missing"),
        "a missing outcome stays pending, not marked done"
    );
    assert!(
        state
            .output_log
            .iter()
            .any(|line| line.contains("pending recovery")),
        "the pending recovery is surfaced, not silently dropped"
    );
}

#[test]
fn pure_timeout_never_escalates_a_remote_job_to_lost() {
    // a remote job made unreachable by a pure timeout keeps its last
    // confirmed status forever; only explicit launcher evidence settles Lost.
    use crate::engines::remote::launcher::RemoteJobPhase;
    let mut row = qm_remote_row("job-x", std::path::Path::new("target/test-observe"));
    row.status = registry::RemoteJobStatus::Running;
    row.unknown_since_ms = Some(0);

    let (status, unknown_since) = observed_status(&row, RemoteJobPhase::Unknown, 10 * 60_000);
    assert_eq!(
        status,
        registry::RemoteJobStatus::Running,
        "an unreachable job keeps its execution status through a pure timeout"
    );
    assert_eq!(unknown_since, Some(0), "the freshness signal is preserved");

    let (evidenced, _) = observed_status(&row, RemoteJobPhase::Lost, 10 * 60_000);
    assert_eq!(
        evidenced,
        registry::RemoteJobStatus::Lost,
        "explicit launcher evidence still settles a terminal Lost"
    );
}

#[test]
fn compensation_marks_the_execution_pending_recovery_when_the_outcome_is_missing() {
    // a completed remote job whose downloaded outcome file is gone records a
    // durable PendingRecovery on its execution (not just a log line), so the UI can
    // surface it and a later refresh can retry.
    use crate::backend::run_attempt::{Placement, ResultImport};
    use crate::backend::tasks::task_controller_by_id;

    let mut state = AppState::scratch(Default::default(), Vec::new());
    let task = state
        .tasks
        .create_task_run(*task_controller_by_id("qm-energy").unwrap());
    let job_id = state
        .tasks
        .runs
        .begin_execution(task, Placement::Remote { host: None }, None, 0);
    let run_uuid = job_id.to_string();

    let missing_dir = std::path::PathBuf::from("target/test-missing-recovery");
    let _ = std::fs::remove_dir_all(&missing_dir);
    let missing = qm_remote_row(&run_uuid, &missing_dir);
    import_completed_remote_jobs(&mut state, vec![missing]);

    let execution = state
        .tasks
        .runs
        .executions()
        .iter()
        .find(|execution| execution.job_id == job_id)
        .unwrap();
    assert_eq!(execution.import_state, ResultImport::PendingRecovery);
    assert!(
        !state.materializations.contains(&run_uuid),
        "a missing outcome is not marked applied"
    );
}

#[test]
fn observation_and_execution_axes_split_on_a_remote_row() {
    // a remote row projects two orthogonal axes. A pure timeout flips
    // observation to Unreachable while its execution status (and projection) is
    // unchanged; a confirmed phase restores Observed and projects its new state.
    use crate::engines::remote::launcher::RemoteJobPhase;
    use crate::job::{ExecutionState, ObservationState};

    let mut row = qm_remote_row("job-obs", std::path::Path::new("target/test-obs-axes"));
    row.status = registry::RemoteJobStatus::Running;
    row.unknown_since_ms = None;
    assert_eq!(row.observation_state(), ObservationState::Observed);
    assert_eq!(row.status.execution_state(), ExecutionState::Running);

    let (status, unknown_since) = observed_status(&row, RemoteJobPhase::Unknown, 5_000);
    row.status = status;
    row.unknown_since_ms = unknown_since;
    assert_eq!(row.status.execution_state(), ExecutionState::Running);
    assert_eq!(row.observation_state(), ObservationState::Unreachable);

    let (status, unknown_since) = observed_status(&row, RemoteJobPhase::Succeeded, 6_000);
    row.status = status;
    row.unknown_since_ms = unknown_since;
    assert_eq!(row.observation_state(), ObservationState::Observed);
    assert_eq!(row.status.execution_state(), ExecutionState::Succeeded);
}

#[test]
fn a_confirmed_remote_observation_advances_the_run_graph_execution_state() {
    // A confirmed remote observation mirrors the registry status onto the run
    // graph, so a remote JobExecution advances past Queued without waiting for a
    // local completion.
    use crate::backend::run_attempt::Placement;
    use crate::backend::tasks::task_controller_by_id;
    use crate::engines::remote::launcher::{ConsoleChunk, LauncherObservation, RemoteJobPhase};
    use crate::frontend::remote_jobs::RemoteJobOutcome;
    use crate::job::ExecutionState;

    let mut state = AppState::scratch(Default::default(), Vec::new());
    let task = state
        .tasks
        .create_task_run(*task_controller_by_id("qm-energy").unwrap());
    let job_id = state
        .tasks
        .runs
        .begin_execution(task, Placement::Remote { host: None }, None, 0);
    let run_uuid = job_id.to_string();

    let db_path = std::env::temp_dir().join(format!(
        "silicolab-obs-graph-{}.db",
        uuid::Uuid::new_v4().simple()
    ));
    let _ = std::fs::remove_file(&db_path);
    let conn = registry::open_at(&db_path).unwrap();
    let mut row = qm_remote_row(&run_uuid, std::path::Path::new("target/test-obs-graph"));
    row.status = registry::RemoteJobStatus::Running;
    registry::upsert(&conn, &row).unwrap();

    let observation = LauncherObservation {
        phase: RemoteJobPhase::Running,
        scheduler_state: None,
        reason: None,
        exit_code: None,
        console: ConsoleChunk {
            text: String::new(),
            next_offset: 0,
        },
    };
    apply_remote_observation(
        &mut state,
        Some(&conn),
        &run_uuid,
        RemoteJobOutcome::Observed(observation),
    );

    let execution = state
        .tasks
        .runs
        .executions()
        .iter()
        .find(|execution| execution.job_id == job_id)
        .unwrap();
    assert_eq!(execution.execution_state, ExecutionState::Running);
    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn ownerless_job_in_scratch_session_belongs_here() {
    // A build submitted with no project open (`None`) and refreshed in a scratch
    // session (`None`) shares an origin, so equality of origins materializes it
    // into scratch.
    assert!(project_root_matches(None, None));
}

#[test]
fn matching_project_belongs_here() {
    assert!(project_root_matches(Some("/work/a"), Some("/work/a")));
}

#[test]
fn mismatched_origin_is_left_for_its_own_workspace() {
    // A different project open, an owned job with no project open, or an
    // ownerless job with a project open: none is dumped into the wrong place.
    assert!(!project_root_matches(Some("/work/a"), Some("/work/b")));
    assert!(!project_root_matches(Some("/work/a"), None));
    assert!(!project_root_matches(None, Some("/work/b")));
}
