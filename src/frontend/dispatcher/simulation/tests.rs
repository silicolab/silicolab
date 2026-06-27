use super::*;
use crate::frontend::jobs::RunningRemoteGpuMonitor;
use crate::frontend::state::{MonitorSource, RemoteGpuLive};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A running-monitor handle bound to `host_id`. We never read the receiver in
/// these tests, so a dropped sender is harmless; the returned `cancel` flag lets
/// the caller assert whether the sampler was told to stop.
fn running_monitor(host_id: &str) -> (RunningRemoteGpuMonitor, Arc<AtomicBool>) {
    let (_tx, rx) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    (
        RunningRemoteGpuMonitor {
            host_id: host_id.into(),
            receiver: rx,
            cancel: cancel.clone(),
        },
        cancel,
    )
}

fn live(host_id: &str) -> RemoteGpuLive {
    RemoteGpuLive {
        host_id: host_id.into(),
        gpus: Vec::new(),
        last_error: None,
    }
}

#[test]
fn switching_to_local_stops_the_running_monitor() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let (monitor, cancel) = running_monitor("a");
    state.jobs.remote_gpu_monitor = Some(monitor);
    state.ui.settings.remote_gpu_live = Some(live("a"));
    state.ui.layout.monitor_source = MonitorSource::Remote("a".into());

    set_monitor_source(&mut state, MonitorSource::Local);

    assert!(
        cancel.load(Ordering::Relaxed),
        "sampler should be cancelled"
    );
    assert!(state.jobs.remote_gpu_monitor.is_none());
    assert!(state.ui.settings.remote_gpu_live.is_none());
    assert_eq!(state.ui.layout.monitor_source, MonitorSource::Local);
}

#[test]
fn reselecting_the_same_host_is_idempotent() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let (monitor, cancel) = running_monitor("a");
    state.jobs.remote_gpu_monitor = Some(monitor);
    state.ui.layout.monitor_source = MonitorSource::Remote("a".into());

    set_monitor_source(&mut state, MonitorSource::Remote("a".into()));

    assert!(
        !cancel.load(Ordering::Relaxed),
        "an already-running host must not be restarted"
    );
    assert_eq!(
        state
            .jobs
            .remote_gpu_monitor
            .as_ref()
            .map(|m| m.host_id.as_str()),
        Some("a")
    );
    assert_eq!(
        state.ui.layout.monitor_source,
        MonitorSource::Remote("a".into())
    );
}

#[test]
fn switching_to_another_host_stops_the_previous_one() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let (monitor, cancel) = running_monitor("a");
    state.jobs.remote_gpu_monitor = Some(monitor);
    state.ui.settings.remote_gpu_live = Some(live("a"));
    state.ui.layout.monitor_source = MonitorSource::Remote("a".into());

    // Host "b" isn't in config, so no new sampler spawns regardless of whether
    // ssh is available in the test environment — but the previous "a" sampler
    // must always be stopped and the source must move to "b".
    set_monitor_source(&mut state, MonitorSource::Remote("b".into()));

    assert!(cancel.load(Ordering::Relaxed), "previous sampler cancelled");
    assert_ne!(
        state
            .jobs
            .remote_gpu_monitor
            .as_ref()
            .map(|m| m.host_id.clone()),
        Some("a".to_string()),
        "the old host's sampler handle must be gone"
    );
    assert_eq!(
        state.ui.layout.monitor_source,
        MonitorSource::Remote("b".into())
    );
}
