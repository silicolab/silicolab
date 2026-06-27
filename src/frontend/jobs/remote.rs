use std::sync::{Arc, atomic::AtomicBool, mpsc::Receiver};

/// What a Remote Hosts settings probe is checking on a host.
#[derive(Debug, Clone, Copy)]
pub enum RemoteProbeKind {
    /// Whether passwordless key login already works.
    Passwordless,
    /// Detect a GROMACS executable + version on the host.
    DetectGromacs,
}

/// Result of a remote-host probe (sent back from the worker thread).
pub enum RemoteProbeOutcome {
    Passwordless(bool),
    /// `(program, version)` when GROMACS was found, else `None`.
    Detected(Option<(String, String)>),
}

/// An in-flight Remote Hosts probe. Runs the blocking `ssh` off the UI thread so
/// a slow or dead host never freezes rendering; the dispatcher drains it each
/// frame (like [`RunningUpdateCheck`]).
pub struct RunningRemoteProbe {
    pub host_id: String,
    pub receiver: Receiver<RemoteProbeOutcome>,
}

/// Spawn a remote-host probe on a worker thread. The host is cloned in; only its
/// connection fields matter (the probe uses a throwaway run anchor).
pub fn spawn_remote_probe(
    host: crate::backend::config::RemoteHost,
    kind: RemoteProbeKind,
) -> RunningRemoteProbe {
    use crate::engines::remote;
    let (sender, receiver) = std::sync::mpsc::channel();
    let host_id = host.id.clone();
    std::thread::spawn(move || {
        let target = remote::RemoteTarget::for_run(&host, "probe");
        let outcome = match kind {
            RemoteProbeKind::Passwordless => {
                RemoteProbeOutcome::Passwordless(remote::check_passwordless(&target))
            }
            RemoteProbeKind::DetectGromacs => RemoteProbeOutcome::Detected(
                remote::detect_remote_engine(&target, remote::GMX_REMOTE_CANDIDATES),
            ),
        };
        let _ = sender.send(outcome);
    });
    RunningRemoteProbe { host_id, receiver }
}

/// Result of a remote hardware inventory probe (sent back from the worker thread).
pub enum RemoteHardwareOutcome {
    Ok(crate::engines::remote::hardware::RemoteHardwareInfo),
    Failed(String),
}

/// An in-flight remote hardware inventory probe. Like [`RunningRemoteProbe`], the
/// blocking SSH runs off the UI thread and the dispatcher drains it each frame.
pub struct RunningRemoteHardwareFetch {
    pub host_id: String,
    pub receiver: Receiver<RemoteHardwareOutcome>,
}

/// Spawn a remote hardware probe on a worker thread: run the aggregate inventory
/// script over SSH and parse it. The host is cloned in (only its connection
/// fields matter; the probe uses a throwaway run anchor).
pub fn spawn_remote_hardware_fetch(
    host: crate::backend::config::RemoteHost,
) -> RunningRemoteHardwareFetch {
    use crate::engines::remote::{self, hardware};
    use std::time::Duration;
    let (sender, receiver) = std::sync::mpsc::channel();
    let host_id = host.id.clone();
    std::thread::spawn(move || {
        let target = remote::RemoteTarget::for_run(&host, "probe");
        let outcome = match remote::run_probe_command(
            &target,
            hardware::PROBE_SCRIPT,
            Duration::from_secs(30),
        ) {
            Ok(stdout) => RemoteHardwareOutcome::Ok(hardware::parse_remote_hardware(&stdout)),
            Err(error) => RemoteHardwareOutcome::Failed(error.to_string()),
        };
        let _ = sender.send(outcome);
    });
    RunningRemoteHardwareFetch { host_id, receiver }
}

/// Handle to a live remote-GPU sampler. `cancel()` ends the loop within ~250 ms;
/// dropping the handle also ends it (the next `send` fails once the receiver is
/// gone). `cancel` is `pub(crate)` so dispatcher tests can build a handle.
pub struct RunningRemoteGpuMonitor {
    pub host_id: String,
    pub receiver: Receiver<Result<Vec<crate::engines::remote::hardware::RemoteGpuStat>, String>>,
    pub(crate) cancel: Arc<AtomicBool>,
}

impl RunningRemoteGpuMonitor {
    /// Signal the sampler thread to stop before its next poll.
    pub fn cancel(&self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Spawn a recurring remote-GPU sampler: every `interval`, SSH-run the nvidia-smi
/// stats query and parse it, sending each `Result` back. The first sample fires
/// immediately. The loop exits when `cancel` is set or the receiver is dropped.
pub fn spawn_remote_gpu_monitor(
    host: crate::backend::config::RemoteHost,
    interval: std::time::Duration,
) -> RunningRemoteGpuMonitor {
    use crate::engines::remote::{self, hardware};
    use std::time::Duration;
    let (sender, receiver) = std::sync::mpsc::channel();
    let host_id = host.id.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let thread_cancel = cancel.clone();
    std::thread::spawn(move || {
        let target = remote::RemoteTarget::for_run(&host, "gpu-monitor");
        loop {
            let msg = match remote::run_probe_command(
                &target,
                hardware::GPU_STATS_SCRIPT,
                Duration::from_secs(15),
            ) {
                Ok(stdout) => Ok(hardware::parse_remote_gpu_stats(&stdout)),
                Err(error) => Err(error.to_string()),
            };
            if sender.send(msg).is_err() {
                break; // receiver dropped (toggled off / app closing)
            }
            // Cancel-responsive sleep so cancel() takes effect within ~250 ms.
            let mut slept = Duration::ZERO;
            while slept < interval {
                if thread_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
                slept += Duration::from_millis(250);
            }
        }
    });
    RunningRemoteGpuMonitor {
        host_id,
        receiver,
        cancel,
    }
}
