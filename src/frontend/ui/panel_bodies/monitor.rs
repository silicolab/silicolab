//! The system monitor: an always-visible, compact glance view plus a
//! click-to-open detail popover. The glance view is a stack of thin utilization
//! bars in the primary-sidebar footer, falling back to a one-line row of colored
//! chips in the status bar when the sidebar is hidden; both record their screen
//! rect in `monitor_anchor` so the popover knows where to open.
//!
//! Each GPU is shown on its own row (averaging utilization across cards would be
//! meaningless): a single card reads "GPU", several read "GPU0"/"GPU1"/… with the
//! full model name on hover. Rows come from the wgpu inventory (the same source
//! as the Compute Hardware panel), de-duplicated and minus the CPU/software
//! adapter, so every real GPU shows even when only some report live utilization
//! (e.g. NVML covers NVIDIA but not an Intel iGPU, which then reads N/A).

use std::collections::{BTreeSet, VecDeque};

use eframe::egui::{self, RichText, Sense};

use crate::backend::hardware::{GpuInfo, GpuKind};
use crate::frontend::actions::AppAction;
use crate::frontend::gpu_monitor;
use crate::frontend::state::{AppState, MonitorSource, RemoteGpuLive};
use crate::frontend::ui::gauge;

/// Bytes per GiB, for human-readable memory/VRAM figures.
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Shown while a freshly-selected remote host hasn't returned its first sample yet.
const REMOTE_WAITING: &str = "Waiting for first GPU sample…";
/// Shown while the SSH sampler is retrying after a transport error.
const REMOTE_RETRYING: &str = "Temporarily unreachable, retrying…";

/// One remote GPU's display data, the remote-host analogue of [`GpuRow`]. Sourced
/// from [`RemoteGpuLive`] (nvidia-smi over SSH) rather than the local sampler.
struct RemoteRow {
    /// Compact label: "GPU" for one card, else "GPU0"/"GPU1"/… (by position).
    short: String,
    /// Full model name, shown on hover.
    name: String,
    /// Live utilization percentage (0–100), `None` when the card didn't report it.
    util: Option<f32>,
    /// Optional second detail line: "VRAM u/t MiB  ·  T °C  ·  P W" (parts the host
    /// didn't report are dropped; `None` when none are present). The utilization `%`
    /// is carried by [`RemoteRow::util`] and shown on the headline line instead.
    detail: Option<String>,
    /// Utilization sparkline history (oldest at front).
    history: VecDeque<Option<f32>>,
}

/// Build the display rows for the selected remote host, plus an optional status
/// line. `live` is the running monitor's state; `host_id` is the source the dock is
/// pointed at. A `None`/mismatched `live` (sampler just (re)started) or an empty
/// reading reads as "waiting"; a transport error reads as "retrying" while any
/// last-known rows stay on screen.
fn remote_rows(live: Option<&RemoteGpuLive>, host_id: &str) -> (Vec<RemoteRow>, Option<String>) {
    let Some(live) = live.filter(|l| l.host_id == host_id) else {
        return (Vec::new(), Some(REMOTE_WAITING.to_string()));
    };

    let multi = live.gpus.len() > 1;
    let rows: Vec<RemoteRow> = live
        .gpus
        .iter()
        .enumerate()
        .map(|(i, view)| RemoteRow {
            short: if multi {
                format!("GPU{i}")
            } else {
                "GPU".to_string()
            },
            name: view.name.clone(),
            util: view.latest.util_pct,
            detail: remote_gpu_detail(&view.latest),
            history: view.util_history.clone(),
        })
        .collect();

    let status = if live.last_error.is_some() {
        Some(REMOTE_RETRYING.to_string())
    } else if rows.is_empty() {
        Some(REMOTE_WAITING.to_string())
    } else {
        None
    };
    (rows, status)
}

/// Second-line detail for a remote GPU: "VRAM u/t MiB  ·  T °C  ·  P W", dropping
/// any field the host didn't report, or `None` when none are present. The
/// utilization `%` is shown separately on the chart's headline line, so it is
/// intentionally not repeated here.
fn remote_gpu_detail(stat: &crate::engines::remote::hardware::RemoteGpuStat) -> Option<String> {
    let mut bits = Vec::new();
    if let (Some(used), Some(total)) = (stat.vram_used_mib, stat.vram_total_mib) {
        bits.push(format!("VRAM {used} / {total} MiB"));
    }
    if let Some(t) = stat.temp_c {
        bits.push(format!("{t} °C"));
    }
    if let Some(p) = stat.power_w {
        bits.push(format!("{p:.0} W"));
    }
    (!bits.is_empty()).then(|| bits.join("  ·  "))
}

