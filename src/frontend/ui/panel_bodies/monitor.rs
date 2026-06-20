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
use crate::frontend::gpu_monitor;
use crate::frontend::state::AppState;
use crate::frontend::ui::gauge;

/// Bytes per GiB, for human-readable memory/VRAM figures.
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

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

/// Footer (sidebar) glance view: compact one-line utilization bars — CPU, Memory,
/// then one per GPU. The whole cluster is one click target that toggles the
/// detail popover and records its rect for anchoring.
pub(crate) fn render_compact_monitor(state: &mut AppState, ui: &mut egui::Ui) -> egui::Response {
    let cpu = state.ui.cpu_pct;
    let mem = state.ui.mem_pct;
    let rows = gpu_rows(state);

    let resp = ui
        .vertical(|ui| {
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
        .response;

    arm_click(state, resp)
}

/// Status-bar fallback (sidebar hidden): one compact, clickable line of colored
/// chips — CPU, Memory, then one per GPU.
pub(crate) fn render_status_monitor(state: &mut AppState, ui: &mut egui::Ui) -> egui::Response {
    let pal = crate::frontend::theme::palette(ui);
    let cpu = state.ui.cpu_pct;
    let mem = state.ui.mem_pct;
    let rows = gpu_rows(state);

    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            status_chip(ui, &pal, "CPU", None, Some(cpu));
            status_chip(ui, &pal, "MEM", None, mem);
            for row in &rows {
                status_chip(ui, &pal, &row.short, Some(&row.name), row.util);
            }
        })
        .response;

    arm_click(state, resp)
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
                .stroke(egui::Stroke::new(1.0, pal.hairline))
                .corner_radius(egui::CornerRadius::same(
                    crate::frontend::theme::radius::CARD,
                ))
                .inner_margin(egui::Margin::symmetric(H_MARGIN, 12))
                .show(ui, |ui| {
                    ui.set_width(content_width);
                    gauge::utilization_chart(
                        ui,
                        "CPU",
                        None,
                        &format!("{cpu:.0}%"),
                        cpu_hist,
                        Some(cpu),
                    );
                    ui.add_space(14.0);
                    gauge::utilization_chart(
                        ui,
                        "Memory",
                        None,
                        &memory_detail(mem, total_ram),
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
                            &gpu_detail(row),
                            hist,
                            row.util,
                        );
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

/// "pct%   VRAM used/total GiB" (VRAM only when the backend reports it), or "N/A".
fn gpu_detail(row: &GpuRow) -> String {
    match row.util {
        Some(pct) => {
            let mut s = format!("{pct:.0}%");
            if let Some((used, total)) = row.vram {
                s.push_str(&format!(
                    "   VRAM {:.1} / {:.1} GiB",
                    used as f64 / GIB,
                    total as f64 / GIB
                ));
            }
            s
        }
        None => "N/A".to_string(),
    }
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
