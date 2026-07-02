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