/// One GPU's display data — one row/chart per card, never averaged.
struct GpuRow {
    /// Compact label for every surface: "GPU" for one card, else "GPU0"/"GPU1"/…
    short: String,
    /// Full model name, shown on hover.
    name: String,
    /// Live utilization as a percentage (0–100), `None` when the backend can't
    /// report it for this card (e.g. a non-NVIDIA GPU outside NVML's reach).
    util: Option<f32>,
    /// Normalized PCI bus id — key into the per-GPU sparkline history.
    bus_id: String,
    /// VRAM (used, total) bytes, when the backend reports it.
    vram: Option<(u64, u64)>,
}

/// De-duplicated real GPUs from the inventory: drop CPU/software adapters (e.g.
/// the WARP "Microsoft Basic Render Driver") and collapse a card enumerated under
/// more than one graphics backend (same PCI bus id, or same name when the id is
/// unknown).
fn real_gpus(inventory: &[GpuInfo]) -> Vec<&GpuInfo> {
    let mut seen = BTreeSet::new();
    inventory
        .iter()
        .filter(|g| {
            g.kind != GpuKind::Cpu && {
                let key = if g.pci_bus_id.is_empty() {
                    g.name.clone()
                } else {
                    gpu_monitor::normalize_bus_id(&g.pci_bus_id)
                };
                seen.insert(key)
            }
        })
        .collect()
}

/// Build one row per real GPU, pairing each with its live sample (utilization /
/// VRAM) when one is available. Falls back to a single line when the inventory is
/// empty (headless / software renderer).
fn gpu_rows(state: &AppState) -> Vec<GpuRow> {
    let samples = &state.ui.gpus;
    let gpus = real_gpus(crate::backend::hardware::gpus());

    if gpus.is_empty() {
        return vec![GpuRow {
            short: "GPU".to_string(),
            name: state
                .ui
                .gpu_name
                .clone()
                .unwrap_or_else(|| "GPU".to_string()),
            util: samples.first().and_then(|s| s.util_pct),
            bus_id: samples
                .first()
                .map(|s| s.pci_bus_id.clone())
                .unwrap_or_default(),
            vram: None,
        }];
    }

    let multi = gpus.len() > 1;
    gpus.iter()
        .enumerate()
        .map(|(i, g)| {
            let sample = gpu_monitor::find_sample(samples, g);
            let bus_id = if g.pci_bus_id.is_empty() {
                String::new()
            } else {
                gpu_monitor::normalize_bus_id(&g.pci_bus_id)
            };
            GpuRow {
                short: if multi {
                    format!("GPU{i}")
                } else {
                    "GPU".to_string()
                },
                name: g.name.clone(),
                util: sample.and_then(|s| s.util_pct),
                bus_id,
                vram: sample.and_then(|s| match (s.vram_used_bytes, s.vram_total_bytes) {
                    (Some(u), Some(t)) => Some((u, t)),
                    _ => None,
                }),
            }
        })
        .collect()
}

/// Source dropdown — "Local" plus every configured remote host — letting the dock
/// switch which machine it shows. Hidden when no remote hosts exist (nothing to
/// switch to). Rendered above the gauge cluster and outside its click target, so
/// opening the dropdown doesn't also toggle the detail popover.
fn render_source_selector(state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    let mut hosts: Vec<(String, String)> = state
        .config
        .remote_hosts
        .values()
        .map(|host| (host.id.clone(), host.label.clone()))
        .collect();
    if hosts.is_empty() {
        return;
    }
    hosts.sort_by(|a, b| a.1.cmp(&b.1));

    let current = state.ui.layout.monitor_source.clone();
    let selected_text = match &current {
        MonitorSource::Local => "Local".to_string(),
        MonitorSource::Remote(id) => hosts
            .iter()
            .find(|(hid, _)| hid == id)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| id.clone()),
    };

    egui::ComboBox::from_id_salt("monitor_source")
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            crate::frontend::theme::stabilize_selectable_rows(ui);
            let mut pick = current.clone();
            ui.selectable_value(&mut pick, MonitorSource::Local, "Local");
            for (id, label) in &hosts {
                ui.selectable_value(&mut pick, MonitorSource::Remote(id.clone()), label);
            }
            if pick != current {
                actions.push(AppAction::SetMonitorSource(pick));
            }
        });
    ui.add_space(5.0);
}

