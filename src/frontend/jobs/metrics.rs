use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::manager::JobManager;

/// One utilization sample: global CPU load, memory load, plus a live per-GPU
/// snapshot (one entry per GPU the sampler could read). `gpus` is empty when no
/// live backend is available — the common case, since live GPU stats need the
/// optional NVML feature; the gauges then read N/A.
pub struct Metrics {
    pub cpu_pct: f32,
    pub mem_pct: Option<f32>,
    pub gpus: Vec<crate::frontend::gpu_monitor::GpuSample>,
}

/// Live cadence control shared with the sampler thread. The UI updates the
/// desired per-sample `interval` (or `None` to suspend) as the refresh-rate
/// setting and window visibility change; the thread waits on `cv` so a change
/// applies promptly and a suspended sampler parks with no wakeups at all.
struct MetricsControl {
    inner: Mutex<ControlState>,
    cv: Condvar,
}

struct ControlState {
    /// Desired per-sample interval; `None` suspends sampling (and releases the
    /// GPU probe) until an interval is set again.
    interval: Option<Duration>,
    /// Bumped on every change so a sampler waiting out an interval wakes to
    /// re-read it, distinguishing a real change from a spurious wakeup.
    generation: u64,
    /// Set when the handle is dropped, so a parked sampler can exit.
    stop: bool,
}

/// GPU sampling floor: a discrete card is polled no more often than this even at
/// the highest CPU/memory rate, since each poll can pull it out of its deepest
/// power state. CPU/memory (cheap, no device wake) keep the chosen rate.
pub(crate) const GPU_MIN_INTERVAL: Duration = Duration::from_secs(2);
/// Stretched GPU interval once the card last read idle: poke a quiescent card
/// even less often. (Live telemetry can't be read without resuming the device,
/// so the win is in how rarely we resume it, not in skipping a single read.)
pub(crate) const GPU_IDLE_INTERVAL: Duration = Duration::from_secs(8);

/// The per-sample interval for a refresh-rate setting, or `None` for `Pause`.
pub fn refresh_interval(refresh: crate::backend::config::MonitorRefresh) -> Option<Duration> {
    use crate::backend::config::MonitorRefresh;
    match refresh {
        MonitorRefresh::High => Some(Duration::from_millis(500)),
        MonitorRefresh::Standard => Some(Duration::from_millis(1000)),
        MonitorRefresh::Low => Some(Duration::from_secs(4)),
        MonitorRefresh::Pause => None,
    }
}

/// How long to leave the GPU alone before the next probe, given the base
/// CPU/memory cadence and the last readings: at least [`GPU_MIN_INTERVAL`], and
/// [`GPU_IDLE_INTERVAL`] once every reporting card last read idle.
pub(crate) fn gpu_interval(
    base: Duration,
    last: &[crate::frontend::gpu_monitor::GpuSample],
) -> Duration {
    let idle = !last.is_empty() && last.iter().all(|s| s.util_pct.is_none_or(|u| u <= 1.0));
    base.max(if idle {
        GPU_IDLE_INTERVAL
    } else {
        GPU_MIN_INTERVAL
    })
}

/// Handle to the live utilization sampler. Dropping it ends the thread (even
/// when parked): [`Drop`] signals `stop` and wakes it.
pub struct RunningMetricsSampler {
    pub receiver: std::sync::mpsc::Receiver<Metrics>,
    control: Arc<MetricsControl>,
}

impl RunningMetricsSampler {
    /// Set the live sampling cadence; `None` suspends sampling (and releases the
    /// GPU probe) until an interval is set again. Cheap and idempotent — a no-op
    /// when unchanged, so it is safe to call every frame.
    pub fn set_interval(&self, interval: Option<Duration>) {
        let mut state = self.control.inner.lock().unwrap();
        if state.interval == interval {
            return;
        }
        state.interval = interval;
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        self.control.cv.notify_all();
    }
}

#[cfg(test)]
impl RunningMetricsSampler {
    /// Wrap a pre-seeded receiver with an inert control, for tests that inject
    /// samples directly instead of running the background thread.
    pub(crate) fn for_test(receiver: std::sync::mpsc::Receiver<Metrics>) -> Self {
        Self {
            receiver,
            control: Arc::new(MetricsControl {
                inner: Mutex::new(ControlState {
                    interval: None,
                    generation: 0,
                    stop: false,
                }),
                cv: Condvar::new(),
            }),
        }
    }
}

impl Drop for RunningMetricsSampler {
    fn drop(&mut self) {
        let mut state = self.control.inner.lock().unwrap();
        state.stop = true;
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        self.control.cv.notify_all();
    }
}

