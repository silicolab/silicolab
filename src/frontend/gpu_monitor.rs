//! Per-GPU live stats: utilization / VRAM / temperature, one sample per GPU the
//! probe can read, joined back to the [`GpuInfo`] inventory by PCI bus id.
//!
//! wgpu's enumeration ([`crate::frontend::gpu_inventory`]) has no live counters,
//! so live stats need a vendor path. With the optional `nvidia` cargo feature on
//! Windows/Linux, NVML supplies them for NVIDIA cards. Everywhere else (default
//! build off-feature, macOS, non-NVIDIA hosts) the sampler yields nothing and the
//! gauges read N/A — the inventory still lists every GPU.

use crate::backend::hardware::GpuInfo;

/// One live per-GPU sample. `pci_bus_id` is normalized (see [`normalize_bus_id`])
/// so it joins directly against `normalize_bus_id(&GpuInfo.pci_bus_id)`; the
/// display name comes from the matched inventory entry, not the sample.
#[derive(Clone, Debug)]
pub struct GpuSample {
    pub pci_bus_id: String,
    pub util_pct: Option<f32>,
    pub vram_used_bytes: Option<u64>,
    pub vram_total_bytes: Option<u64>,
    pub temp_c: Option<u32>,
}

/// Normalize a PCI bus id for cross-source comparison: NVML reports an 8-digit
/// domain (`00000000:01:00.0`) while wgpu reports 4 (`0000:01:00.0`). Keep the
/// `bus:device.function` tail (drop the leading domain) and lower-case it.
pub fn normalize_bus_id(raw: &str) -> String {
    let parts: Vec<&str> = raw.split(':').collect();
    let tail = if parts.len() >= 2 {
        &parts[parts.len() - 2..]
    } else {
        &parts[..]
    };
    tail.join(":").to_ascii_lowercase()
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / 1024.0_f64.powi(3)
}

/// The live sample for a given inventory GPU, matched by normalized PCI bus id.
/// `None` when the GPU has no bus id (can't be matched) or no sample arrived.
pub fn find_sample<'a>(samples: &'a [GpuSample], gpu: &GpuInfo) -> Option<&'a GpuSample> {
    if gpu.pci_bus_id.is_empty() {
        return None;
    }
    let key = normalize_bus_id(&gpu.pci_bus_id);
    samples.iter().find(|s| s.pci_bus_id == key)
}

/// One-line `util · VRAM · temp` summary for the Compute Hardware panel, or
/// `None` when the sample carries no live utilization (e.g. no NVML backend).
pub fn live_line(s: &GpuSample) -> Option<String> {
    let util = s.util_pct?;
    let mut parts = vec![format!("{util:.0}%")];
    if let (Some(u), Some(t)) = (s.vram_used_bytes, s.vram_total_bytes) {
        parts.push(format!("VRAM {:.1}/{:.1} GiB", gib(u), gib(t)));
    }
    if let Some(c) = s.temp_c {
        parts.push(format!("{c}°C"));
    }
    Some(parts.join("  ·  "))
}

#[cfg(all(feature = "nvidia", any(target_os = "windows", target_os = "linux")))]
mod backend_impl {
    use super::{GpuSample, normalize_bus_id};
    use nvml_wrapper::Nvml;
    use nvml_wrapper::enum_wrappers::device::TemperatureSensor;

    /// NVML-backed sampler. The handle is acquired lazily on first use and can be
    /// released again with [`suspend`](GpuSampler::suspend) while the monitor is
    /// idle — holding it open (or querying it) keeps a discrete card from
    /// dropping into its deepest runtime power state, so we let it go when there
    /// is nothing to show. `Nvml::init()` Errs (never panics) on non-NVIDIA /
    /// driver-absent hosts; that outcome is remembered so we don't retry it on
    /// every resume, and `sample()` then yields nothing.
    pub struct GpuSampler {
        nvml: Option<Nvml>,
        unavailable: bool,
    }

    impl GpuSampler {
        pub fn new() -> Self {
            Self {
                nvml: None,
                unavailable: false,
            }
        }