/// Footer (sidebar) glance view: the source dropdown above a stack of compact
/// one-line utilization bars. Local shows CPU, Memory, then one per GPU; a remote
/// host shows one bar per GPU (no remote CPU/memory). The gauge cluster is one
/// click target that toggles the detail popover and records its rect for anchoring.
pub(crate) fn render_compact_monitor(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) -> egui::Response {
    render_source_selector(state, ui, actions);
    let resp = match state.ui.layout.monitor_source.clone() {
        MonitorSource::Local => render_compact_local(state, ui),
        MonitorSource::Remote(id) => render_compact_remote(state, ui, &id),
    };
    arm_click(state, resp)
}

/// Compact local bars: CPU, Memory, then one per real GPU.
fn render_compact_local(state: &mut AppState, ui: &mut egui::Ui) -> egui::Response {
    let cpu = state.ui.cpu_pct;
    let mem = state.ui.mem_pct;
    let rows = gpu_rows(state);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        gauge::utilization_row_inline(ui, "CPU", None, &pct_text(Some(cpu)), Some(cpu / 100.0));
        gauge::utilization_row_inline(ui, "MEM", None, &pct_text(mem), mem.map(|m| m / 100.0));
        for row in &rows {
            gauge::utilization_row_inline(
                ui,
                &row.short,
                Some(&row.name),
                &pct_text(row.util),
                row.util.map(|u| u / 100.0),
            );
        }
    })
    .response
}

/// Compact remote bars: one per GPU of the selected host, or a muted status line
/// while waiting for the first sample / retrying after a transport error.
fn render_compact_remote(state: &mut AppState, ui: &mut egui::Ui, host_id: &str) -> egui::Response {
    let (rows, status) = remote_rows(state.ui.settings.remote_gpu_live.as_ref(), host_id);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        for row in &rows {
            gauge::utilization_row_inline(
                ui,
                &row.short,
                Some(&row.name),
                &pct_text(row.util),
                row.util.map(|u| u / 100.0),
            );
        }
        if let Some(status) = status {
            let pal = crate::frontend::theme::palette(ui);
            ui.label(RichText::new(status).color(pal.text_tertiary));
        }
    })
    .response
}

/// Status-bar fallback (sidebar hidden): the source dropdown followed by one
/// compact, clickable line of colored chips. Local shows CPU, Memory, then one per
/// GPU; a remote host shows one chip per GPU. The dropdown is kept here too so the
/// source can still be switched (back to Local) when the sidebar is hidden.
pub(crate) fn render_status_monitor(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) -> egui::Response {
    render_source_selector(state, ui, actions);
    let resp = match state.ui.layout.monitor_source.clone() {
        MonitorSource::Local => render_status_local(state, ui),
        MonitorSource::Remote(id) => render_status_remote(state, ui, &id),
    };
    arm_click(state, resp)
}

/// Status-bar chips for the local machine: CPU, Memory, then one per real GPU.
fn render_status_local(state: &mut AppState, ui: &mut egui::Ui) -> egui::Response {
    let pal = crate::frontend::theme::palette(ui);
    let cpu = state.ui.cpu_pct;
    let mem = state.ui.mem_pct;
    let rows = gpu_rows(state);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        status_chip(ui, &pal, "CPU", None, Some(cpu));
        status_chip(ui, &pal, "MEM", None, mem);
        for row in &rows {
            status_chip(ui, &pal, &row.short, Some(&row.name), row.util);
        }
    })
    .response
}

/// Status-bar chips for a remote host: one per GPU, or a muted status line while
/// waiting / retrying.
fn render_status_remote(state: &mut AppState, ui: &mut egui::Ui, host_id: &str) -> egui::Response {
    let pal = crate::frontend::theme::palette(ui);
    let (rows, status) = remote_rows(state.ui.settings.remote_gpu_live.as_ref(), host_id);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for row in &rows {
            status_chip(ui, &pal, &row.short, Some(&row.name), row.util);
        }
        if let Some(status) = status {
            ui.label(RichText::new(status).small().color(pal.text_tertiary));
        }
    })
    .response
}