/// Spawn the CPU/GPU sampler at `initial_interval` (`None` starts it parked).
/// The first CPU reading is meaningless (needs two refreshes >=
/// MINIMUM_CPU_UPDATE_INTERVAL apart), so a fresh start primes once and waits a
/// beat before its first emitted sample. The GPU probe is built once on the
/// thread and sampled on its own (longer) cadence — and released entirely while
/// the sampler is suspended — to wake a discrete card as rarely as possible.
pub fn spawn_metrics_sampler(initial_interval: Option<Duration>) -> RunningMetricsSampler {
    let (sender, receiver) = std::sync::mpsc::channel();
    let control = Arc::new(MetricsControl {
        inner: Mutex::new(ControlState {
            interval: initial_interval,
            generation: 0,
            stop: false,
        }),
        cv: Condvar::new(),
    });
    let thread_control = Arc::clone(&control);
    std::thread::spawn(move || {
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_usage();
        let mut gpu_sampler = crate::frontend::gpu_monitor::GpuSampler::new();
        // Last GPU readings, reused on ticks where the card is intentionally not
        // re-probed so the gauges hold steady between its (sparser) samples.
        let mut gpus: Vec<crate::frontend::gpu_monitor::GpuSample> = Vec::new();
        let mut last_gpu: Option<Instant> = None;
        // Whether the CPU baseline is current. Cleared on suspend (the baseline
        // goes stale while parked) so we re-prime before the next real sample.
        let mut primed = false;

        // Wait out `dur`, returning early on a cadence change or shutdown.
        // Returns `true` when the handle was dropped (time to exit).
        let wait_tick = |dur: Duration| -> bool {
            let guard = thread_control.inner.lock().unwrap();
            let generation = guard.generation;
            let (guard, _) = thread_control
                .cv
                .wait_timeout_while(guard, dur, |s| s.generation == generation && !s.stop)
                .unwrap();
            guard.stop
        };

        loop {
            // Resolve the current cadence, parking (and releasing the GPU probe)
            // while suspended. Returns the active per-sample interval.
            let interval = {
                let mut guard = thread_control.inner.lock().unwrap();
                if guard.interval.is_none() && !guard.stop {
                    gpu_sampler.suspend();
                    last_gpu = None;
                    primed = false;
                    guard = thread_control
                        .cv
                        .wait_while(guard, |s| s.interval.is_none() && !s.stop)
                        .unwrap();
                }
                if guard.stop {
                    break;
                }
                guard
                    .interval
                    .expect("interval is Some once the park loop exits")
            };

            // Cold start (or first sample after a suspension): establish a CPU
            // baseline, wait one beat so the next refresh has a real delta, then
            // loop back to take the actual sample.
            if !primed {
                sys.refresh_cpu_usage();
                primed = true;
                if wait_tick(interval) {
                    break;
                }
                continue;
            }

            sys.refresh_cpu_usage();
            sys.refresh_memory();
            let total = sys.total_memory();
            let mem_pct =
                (total > 0).then(|| (sys.used_memory() as f64 / total as f64 * 100.0) as f32);

            // Probe the GPU only when its (longer) interval has elapsed;
            // otherwise reuse the last readings.
            if last_gpu.is_none_or(|t| t.elapsed() >= gpu_interval(interval, &gpus)) {
                gpus = gpu_sampler.sample();
                last_gpu = Some(Instant::now());
            }

            let sample = Metrics {
                cpu_pct: sys.global_cpu_usage(),
                mem_pct,
                gpus: gpus.clone(),
            };
            if sender.send(sample).is_err() {
                break; // receiver dropped (app closing)
            }

            if wait_tick(interval) {
                break;
            }
        }
    });
    RunningMetricsSampler { receiver, control }
}

/// Start or stop the live metrics sampler on `jobs` to match `on`. Idempotent:
/// turning on when already running does not spawn a second sampler. Separated
/// from the settings handler so the lifecycle is testable without touching disk.
/// `initial_interval` seeds the cadence when a sampler is spawned (`None` starts
/// it parked); the UI then drives it live via [`RunningMetricsSampler::set_interval`].
pub(crate) fn apply_metrics_sampler(
    jobs: &mut JobManager,
    on: bool,
    initial_interval: Option<Duration>,
) {
    if on {
        if jobs.metrics.is_none() {
            jobs.metrics = Some(spawn_metrics_sampler(initial_interval));
        }
    } else {
        jobs.metrics = None;
    }
}
