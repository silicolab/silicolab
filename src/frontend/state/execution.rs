//! Engine-neutral execution preferences shared by every task panel: where a job
//! runs (the compute target) and its resource envelope (parallel subtasks, CPU
//! cores per subtask, GPUs, memory). A given task supports only a subset of these
//! knobs; [`ExecutionCaps`] says which, so a panel greys out the rest (shown, not
//! hidden) and every panel reads the same. The dispatcher translates these neutral
//! values into each engine's own request form at submit time.

use crate::backend::config::{AppConfig, ComputeTarget};

/// The execution envelope a task panel edits. Carried per-prompt and seeded from
/// the global defaults when a panel opens (see [`ExecutionPrefs::seeded`]); the
/// user can submit as-is or override it for this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionPrefs {
    /// Run locally or on a configured remote host.
    pub target: ComputeTarget,
    /// Independent parallel subtasks (e.g. docking replicas, one process each);
    /// `1` is a single task.
    pub subtasks: u32,
    /// CPU cores per subtask. `0` means "let the engine decide" (its default,
    /// which for QM is the global core cap).
    pub cores_per_subtask: u32,
    /// GPUs to use; `0` means none / CPU only.
    pub gpu_count: u32,
    /// Memory cap per subtask in MiB; `0` means no explicit cap.
    pub memory_mib: u32,
}

impl Default for ExecutionPrefs {
    fn default() -> Self {
        Self {
            target: ComputeTarget::Local,
            subtasks: 1,
            cores_per_subtask: 0,
            gpu_count: 0,
            memory_mib: 0,
        }
    }
}

impl ExecutionPrefs {
    /// Seed a fresh panel's prefs from the global defaults: the compute target
    /// comes from `config.default_compute_target`. The resource knobs have no
    /// global default yet, so they start at "engine decides" and are overridden
    /// per run.
    pub fn seeded(config: &AppConfig) -> Self {
        Self {
            target: config.default_compute_target.clone(),
            ..Self::default()
        }
    }
}

/// Which resource knobs a task actually honours. The compute target is always
/// offered; an unsupported knob renders greyed out rather than hidden, so the
/// execution section keeps a consistent shape across every task panel.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExecutionCaps {
    pub subtasks: bool,
    pub cores: bool,
    pub gpu: bool,
    pub memory: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_takes_target_from_config_and_defaults_resources() {
        let config = AppConfig {
            default_compute_target: ComputeTarget::Remote("hpc".to_string()),
            ..AppConfig::default()
        };

        let prefs = ExecutionPrefs::seeded(&config);

        // The compute target is seeded from the global default…
        assert_eq!(prefs.target, ComputeTarget::Remote("hpc".to_string()));
        // …while the resource knobs start at "engine decides" (single subtask,
        // auto cores, no GPU, no memory cap), to be overridden per run.
        assert_eq!(prefs.subtasks, 1);
        assert_eq!(prefs.cores_per_subtask, 0);
        assert_eq!(prefs.gpu_count, 0);
        assert_eq!(prefs.memory_mib, 0);
    }
}
