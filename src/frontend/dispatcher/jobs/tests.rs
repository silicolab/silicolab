use super::*;
use crate::frontend::gpu_monitor::GpuSample;
use crate::frontend::jobs::{Metrics, QmWorkerMessage, RunningMetricsSampler, RunningQmJob};

#[test]
fn poll_remote_gpu_monitor_drains_sample_into_state() {
    use crate::engines::remote::hardware::RemoteGpuStat;
    use crate::frontend::jobs::RunningRemoteGpuMonitor;
    use crate::frontend::state::RemoteGpuLive;

    let mut state = AppState::scratch(Default::default(), Vec::new());
    state.ui.settings.remote_gpu_live = Some(RemoteGpuLive {
        host_id: "h".into(),
        gpus: Vec::new(),
        last_error: None,
    });
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Ok(vec![RemoteGpuStat {
        index: 0,
        name: "GPU A".into(),
        util_pct: Some(33.0),
        vram_used_mib: Some(512),
        vram_total_mib: Some(8192),
        temp_c: Some(45),
        power_w: Some(60.0),
    }]))
    .unwrap();
    state.jobs.remote_gpu_monitor = Some(RunningRemoteGpuMonitor {
        host_id: "h".into(),
        receiver: rx,
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });

    let ctx = egui::Context::default();
    poll_remote_gpu_monitor(&mut state, &ctx);

    let live = state.ui.settings.remote_gpu_live.as_ref().unwrap();
    assert_eq!(live.gpus.len(), 1);
    assert_eq!(live.gpus[0].latest.util_pct, Some(33.0));
    assert_eq!(live.gpus[0].util_history.back().copied(), Some(Some(33.0)));
    assert!(live.last_error.is_none());
}

#[test]
fn poll_remote_gpu_monitor_drains_all_queued_and_clears_handle_on_disconnect() {
    use crate::engines::remote::hardware::RemoteGpuStat;
    use crate::frontend::jobs::RunningRemoteGpuMonitor;
    use crate::frontend::state::RemoteGpuLive;

    let mut state = AppState::scratch(Default::default(), Vec::new());
    state.ui.settings.remote_gpu_live = Some(RemoteGpuLive {
        host_id: "h".into(),
        gpus: Vec::new(),
        last_error: None,
    });
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Ok(vec![RemoteGpuStat {
        index: 0,
        name: "GPU A".into(),
        util_pct: Some(10.0),
        vram_used_mib: None,
        vram_total_mib: None,
        temp_c: None,
        power_w: None,
    }]))
    .unwrap();
    tx.send(Ok(vec![RemoteGpuStat {
        index: 0,
        name: "GPU A".into(),
        util_pct: Some(80.0),
        vram_used_mib: None,
        vram_total_mib: None,
        temp_c: None,
        power_w: None,
    }]))
    .unwrap();
    drop(tx);
    state.jobs.remote_gpu_monitor = Some(RunningRemoteGpuMonitor {
        host_id: "h".into(),
        receiver: rx,
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });

    let ctx = egui::Context::default();
    poll_remote_gpu_monitor(&mut state, &ctx);

    {
        let live = state.ui.settings.remote_gpu_live.as_ref().unwrap();
        assert_eq!(live.gpus[0].latest.util_pct, Some(80.0));
        assert_eq!(live.gpus[0].util_history.len(), 2);
    }
    assert!(state.jobs.remote_gpu_monitor.is_none());
}

#[test]
fn poll_metrics_drains_latest_into_state() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Metrics {
        cpu_pct: 42.0,
        mem_pct: Some(55.0),
        gpus: vec![GpuSample {
            pci_bus_id: "01:00.0".into(),
            util_pct: Some(50.0),
            vram_used_bytes: None,
            vram_total_bytes: None,
            temp_c: None,
        }],
    })
    .unwrap();
    state.jobs.metrics = Some(RunningMetricsSampler::for_test(rx));
    let ctx = egui::Context::default();
    poll_metrics(&mut state, &ctx);
    assert_eq!(state.ui.cpu_pct, 42.0);
    assert_eq!(state.ui.mem_pct, Some(55.0));
    assert_eq!(state.ui.gpus.len(), 1);
    assert_eq!(state.ui.gpus[0].util_pct, Some(50.0));
    // The GPU's util is recorded in its own per-card sparkline history.
    assert_eq!(
        state
            .ui
            .monitor_history
            .gpus
            .get("01:00.0")
            .and_then(|h| h.back().copied()),
        Some(Some(50.0))
    );
}

