use super::*;
use crate::backend::tasks::TaskStatus;
use crate::frontend::state::AppState;

#[test]
fn agent_qm_subcommands_map_to_controllers() {
    assert_eq!(
        agent_task_controller_id(HeavyKind::Qm, "qm energy"),
        "qm-energy"
    );
    assert_eq!(
        agent_task_controller_id(HeavyKind::Qm, "qm opt"),
        "qm-optimize"
    );
    assert_eq!(
        agent_task_controller_id(HeavyKind::Qm, "qm freq"),
        "qm-frequencies"
    );
    assert_eq!(
        agent_task_controller_id(HeavyKind::Qm, "qm ts"),
        "qm-transition-state"
    );
    assert_eq!(agent_task_controller_id(HeavyKind::Md, "md run"), "run-md");
    assert_eq!(
        agent_task_controller_id(HeavyKind::Dock, "dock lig"),
        "dock-ligand"
    );
}

#[test]
fn register_creates_a_ready_task_run() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let id = register_agent_task_run(&mut state, HeavyKind::Qm, "qm optimize");
    let task = state.tasks.task_run(id).expect("task run created");
    assert_eq!(task.controller_id, "qm-optimize");
    assert_eq!(task.status, TaskStatus::Ready);
}

#[test]
fn agent_qm_completion_creates_the_optimized_entry_and_records_the_result() {
    // QM vertical slice (agent placement): an agent-driven QM run drains to
    // completion, adds its optimized geometry as an entry, and records it as the
    // task's result — attributed by the TrackedAgentJob's task_run_id.
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let task = register_agent_task_run(&mut state, HeavyKind::Qm, "qm optimize");
    let job_id = crate::frontend::dispatcher::begin_job_execution(
        &mut state,
        task,
        crate::backend::run_attempt::Placement::Local,
        Some("qm-optimize".to_string()),
    );

    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(QmWorkerMessage::Finished(Box::new(
        crate::engines::qm::QmOutcome {
            energy_hartree: -1.0,
            converged: true,
            optimized_structure: Some(crate::domain::Structure::empty()),
            summary: "energy -1.0 Eh".to_string(),
            scf_trace: Vec::new(),
            opt_trace: Vec::new(),
            frequencies: Vec::new(),
        },
    )))
    .unwrap();
    let mut running = RunningQmJob {
        cancel: crate::wire::JobCancelHandle::from_flag(std::sync::Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        )),
        receiver: rx,
        latest_stage: None,
        cancel_requested: false,
    };

    let completion = drain_qm(&mut state, &mut running, task, job_id);
    let (_summary, is_error) = completion.expect("the job completes");
    assert!(!is_error, "a converged run is not an error");
    assert!(
        state
            .tasks
            .task_run(task)
            .unwrap()
            .result_entry_id
            .is_some(),
        "the optimized geometry is recorded as the task result"
    );
}

#[test]
fn complete_marks_terminal_status() {
    // An agent-launched job finalizes through the same run graph a manual job
    // does: completing its bound execution marks the task terminal.
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ok = register_agent_task_run(&mut state, HeavyKind::Qm, "qm energy");
    let ok_job = crate::frontend::dispatcher::begin_job_execution(
        &mut state,
        ok,
        crate::backend::run_attempt::Placement::Local,
        Some("qm-energy".to_string()),
    );
    crate::frontend::dispatcher::complete_local_job(
        &mut state,
        Some(ok_job),
        TaskStatus::Completed,
    );
    assert_eq!(
        state.tasks.task_run(ok).unwrap().status,
        TaskStatus::Completed
    );

    let bad = register_agent_task_run(&mut state, HeavyKind::Md, "md run");
    let bad_job = crate::frontend::dispatcher::begin_job_execution(
        &mut state,
        bad,
        crate::backend::run_attempt::Placement::Local,
        Some("md-run".to_string()),
    );
    crate::frontend::dispatcher::complete_local_job(&mut state, Some(bad_job), TaskStatus::Failed);
    assert_eq!(
        state.tasks.task_run(bad).unwrap().status,
        TaskStatus::Failed
    );
}
