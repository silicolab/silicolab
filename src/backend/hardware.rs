//! Cheap, cached host-hardware probe: total RAM and a derived QM memory budget
//! (Commit 1); CPU/GPU/core inventory follows in the Compute Hardware panel.

use std::sync::OnceLock;

use sysinfo::System;

/// Fraction of total physical RAM an in-core QM job may claim before the guard
/// warns. In-core SCF allocates the full nao⁴ ERI tensor up front, so we leave a
/// wide margin for the OS, the rest of the app, and SCF working set.
pub const QM_INCORE_RAM_FRACTION_PCT: u64 = 70;

/// Snapshot of the host hardware reported at first call.
#[derive(Debug, Clone)]
pub struct HardwareInfo {
    pub cpu_brand: String,
    pub physical_cores: usize,
    pub logical_cores: usize,
    /// Apple-Silicon performance ("big") core count, when the OS exposes it.
    pub performance_cores: Option<usize>,
    /// Apple-Silicon efficiency ("little") core count, when the OS exposes it.
    pub efficiency_cores: Option<usize>,
    pub total_ram_bytes: u64,
}

static INFO: OnceLock<HardwareInfo> = OnceLock::new();

/// Cached host inventory. Probed once on first call; subsequent calls are free.
pub fn info() -> &'static HardwareInfo {
    INFO.get_or_init(probe)
}

fn probe() -> HardwareInfo {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    let cpu_brand = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".to_string());
    let logical_cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let physical_cores = sys.physical_core_count().unwrap_or(logical_cores).max(1);
    let (performance_cores, efficiency_cores) = perf_efficiency_cores();
    HardwareInfo {
        cpu_brand,
        physical_cores,
        logical_cores,
        performance_cores,
        efficiency_cores,
        total_ram_bytes: sys.total_memory(),
    }
}

/// Best-effort P/E core split. Apple Silicon exposes it via sysctl perf levels
/// (`hw.perflevel0.physicalcpu` = performance, `hw.perflevel1.physicalcpu` =
/// efficiency). Everywhere else we don't claim a split.
#[cfg(target_os = "macos")]
fn perf_efficiency_cores() -> (Option<usize>, Option<usize>) {
    fn sysctl_usize(key: &str) -> Option<usize> {
        let out = std::process::Command::new("sysctl")
            .arg("-n")
            .arg(key)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8(out.stdout).ok()?.trim().parse().ok()
    }
    (
        sysctl_usize("hw.perflevel0.physicalcpu"),
        sysctl_usize("hw.perflevel1.physicalcpu"),
    )
}

#[cfg(not(target_os = "macos"))]
fn perf_efficiency_cores() -> (Option<usize>, Option<usize>) {
    (None, None)
}

/// Clamp a requested core count to the valid range [1, logical].
///
/// Used by the compute-hardware settings handler so the pure clamping logic
/// is unit-testable without touching the config file.
pub fn clamp_core_count(requested: usize, logical: usize) -> usize {
    requested.clamp(1, logical.max(1))
}

/// Total physical RAM in bytes (sysinfo reports bytes since 0.30).
pub fn total_memory_bytes() -> u64 {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.total_memory()
}

/// The most memory an in-core QM run may be estimated to need before the guard
/// intervenes. Computed each call (cheap: one `refresh_memory`).
pub fn qm_incore_budget_bytes() -> u64 {
    total_memory_bytes() / 100 * QM_INCORE_RAM_FRACTION_PCT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_reports_a_sane_inventory() {
        let hw = info();
        assert!(hw.logical_cores >= 1);
        assert!(hw.physical_cores >= 1 && hw.physical_cores <= hw.logical_cores);
        assert!(hw.total_ram_bytes > 0);
        assert!(!hw.cpu_brand.is_empty());
        // P/E counts are best-effort; when both known they shouldn't exceed physical.
        if let (Some(p), Some(e)) = (hw.performance_cores, hw.efficiency_cores) {
            assert!(p + e >= 1);
        }
    }

    #[test]
    fn budget_is_a_fraction_of_total_ram() {
        let total = total_memory_bytes();
        assert!(total > 0, "host should report some RAM");
        let budget = qm_incore_budget_bytes();
        assert!(
            budget > 0 && budget < total,
            "budget {budget} must be a positive fraction of {total}"
        );
        assert_eq!(budget, total / 100 * QM_INCORE_RAM_FRACTION_PCT);
    }

    #[test]
    fn clamp_core_count_clamps_below_one() {
        assert_eq!(clamp_core_count(0, 10), 1);
    }

    #[test]
    fn clamp_core_count_clamps_above_logical() {
        assert_eq!(clamp_core_count(100_000, 10), 10);
    }

    #[test]
    fn clamp_core_count_passes_through_valid() {
        assert_eq!(clamp_core_count(4, 10), 4);
    }
}
