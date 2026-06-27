use std::collections::BTreeMap;

/// Number of recent utilization samples kept for the monitor popover sparklines.
const MONITOR_HISTORY_LEN: usize = 120;

/// Rolling per-metric history (oldest at front) feeding the monitor sparklines.
/// Each is capped at [`MONITOR_HISTORY_LEN`]; `None` marks a sample with no
/// reading (e.g. GPU on a machine without one). GPUs are kept per card (keyed by
/// normalized PCI bus id) rather than averaged — multi-GPU hosts get one series
/// per card.
#[derive(Default, Clone)]
pub struct MonitorHistory {
    pub cpu: std::collections::VecDeque<Option<f32>>,
    pub mem: std::collections::VecDeque<Option<f32>>,
    pub gpus: BTreeMap<String, std::collections::VecDeque<Option<f32>>>,
}

impl MonitorHistory {
    /// Append one tick to every series. `gpus` is this tick's per-card
    /// `(bus_id, utilization)`; a card seen for the first time starts a series,
    /// and every known card's series advances (with `None` when that card had no
    /// reading this tick), so each series stays on the same time base.
    pub fn push(&mut self, cpu: f32, mem: Option<f32>, gpus: &[(String, Option<f32>)]) {
        push_capped(&mut self.cpu, Some(cpu));
        push_capped(&mut self.mem, mem);
        for (bus_id, _) in gpus {
            self.gpus.entry(bus_id.clone()).or_default();
        }
        for (bus_id, series) in self.gpus.iter_mut() {
            let util = gpus.iter().find(|(b, _)| b == bus_id).and_then(|(_, u)| *u);
            push_capped(series, util);
        }
    }
}

fn push_capped(buf: &mut std::collections::VecDeque<Option<f32>>, value: Option<f32>) {
    if buf.len() >= MONITOR_HISTORY_LEN {
        buf.pop_front();
    }
    buf.push_back(value);
}

/// Live remote-GPU monitoring data for the one host currently being watched.
#[derive(Debug, Default, Clone)]
pub struct RemoteGpuLive {
    pub host_id: String,
    pub gpus: Vec<RemoteGpuView>,
    /// Last transport error, kept while the sampler retries; cleared on a good sample.
    pub last_error: Option<String>,
}

/// One remote GPU's latest reading plus its utilization sparkline history.
#[derive(Debug, Clone)]
pub struct RemoteGpuView {
    pub index: u32,
    pub name: String,
    pub latest: crate::engines::remote::hardware::RemoteGpuStat,
    pub util_history: std::collections::VecDeque<Option<f32>>,
}

impl RemoteGpuLive {
    /// Apply one fresh batch of per-GPU stats: update each GPU's latest reading and
    /// push its utilization onto the (capped) sparkline history, creating a view for
    /// a newly-seen GPU and dropping views for GPUs no longer reported.
    pub fn apply(&mut self, stats: Vec<crate::engines::remote::hardware::RemoteGpuStat>) {
        self.gpus
            .retain(|v| stats.iter().any(|s| s.index == v.index));
        for stat in stats {
            match self.gpus.iter_mut().find(|v| v.index == stat.index) {
                Some(view) => {
                    push_capped(&mut view.util_history, stat.util_pct);
                    view.name = stat.name.clone();
                    view.latest = stat;
                }
                None => {
                    let mut util_history = std::collections::VecDeque::new();
                    push_capped(&mut util_history, stat.util_pct);
                    self.gpus.push(RemoteGpuView {
                        index: stat.index,
                        name: stat.name.clone(),
                        latest: stat,
                        util_history,
                    });
                }
            }
        }
        self.gpus.sort_by_key(|v| v.index);
        self.last_error = None;
    }
}