#[test]
fn esc_still_requests_qm_cancel_with_stage_boundary_message() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let (_tx, rx) = std::sync::mpsc::channel::<QmWorkerMessage>();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    state.jobs.qm = Some(RunningQmJob {
        cancel: crate::wire::JobCancelHandle::from_flag(std::sync::Arc::clone(&cancel)),
        receiver: rx,
        latest_stage: Some("SCF".into()),
        cancel_requested: false,
    });
    let ctx = egui::Context::default();
    ctx.input_mut(|input| {
        input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
    });

    poll_qm_job(&mut state, &ctx);

    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(state.message, "QM calculation stopping");
    assert!(state.jobs.qm.is_some());
}

#[test]
fn save_qm_run_artifacts_writes_report_and_series_into_the_run_dir() {
    use crate::backend::runs::{SERIES_FILE, load_qm_series_file};
    use crate::backend::tasks::task_controller_by_id;

    let mut state = AppState::scratch(Default::default(), Vec::new());
    let controller = *task_controller_by_id("qm-energy").expect("qm-energy controller");
    let task_id = state.tasks.create_task_run(controller);
    state.active_task_run = Some(task_id);

    let outcome = crate::engines::qm::QmOutcome {
        energy_hartree: -74.96,
        converged: true,
        optimized_structure: None,
        summary: "E = -74.96 Eh".to_string(),
        scf_trace: vec![-74.1, -74.96],
        opt_trace: Vec::new(),
        frequencies: Vec::new(),
    };
    save_qm_run_artifacts(&mut state, &outcome);

    let run_dir = state
        .tasks
        .task_run(task_id)
        .and_then(|task| task.run_dir.clone())
        .expect("run dir created on demand");
    let series = load_qm_series_file(&run_dir.join(SERIES_FILE)).expect("series saved");
    assert_eq!(series.scf_trace, vec![-74.1, -74.96]);
    let report = std::fs::read_to_string(run_dir.join(QM_OUTPUT_FILE)).expect("report saved");
    assert!(report.starts_with("E = -74.96 Eh"));

    // An outcome with no plottable trace (e.g. a frequency-only run) still writes
    // its report, but no series file for a chart that would have no data.
    let controller = *task_controller_by_id("qm-energy").unwrap();
    let empty_task = state.tasks.create_task_run(controller);
    state.active_task_run = Some(empty_task);
    let empty = crate::engines::qm::QmOutcome {
        scf_trace: Vec::new(),
        opt_trace: Vec::new(),
        frequencies: Vec::new(),
        ..outcome
    };
    save_qm_run_artifacts(&mut state, &empty);
    let empty_dir = state
        .tasks
        .task_run(empty_task)
        .and_then(|task| task.run_dir.clone())
        .expect("run dir created on demand");
    assert!(empty_dir.join(QM_OUTPUT_FILE).is_file());
    assert!(!empty_dir.join(SERIES_FILE).exists());
}

/// Reproduces the shape of a real single-point run as it is persisted: a
/// completed `qm-energy` task run whose `source_entry_id` is the input structure
/// and whose run directory holds the report and the series. Both entry-list chips
/// and the report viewer must resolve through it.
#[test]
fn single_point_entry_surfaces_its_report_and_chart() {
    use crate::backend::runs::{QmSeries, save_qm_series_file};
    use crate::backend::tasks::{TaskStatus, task_controller_by_id};

    let mut state = AppState::scratch(Default::default(), Vec::new());
    let entry_id = state
        .entries
        .add_entry(crate::domain::Structure::empty(), None, PathBuf::new());

    // A structure nothing has been computed on carries no QM chip.
    assert!(entry_qm_run_dir(&state, entry_id).is_none());
    assert!(!entry_chart_available(&mut state, entry_id));

    let run_dir = std::env::temp_dir().join("silicolab-single-point-surface");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(run_dir.join(QM_OUTPUT_FILE), "total energy: -232.337 Eh\n").unwrap();
    let outcome = crate::engines::qm::QmOutcome {
        energy_hartree: -232.337,
        converged: true,
        optimized_structure: None,
        summary: "total energy: -232.337 Eh".to_string(),
        scf_trace: vec![-233.6, -232.337],
        opt_trace: Vec::new(),
        frequencies: Vec::new(),
    };
    save_qm_series_file(&run_dir, &QmSeries::from_outcome(&outcome)).unwrap();

    let controller = *task_controller_by_id("qm-energy").expect("qm-energy controller");
    let task_id = state.tasks.create_task_run(controller);
    state.tasks.set_source_entry_id(task_id, Some(entry_id));
    state.tasks.set_run_dir(task_id, run_dir.clone());
    state.tasks.mark_status(task_id, TaskStatus::Completed);

    // `entry_chart_available` memoizes per entry, and the entry was rendered as
    // chartless before this run existed. Completing a QM run therefore has to
    // invalidate that memo — every QM completion handler does exactly this, and
    // omitting it leaves the finished run's chart chip permanently hidden.
    assert!(
        !entry_chart_available(&mut state, entry_id),
        "a stale memo must survive until it is explicitly cleared"
    );
    state.ui.chart_availability.clear();

    assert_eq!(
        entry_qm_run_dir(&state, entry_id).as_deref(),
        Some(&*run_dir)
    );
    assert!(
        entry_chart_available(&mut state, entry_id),
        "the input structure's chart chip must find the run's series.json"
    );

    show_qm_output(&mut state, entry_id);
    let viewer = state.ui.text_viewer.as_ref().expect("report opened");
    assert!(viewer.text.contains("-232.337 Eh"));

    let _ = std::fs::remove_dir_all(&run_dir);
}

