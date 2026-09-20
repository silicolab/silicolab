use super::*;
use crate::backend::tasks::task_controller_by_id;
use crate::engines::qm::QmOutcome;

fn add(state: &mut AppState, name: &str) -> u64 {
    state.entries.add_entry(
        Structure::new(name, Vec::new()),
        None,
        PathBuf::from(format!("{name}.xyz")),
    )
}

fn prepare(state: &mut AppState, controller: &str) -> (u64, JobContext, PathBuf) {
    let controller = *task_controller_by_id(controller).unwrap();
    let id = state.tasks.create_task_run(controller);
    let input = crate::frontend::entry_ref::primary_input(state).unwrap();
    bind_task_inputs(state, id, vec![input]).unwrap();
    let dir = ensure_task_run_dir(state, id, controller.kind, None).unwrap();
    let job = begin_job_execution(
        state,
        id,
        crate::backend::run_attempt::Placement::Local,
        None,
    );
    (
        id,
        JobContext {
            job_id: Some(job),
            task_run_id: Some(id),
        },
        dir,
    )
}

#[test]
fn qm_artifacts_follow_bound_identity_for_every_outcome_kind() {
    for controller in ["qm-energy", "qm-optimize", "qm-frequencies"] {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let a = add(&mut state, "A");
        state.entries.bump_active_revision();
        let (task, cx, dir) = prepare(&mut state, controller);
        let b = add(&mut state, "B");
        let other = state
            .tasks
            .create_task_run(*task_controller_by_id("qm-energy").unwrap());
        state.active_task_run = Some(other);
        state.tasks.open_panel(other);
        let optimized = controller == "qm-optimize";
        let outcome = QmOutcome {
            energy_hartree: -1.0,
            converged: true,
            optimized_structure: optimized.then(|| Structure::new("optimized A", Vec::new())),
            summary: "report for A".into(),
            scf_trace: vec![-0.5, -1.0],
            opt_trace: Vec::new(),
            frequencies: vec![42.0],
        };
        apply_qm_outcome(&mut state, &cx, outcome);
        let run = state.tasks.task_run(task).unwrap();
        assert_eq!(run.source_entry_id, Some(a));
        assert_eq!(run.inputs.as_ref().unwrap()[0].revision, 1);
        assert_eq!(run.run_dir.as_ref(), Some(&dir));
        assert_eq!(run.result_entry_id.is_some(), optimized);
        assert_ne!(run.result_entry_id, Some(b));
        assert_eq!(state.active_task_run, Some(other));
        assert!(state.tasks.task_run(other).unwrap().run_dir.is_none());
        assert!(
            std::fs::read_to_string(dir.join(QM_OUTPUT_FILE))
                .unwrap()
                .contains("report for A")
        );
        let series =
            crate::backend::runs::load_qm_series_file(&dir.join(crate::backend::runs::SERIES_FILE))
                .unwrap();
        assert_eq!(series.scf_trace, vec![-0.5, -1.0]);
        assert!(
            state
                .materializations
                .contains(&cx.job_id.unwrap().to_string())
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn md_result_and_ledger_follow_job_after_active_panel_switch() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let a = add(&mut state, "A");
    let (task, cx, dir) = prepare(&mut state, "run-md");
    add(&mut state, "B");
    let other = state
        .tasks
        .create_task_run(*task_controller_by_id("run-md").unwrap());
    state.active_task_run = Some(other);
    let outcome = || crate::frontend::jobs::EngineSuccess {
        engine: "GROMACS",
        job_kind: "run-md",
        structure: Structure::new("MD A", Vec::new()),
        summary: "done".into(),
        working_dir: dir.clone(),
        trajectory: Some(dir.join("production.xtc")),
    };
    apply_engine_outcome(&mut state, &cx, outcome());
    let count = state.entries.records.len();
    apply_engine_outcome(&mut state, &cx, outcome());
    assert_eq!(state.entries.records.len(), count);
    assert_eq!(state.active_task_run, Some(other));
    let run = state.tasks.task_run(task).unwrap();
    assert_eq!(run.source_entry_id, Some(a));
    assert!(run.result_entry_id.is_some());
    assert!(
        state
            .materializations
            .contains(&cx.job_id.unwrap().to_string())
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn docking_saves_all_poses_and_best_result_once_for_explicit_inputs() {
    use crate::engines::docking::{DockedPose, DockingOutcome};
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let receptor = add(&mut state, "receptor");
    let ligand = add(&mut state, "ligand");
    add(&mut state, "unrelated");
    let controller = *task_controller_by_id("dock-ligand").unwrap();
    let task = state.tasks.create_task_run(controller);
    let inputs = [("receptor", receptor), ("ligand", ligand)]
        .into_iter()
        .map(|(role, id)| crate::frontend::entry_ref::input_reference(&state, role, id).unwrap())
        .collect();
    bind_task_inputs(&mut state, task, inputs).unwrap();
    let dir = ensure_task_run_dir(&mut state, task, controller.kind, None).unwrap();
    let job = begin_job_execution(
        &mut state,
        task,
        crate::backend::run_attempt::Placement::Local,
        None,
    );
    let cx = JobContext {
        job_id: Some(job),
        task_run_id: Some(task),
    };
    let other = state.tasks.create_task_run(controller);
    state.active_task_run = Some(other);
    let outcome = || DockingOutcome {
        poses: (0..3)
            .map(|i| DockedPose {
                affinity: -8.0 + i as f64,
                intermolecular: 0.0,
                internal: 0.0,
                torsional: 0.0,
                structure: Structure::new(format!("pose {i}"), Vec::new()),
                pdbqt: format!("REMARK pose {i}\n"),
            })
            .collect(),
        notes: Vec::new(),
        summary: "poses".into(),
    };
    apply_docking_outcome(&mut state, &cx, outcome());
    apply_docking_outcome(&mut state, &cx, outcome());
    assert_eq!(state.entries.records.len(), 6);
    let run = state.tasks.task_run(task).unwrap();
    assert_eq!(run.source_entry_id, Some(ligand));
    assert_eq!(run.inputs.as_ref().unwrap()[0].entry_id, receptor);
    let ledger = state.materializations.get(&job.to_string()).unwrap();
    assert_eq!(ledger.entries.len(), 3);
    assert_eq!(run.result_entry_id, ledger.primary_entry_id);
    let poses = std::fs::read_to_string(dir.join(DOCK_POSES_FILE)).unwrap();
    assert_eq!(poses.matches("ENDMDL").count(), 3);
    assert!(state.tasks.task_run(other).unwrap().run_dir.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn missing_identity_does_not_import_into_active_task() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    add(&mut state, "A");
    let (task, _, dir) = prepare(&mut state, "qm-optimize");
    state.active_task_run = Some(task);
    let cx = JobContext {
        job_id: None,
        task_run_id: None,
    };
    apply_qm_outcome(
        &mut state,
        &cx,
        QmOutcome {
            energy_hartree: -1.0,
            converged: true,
            optimized_structure: Some(Structure::empty()),
            summary: "unowned".into(),
            scf_trace: Vec::new(),
            opt_trace: Vec::new(),
            frequencies: Vec::new(),
        },
    );
    assert_eq!(state.entries.records.len(), 1);
    assert!(!dir.join(QM_OUTPUT_FILE).exists());
    assert!(
        state
            .tasks
            .task_run(task)
            .unwrap()
            .result_entry_id
            .is_none()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn successful_local_launches_dismiss_forms_and_restore_submitted_parameters_for_rerun() {
    use crate::frontend::state::SubmittedComputePrompt;
    for controller in ["qm-energy", "dock-ligand", "run-md"] {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let element = if controller == "run-md" { "Ar" } else { "He" };
        let structure = Structure::with_cell(
            "input",
            vec![crate::domain::Atom {
                element: element.into(),
                position: nalgebra::Point3::origin(),
                charge: 0.0,
            }],
            crate::domain::UnitCell::from_parameters(30.0, 30.0, 30.0, 90.0, 90.0, 90.0),
        );
        let first = state
            .entries
            .add_entry(structure.clone(), None, PathBuf::from("first.xyz"));
        let second = state
            .entries
            .add_entry(structure, None, PathBuf::from("second.xyz"));
        let task = state
            .tasks
            .create_task_run(*task_controller_by_id(controller).unwrap());
        state.tasks.open_panel(task);
        ensure_panel_form(&mut state, task);
        match controller {
            "qm-energy" => {
                let prompt = state.ui.pending_qm.as_mut().unwrap();
                prompt.method = crate::engines::qm::QmMethod::Rhf;
                prompt.basis = "sto-3g".into();
                start_pending_qm(&mut state);
                assert!(state.jobs.qm_running());
                assert!(state.ui.pending_qm.is_none());
            }
            "dock-ligand" => {
                let prompt = state.ui.pending_docking.as_mut().unwrap();
                prompt.receptor_entry = Some(first);
                prompt.ligand_entry = Some(second);
                prompt.score_only = true;
                prompt.seed = 42;
                start_pending_docking(&mut state);
                assert!(state.jobs.docking_running());
                assert!(state.ui.pending_docking.is_none());
            }
            _ => {
                state.config.engine_overrides.insert(
                    crate::engines::registry::EngineId::GROMACS,
                    crate::engines::registry::EngineLaunch::native("missing-test-gmx"),
                );
                state.ui.pending_md_run.as_mut().unwrap().run_name = "submitted-md".into();
                start_pending_md_run(&mut state);
                assert!(state.jobs.engine_running());
                assert!(state.ui.pending_md_run.is_none());
            }
        }
        assert!(state.ui.submitted_compute_prompts.contains_key(&task));
        ensure_panel_form(&mut state, task);
        assert!(state.ui.pending_qm.is_none());
        assert!(state.ui.pending_docking.is_none());
        assert!(state.ui.pending_md_run.is_none());
        state.cancel_transient_jobs();
        state.tasks.mark_status(task, TaskStatus::Completed);
        ensure_panel_form(&mut state, task);
        match &state.ui.submitted_compute_prompts[&task] {
            SubmittedComputePrompt::Qm(_) => {
                assert_eq!(state.ui.pending_qm.as_ref().unwrap().basis, "sto-3g")
            }
            SubmittedComputePrompt::Docking(_) => {
                assert_eq!(state.ui.pending_docking.as_ref().unwrap().seed, 42)
            }
            SubmittedComputePrompt::Md(_) => assert_eq!(
                state.ui.pending_md_run.as_ref().unwrap().run_name,
                "submitted-md"
            ),
        }
    }
}