/// Detail popover: two-line utilization charts (CPU, Memory, then one per GPU)
/// with richer values; the GPU rows carry the full model name on hover. Anchored
/// just above the glance widget; closes on Escape or a click outside both the
/// popover and the trigger.
pub(crate) fn render_monitor_popover(state: &mut AppState, ctx: &egui::Context) {
    // Belt-and-suspenders: never draw the popover when the monitor is disabled,
    // even if some path left `monitor_popover_open` set (the trigger widget that
    // would otherwise close it isn't rendered while the monitor is off).
    if !state.config.show_utilization_bars || !state.ui.layout.monitor_popover_open {
        return;
    }
    let Some(anchor) = state.ui.layout.monitor_anchor else {
        return;
    };

    let source = state.ui.layout.monitor_source.clone();
    let (remote_rows_vec, remote_status) = match &source {
        MonitorSource::Local => (Vec::new(), None),
        MonitorSource::Remote(id) => remote_rows(state.ui.settings.remote_gpu_live.as_ref(), id),
    };
    let cpu = state.ui.cpu_pct;
    let mem = state.ui.mem_pct;
    let rows = gpu_rows(state);
    let total_ram = crate::backend::hardware::info().total_ram_bytes as f64;
    let cpu_hist = &state.ui.monitor_history.cpu;
    let mem_hist = &state.ui.monitor_history.mem;
    let gpu_hists = &state.ui.monitor_history.gpus;
    let empty_hist = VecDeque::new();

    // Match the popover to the sidebar: the anchor is the footer's full-width
    // gauge cluster, so the popover tracks the sidebar as it's resized. The
    // content width drops the two horizontal margins; a floor keeps the narrow
    // status-bar fallback readable.
    const H_MARGIN: i8 = 14;
    // Gap left between the popover's bottom and the gauge cluster above which it
    // opens, so it doesn't crowd the always-visible bars.
    const GAP_ABOVE_CLUSTER: f32 = 10.0;
    let content_width = (anchor.width() - 2.0 * f32::from(H_MARGIN)).max(200.0);

    let area = egui::Area::new(egui::Id::new("system_monitor_popover"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(anchor.left(), anchor.top() - GAP_ABOVE_CLUSTER))
        .pivot(egui::Align2::LEFT_BOTTOM)
        .constrain(true)
        .show(ctx, |ui| {
            let pal = crate::frontend::theme::palette(ui);
            egui::Frame::popup(ui.style())
                .fill(pal.window_backing)
                .stroke(egui::Stroke::new(1.0_f32, pal.hairline))
                .corner_radius(egui::CornerRadius::same(
                    crate::frontend::theme::radius::CARD,
                ))
                .inner_margin(egui::Margin::symmetric(H_MARGIN, 12))
                .show(ui, |ui| {
                    ui.set_width(content_width);
                    match &source {
                        MonitorSource::Local => {
                            gauge::utilization_chart(
                                ui,
                                "CPU",
                                None,
                                &format!("{cpu:.0}%"),
                                None,
                                cpu_hist,
                                Some(cpu),
                            );
                            ui.add_space(14.0);
                            gauge::utilization_chart(
                                ui,
                                "Memory",
                                None,
                                &memory_detail(mem, total_ram),
                                None,
                                mem_hist,
                                mem,
                            );
                            for row in &rows {
                                ui.add_space(14.0);
                                let hist = gpu_hists.get(&row.bus_id).unwrap_or(&empty_hist);
                                gauge::utilization_chart(
                                    ui,
                                    &row.short,
                                    Some(&row.name),
                                    &pct_text(row.util),
                                    gpu_detail(row).as_deref(),
                                    hist,
                                    row.util,
                                );
                            }
                        }
                        MonitorSource::Remote(_) => {
                            for (i, row) in remote_rows_vec.iter().enumerate() {
                                if i > 0 {
                                    ui.add_space(14.0);
                                }
                                gauge::utilization_chart(
                                    ui,
                                    &row.short,
                                    Some(&row.name),
                                    &pct_text(row.util),
                                    row.detail.as_deref(),
                                    &row.history,
                                    row.util,
                                );
                            }
                            if let Some(status) = &remote_status {
                                if !remote_rows_vec.is_empty() {
                                    ui.add_space(14.0);
                                }
                                let pal = crate::frontend::theme::palette(ui);
                                ui.label(RichText::new(status).color(pal.text_tertiary));
                            }
                        }
                    }
                });
        });

    let popover_rect = area.response.rect;
    let clicked_outside = ctx.input(|i| {
        i.pointer.any_click()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !popover_rect.contains(p) && !anchor.contains(p))
    });
    if clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.ui.layout.monitor_popover_open = false;
    }
}