#[test]
fn single_point_run_anchors_its_report_to_the_input_entry() {
    use crate::backend::tasks::{TaskStatus, task_controller_by_id};

    // The regression this whole path exists for: a single-point energy produces no
    // new entry, so its results must be reachable from the structure it was
    // computed from rather than being stranded in the run directory.
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let entry_id = state
        .entries
        .add_entry(crate::domain::Structure::empty(), None, PathBuf::new());

    let controller = *task_controller_by_id("qm-energy").expect("qm-energy controller");
    let task_id = state.tasks.create_task_run(controller);
    state.tasks.set_source_entry_id(task_id, Some(entry_id));
    state
        .tasks
        .set_run_dir(task_id, std::env::temp_dir().join("silicolab-anchor-test"));

    // Only a *completed* run surfaces results.
    assert!(state.tasks.latest_qm_run_for_entry(entry_id).is_none());
    state.tasks.mark_status(task_id, TaskStatus::Completed);
    assert_eq!(
        state
            .tasks
            .latest_qm_run_for_entry(entry_id)
            .map(|task| task.id),
        Some(task_id)
    );

    // An optimization anchors to the geometry it produced, not to its input, so
    // the input structure is not claimed by a run whose result lives elsewhere.
    let optimize = *task_controller_by_id("qm-optimize").expect("qm-optimize controller");
    let opt_task = state.tasks.create_task_run(optimize);
    let result_id =
        state
            .entries
            .add_entry(crate::domain::Structure::empty(), None, PathBuf::new());
    state.tasks.set_source_entry_id(opt_task, Some(entry_id));
    state.tasks.set_result_entry_id(opt_task, Some(result_id));
    state.tasks.set_run_dir(
        opt_task,
        std::env::temp_dir().join("silicolab-anchor-test-2"),
    );
    state.tasks.mark_status(opt_task, TaskStatus::Completed);

    assert_eq!(
        state
            .tasks
            .latest_qm_run_for_entry(result_id)
            .map(|task| task.id),
        Some(opt_task)
    );
    assert_eq!(
        state
            .tasks
            .latest_qm_run_for_entry(entry_id)
            .map(|task| task.id),
        Some(task_id),
        "the optimize run must not steal its input entry's single-point result"
    );
}

#[test]
fn optimization_progress_accumulates_the_energy_trace() {
    use crate::engines::forcefield::OptimizationReport;
    use crate::frontend::jobs::{OptimizationWorkerMessage, RunningOptimization};

    let mut state = AppState::scratch(Default::default(), Vec::new());
    let (tx, rx) = std::sync::mpsc::channel();
    let report = |steps: usize, energy: f32| OptimizationReport {
        initial_energy: 10.0,
        final_energy: energy,
        steps,
        converged: false,
        stopped: false,
        timed_out: false,
    };
    tx.send(OptimizationWorkerMessage::Progress {
        structure: crate::domain::Structure::empty(),
        report: report(5, 4.0),
    })
    .unwrap();
    tx.send(OptimizationWorkerMessage::Progress {
        structure: crate::domain::Structure::empty(),
        report: report(10, 2.5),
    })
    .unwrap();
    state.jobs.set_optimizer(RunningOptimization {
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        receiver: rx,
        latest_report: None,
        energy_trace: Vec::new(),
    });

    let ctx = egui::Context::default();
    poll_optimization_job(&mut state, &ctx);

    let running = state.jobs.take_optimizer().expect("job still running");
    assert_eq!(
        running.energy_trace,
        vec![[0.0, 10.0], [5.0, 4.0], [10.0, 2.5]],
        "first progress seeds the initial energy at step 0"
    );
}