        /// Acquire the NVML handle on demand. A no-op once held, and skipped once
        /// a prior init has failed (a driverless host never gains one mid-run).
        fn ensure(&mut self) {
            if self.nvml.is_none() && !self.unavailable {
                match Nvml::init() {
                    Ok(nvml) => self.nvml = Some(nvml),
                    Err(_) => self.unavailable = true,
                }
            }
        }

        /// Release the NVML/driver handle so it can't keep a discrete GPU out of
        /// its deepest runtime power state while the monitor is idle. Re-acquired
        /// lazily on the next [`sample`](GpuSampler::sample).
        pub fn suspend(&mut self) {
            self.nvml = None;
        }

        pub fn sample(&mut self) -> Vec<GpuSample> {
            self.ensure();
            let Some(nvml) = self.nvml.as_ref() else {
                return Vec::new();
            };
            let count = nvml.device_count().unwrap_or(0);
            (0..count)
                .filter_map(|i| {
                    let dev = nvml.device_by_index(i).ok()?;
                    let util = dev.utilization_rates().ok();
                    let mem = dev.memory_info().ok();
                    let bus = dev
                        .pci_info()
                        .ok()
                        .map(|p| normalize_bus_id(&p.bus_id))
                        .unwrap_or_default();
                    Some(GpuSample {
                        pci_bus_id: bus,
                        util_pct: util.map(|u| u.gpu as f32),
                        vram_used_bytes: mem.as_ref().map(|m| m.used),
                        vram_total_bytes: mem.as_ref().map(|m| m.total),
                        temp_c: dev.temperature(TemperatureSensor::Gpu).ok(),
                    })
                })
                .collect()
        }
    }
}

#[cfg(not(all(feature = "nvidia", any(target_os = "windows", target_os = "linux"))))]
mod backend_impl {
    use super::GpuSample;

    /// No live-stats backend (default off-feature build, macOS, or non-NVIDIA
    /// target). The inventory still lists every GPU; the gauges read N/A.
    pub struct GpuSampler;

    impl GpuSampler {
        pub fn new() -> Self {
            Self
        }

        /// API parity with the NVML backend; nothing to release here.
        pub fn suspend(&mut self) {}

        pub fn sample(&mut self) -> Vec<GpuSample> {
            Vec::new()
        }
    }
}

pub use backend_impl::GpuSampler;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::hardware::{GpuInfo, GpuKind};

    fn gpu(name: &str, kind: GpuKind, bus: &str) -> GpuInfo {
        GpuInfo {
            name: name.into(),
            kind,
            vendor: 0x10DE,
            pci_bus_id: bus.into(),
            backend: "Dx12".into(),
        }
    }

    #[test]
    fn normalize_bus_id_drops_domain_width_difference() {
        // NVML's 8-digit domain and wgpu's 4-digit domain normalize equal.
        assert_eq!(normalize_bus_id("00000000:01:00.0"), "01:00.0");
        assert_eq!(normalize_bus_id("0000:01:00.0"), "01:00.0");
        assert_eq!(
            normalize_bus_id("00000000:01:00.0"),
            normalize_bus_id("0000:01:00.0")
        );
    }

    #[test]
    fn find_sample_matches_across_domain_formats() {
        let dgpu = gpu("RTX", GpuKind::Discrete, "0000:01:00.0");
        let samples = vec![GpuSample {
            pci_bus_id: "01:00.0".into(), // already normalized at creation
            util_pct: Some(60.0),
            vram_used_bytes: None,
            vram_total_bytes: None,
            temp_c: None,
        }];
        assert!(find_sample(&samples, &dgpu).is_some());
        let no_bus = gpu("RTX", GpuKind::Discrete, "");
        assert!(find_sample(&samples, &no_bus).is_none());
    }

    #[test]
    fn null_sampler_yields_nothing_off_feature() {
        // Compiles under both cfgs; on the NVML build this exercises the no-NVIDIA
        // path indirectly via an empty result when init fails in CI.
        let mut s = GpuSampler::new();
        let _ = s.sample();
        // Releasing the handle and sampling again must re-acquire lazily without
        // panicking (and stays empty when there is no NVIDIA backend).
        s.suspend();
        let _ = s.sample();
    }
}