/// Turn a glance-cluster response into a click target: record its rect (for
/// popover anchoring) and toggle the popover on click.
fn arm_click(state: &mut AppState, resp: egui::Response) -> egui::Response {
    let resp = resp.interact(Sense::click());
    state.ui.layout.monitor_anchor = Some(resp.rect);
    if resp.clicked() {
        state.ui.layout.monitor_popover_open = !state.ui.layout.monitor_popover_open;
    }
    resp
}

/// A small threshold-colored dot followed by "LABEL pct%" for the status bar,
/// with `tooltip` (the full GPU name) shown on hover.
fn status_chip(
    ui: &mut egui::Ui,
    pal: &crate::frontend::theme::Palette,
    label: &str,
    tooltip: Option<&str>,
    value: Option<f32>,
) {
    let color = value.map_or(pal.text_tertiary, |pct| gauge::gauge_color(pal, pct));
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(RichText::new("●").small().color(color));
            ui.label(
                RichText::new(format!("{label} {}", pct_text(value)))
                    .small()
                    .color(pal.text_muted),
            );
        })
        .response;
    if let Some(tooltip) = tooltip {
        resp.on_hover_text(tooltip);
    }
}

/// "pct%" or "N/A".
fn pct_text(value: Option<f32>) -> String {
    value.map_or_else(|| "N/A".to_string(), |pct| format!("{pct:.0}%"))
}

/// "used / total GiB (pct%)" reconstructed from the percentage and total RAM, or
/// "N/A" when no memory sample is available.
fn memory_detail(mem_pct: Option<f32>, total_ram: f64) -> String {
    match mem_pct {
        Some(pct) => {
            let used = total_ram * (pct as f64 / 100.0);
            format!("{:.1} / {:.1} GiB ({pct:.0}%)", used / GIB, total_ram / GIB)
        }
        None => "N/A".to_string(),
    }
}

