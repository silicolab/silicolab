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
    /// CPU cores per subtask. Panels seed this from the global default cores
    /// (`AppConfig::compute_core_count`); `0` means "let the engine decide".
    pub cores_per_subtask: u32,
    /// GPUs to use; `0` means none / CPU only.
    pub gpu: crate::backend::config::GpuRequest,
    pub gpu_explicit: bool,
    /// Memory cap per subtask in MiB; `0` means no explicit cap.
    pub memory_mib: u32,
    pub walltime_minutes: u32,
}

impl Default for ExecutionPrefs {
    fn default() -> Self {
        Self {
            target: ComputeTarget::Local,
            subtasks: 1,
            cores_per_subtask: 0,
            gpu: crate::backend::config::GpuRequest::None,
            gpu_explicit: false,
            memory_mib: 0,
            walltime_minutes: 0,
        }
    }
}

impl ExecutionPrefs {
    /// Seed a fresh panel's prefs from the global defaults: the compute target
    /// from `config.default_compute_target`, and the per-subtask core count from
    /// `config.compute_core_count` (the global "Default CPU cores"). Both are
    /// overridable per run. The remaining knobs have no global default yet, so
    /// they start at "engine decides".
    pub fn seeded(config: &AppConfig) -> Self {
        Self {
            target: config.default_compute_target.clone(),
            cores_per_subtask: config.compute_core_count as u32,
            ..Self::default()
        }
    }

    pub fn job_resources(&self) -> crate::backend::config::JobResources {
        crate::backend::config::JobResources {
            cpus_per_task: (self.cores_per_subtask > 0).then_some(self.cores_per_subtask),
            memory_mib: (self.memory_mib > 0).then_some(self.memory_mib as u64),
            walltime_seconds: (self.walltime_minutes > 0)
                .then_some(self.walltime_minutes as u64 * 60),
            gpu: self.gpu.clone(),
            gpu_explicit: self.gpu_explicit,
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
    pub walltime: bool,
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
        // …the per-subtask core count seeds from the global default…
        assert_eq!(prefs.cores_per_subtask, config.compute_core_count as u32);
        // …while the remaining knobs start at "engine decides" (single subtask,
        // no GPU, no memory cap), to be overridden per run.
        assert_eq!(prefs.subtasks, 1);
        assert_eq!(prefs.gpu, crate::backend::config::GpuRequest::None);
        assert!(!prefs.gpu_explicit);
        assert_eq!(prefs.memory_mib, 0);
        assert_eq!(prefs.walltime_minutes, 0);
    }
}