/// Second-line detail for a local GPU: "VRAM used / total GiB", or `None` when the
/// backend didn't report VRAM. The utilization `%` is shown separately on the
/// chart's headline line, so it is intentionally not repeated here.
fn gpu_detail(row: &GpuRow) -> Option<String> {
    row.vram.map(|(used, total)| {
        format!(
            "VRAM {:.1} / {:.1} GiB",
            used as f64 / GIB,
            total as f64 / GIB
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(name: &str, kind: GpuKind, bus: &str) -> GpuInfo {
        GpuInfo {
            name: name.into(),
            kind,
            vendor: 0,
            pci_bus_id: bus.into(),
            backend: String::new(),
        }
    }

    use crate::engines::remote::hardware::RemoteGpuStat;
    use crate::frontend::state::RemoteGpuView;

    fn stat(index: u32, name: &str, util: Option<f32>) -> RemoteGpuStat {
        RemoteGpuStat {
            index,
            name: name.into(),
            util_pct: util,
            vram_used_mib: None,
            vram_total_mib: None,
            temp_c: None,
            power_w: None,
        }
    }

    fn view(stat: RemoteGpuStat) -> RemoteGpuView {
        let mut history = VecDeque::new();
        history.push_back(stat.util_pct);
        RemoteGpuView {
            index: stat.index,
            name: stat.name.clone(),
            latest: stat,
            util_history: history,
        }
    }

    fn remote_live(host_id: &str, views: Vec<RemoteGpuView>) -> RemoteGpuLive {
        RemoteGpuLive {
            host_id: host_id.into(),
            gpus: views,
            last_error: None,
        }
    }

    #[test]
    fn remote_rows_single_gpu_is_labelled_gpu() {
        let live = remote_live("h", vec![view(stat(0, "RTX 3070 Ti", Some(42.0)))]);
        let (rows, status) = remote_rows(Some(&live), "h");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].short, "GPU");
        assert_eq!(rows[0].name, "RTX 3070 Ti");
        assert_eq!(rows[0].util, Some(42.0));
        assert!(status.is_none());
    }

    #[test]
    fn remote_rows_multiple_gpus_are_indexed_by_position() {
        let live = remote_live(
            "h",
            vec![view(stat(0, "A", Some(1.0))), view(stat(3, "B", Some(2.0)))],
        );
        let (rows, _) = remote_rows(Some(&live), "h");
        assert_eq!(rows.len(), 2);
        // Position-based labels, not the (possibly sparse) nvidia-smi index.
        assert_eq!(rows[0].short, "GPU0");
        assert_eq!(rows[1].short, "GPU1");
    }

    #[test]
    fn remote_rows_empty_with_no_error_is_waiting() {
        let live = remote_live("h", Vec::new());
        let (rows, status) = remote_rows(Some(&live), "h");
        assert!(rows.is_empty());
        assert_eq!(status.as_deref(), Some(REMOTE_WAITING));
    }

    #[test]
    fn remote_rows_with_error_shows_retry_status() {
        let mut live = remote_live("h", vec![view(stat(0, "A", Some(5.0)))]);
        live.last_error = Some("connection refused".into());
        let (rows, status) = remote_rows(Some(&live), "h");
        assert_eq!(
            rows.len(),
            1,
            "last-known rows stay on screen while retrying"
        );
        assert_eq!(status.as_deref(), Some(REMOTE_RETRYING));
    }

    #[test]
    fn remote_rows_host_mismatch_is_waiting() {
        let live = remote_live("a", vec![view(stat(0, "A", Some(5.0)))]);
        let (rows, status) = remote_rows(Some(&live), "b");
        assert!(rows.is_empty());
        assert_eq!(status.as_deref(), Some(REMOTE_WAITING));
    }

    #[test]
    fn remote_rows_none_live_is_waiting() {
        let (rows, status) = remote_rows(None, "h");
        assert!(rows.is_empty());
        assert_eq!(status.as_deref(), Some(REMOTE_WAITING));
    }

    #[test]
    fn remote_rows_detail_has_vram_temp_power_but_not_pct() {
        let mut s = stat(0, "A", Some(50.0));
        s.vram_used_mib = Some(512);
        s.vram_total_mib = Some(8192);
        s.temp_c = Some(45);
        s.power_w = Some(60.0);
        let live = remote_live("h", vec![view(s)]);
        let (rows, _) = remote_rows(Some(&live), "h");
        // The utilization % is carried by `util` (shown on the headline line), so the
        // detail line must NOT repeat it — that double-render plus the long string is
        // what overflowed and overlapped the label in a narrow popover.
        assert_eq!(rows[0].util, Some(50.0));
        let detail = rows[0].detail.as_deref().expect("detail present");
        assert!(
            !detail.contains('%'),
            "detail must omit the pct: {detail:?}"
        );
        assert!(detail.contains("512 / 8192 MiB"), "detail = {detail:?}");
        assert!(detail.contains("45 °C"), "detail = {detail:?}");
        assert!(detail.contains("60 W"), "detail = {detail:?}");
    }

    #[test]
    fn remote_rows_detail_is_none_when_only_util_reported() {
        // util present but no VRAM/temp/power → no second line at all (not an empty
        // string), so the chart skips the detail row.
        let live = remote_live("h", vec![view(stat(0, "A", Some(50.0)))]);
        let (rows, _) = remote_rows(Some(&live), "h");
        assert_eq!(rows[0].util, Some(50.0));
        assert_eq!(rows[0].detail, None);
    }

    #[test]
    fn gpu_detail_is_vram_only_or_none() {
        let mut row = GpuRow {
            short: "GPU".into(),
            name: "RTX".into(),
            util: Some(30.0),
            bus_id: String::new(),
            vram: Some((6 * 1024 * 1024 * 1024, 8 * 1024 * 1024 * 1024)),
        };
        let detail = gpu_detail(&row).expect("vram present");
        assert!(detail.starts_with("VRAM "), "detail = {detail:?}");
        assert!(
            !detail.contains('%'),
            "detail must omit the pct: {detail:?}"
        );
        // No VRAM reported → no detail line.
        row.vram = None;
        assert_eq!(gpu_detail(&row), None);
    }

    #[test]
    fn real_gpus_drops_cpu_and_dedups_same_card() {
        let inv = vec![
            gpu("Intel Arc Graphics", GpuKind::Integrated, "0000:00:02.0"),
            gpu("NVIDIA RTX 4070", GpuKind::Discrete, "0000:01:00.0"),
            // Same iGPU enumerated under a second backend (Vulkan + Dx12).
            gpu("Intel Arc Graphics", GpuKind::Integrated, "0000:00:02.0"),
            // Software/WARP adapter — not a real GPU.
            gpu("Microsoft Basic Render Driver", GpuKind::Cpu, ""),
        ];
        let real = real_gpus(&inv);
        assert_eq!(real.len(), 2, "iGPU deduped, CPU/software dropped");
        assert_eq!(real[0].name, "Intel Arc Graphics");
        assert_eq!(real[1].name, "NVIDIA RTX 4070");
    }
}
