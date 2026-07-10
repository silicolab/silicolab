use super::super::*;
use crate::engines::qm::{MemoryVerdict, QmScfBackend, memory_verdict};
use crate::frontend::actions::{Notification, NotificationButton, NotificationSeverity};

/// Resolve the product structure for a two-endpoint transition-state search from
/// the prompt's chosen entry, loading it on demand. `None` for any other route or
/// when no usable (non-empty) entry is chosen.
fn resolve_ts_product(
    state: &mut AppState,
    prompt: &crate::frontend::state::QmPrompt,
) -> Option<crate::domain::Structure> {
    if prompt.kind != crate::engines::qm::QmKind::TransitionState
        || prompt.ts.route != crate::frontend::state::TsRouteKind::TwoEndpoint
    {
        return None;
    }
    let entry_id = prompt.ts.product_entry?;
    state.ensure_entry_loaded(entry_id);
    state
        .entries
        .entry(entry_id)
        .map(|entry| entry.structure.clone())
        .filter(|structure| !structure.atoms.is_empty())
}

pub(crate) fn start_pending_qm(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::QmPrompt);
    let Some(prompt) = state.ui.pending_qm.clone() else {
        return;
    };
    if state.jobs.qm_running() {
        state.set_message("a QM calculation is already running; press Esc to stop".to_string());
        return;
    }
    if state.structure().atoms.is_empty() {
        state.set_message("open a structure before running a QM calculation".to_string());
        return;
    }
    // A periodic run needs a real unit cell; reject early with a clear message
    // rather than letting the worker fail (the panel only offers the periodic
    // mode when a cell is present, but the prompt can outlive an entry switch).
    if prompt.periodic
        && state
            .structure()
            .cell
            .as_ref()
            .filter(|cell| !cell.is_placeholder())
            .is_none()
    {
        state
            .set_message("periodic QM needs a real unit cell; this structure has none".to_string());
        return;
    }
    // Memory guard: estimate the in-core ERI allocation for a molecular job and
    // refuse (or offer integral-direct) before we spawn the worker and clear the
    // prompt. Periodic jobs are exempt (no nao⁴ in-core tensor). A LOCAL job is
    // judged here against this machine's RAM; a REMOTE job defers to the off-thread
    // submit, which probes the host and judges against ITS RAM (this machine's
    // budget would be the wrong yardstick), so it is not pre-flighted on this path.
    // Two-endpoint transition-state searches need a product structure; resolve it
    // from the chosen entry and reject early with a clear message if it is missing
    // (the panel can outlive the entry it pointed at).
    let ts_product = resolve_ts_product(state, &prompt);
    if prompt.kind == crate::engines::qm::QmKind::TransitionState
        && prompt.ts.route == crate::frontend::state::TsRouteKind::TwoEndpoint
    {
        if ts_product.is_none() {
            state.set_message(
                "choose a product structure for the two-endpoint transition-state search"
                    .to_string(),
            );
            return;
        }
        // The reactant is the active entry; identical endpoints give a degenerate
        // (zero-displacement) guess, so reject the active entry as its own product.
        if prompt.ts.product_entry == state.entries.active_entry_id() {
            state.set_message(
                "the product must be a different structure than the reactant (the active entry)"
                    .to_string(),
            );
            return;
        }
    }
    if !prompt.periodic && resolve_remote_host(state, &prompt.prefs.target).is_none() {
        let request = prompt.to_request(state.structure().clone(), ts_product.clone());
        let verdict = memory_verdict(&request, crate::backend::hardware::qm_incore_budget_bytes());
        if let Some(notification) = qm_memory_notification(&verdict, "this machine") {
            state.ui.notification = Some(notification);
            return; // leave pending_qm intact so the prompt stays open
        }
    }
    let job = prompt.to_job(state.structure().clone(), ts_product);
    let remote_host = resolve_remote_host(state, &prompt.prefs.target);
    state.set_source_path(None);
    state.ui.editor = None;
    state.ui.pending_qm = None;
    match remote_host {
        // A configured remote target: deploy + submit detached, tracked via the
        // job registry and the opt-in refresh — not the in-process worker.
        Some(host) => start_remote_qm(state, job, host, prompt.prefs.job_resources()),
        None => {
            reserve_qm_run_dir(state);
            let running = spawn_qm_job(job, Some(qm_thread_count(state, &prompt.prefs)));
            state.jobs.set_qm(running);
            if let Some(task_run_id) = state.active_task_run {
                state.tasks.mark_status(task_run_id, TaskStatus::Running);
            }
            state.set_message("QM calculation running; press Esc to stop".to_string());
        }
    }
}

/// Create the active QM task's run directory up front, which also records the
/// entry the run was launched from. A single-point energy surfaces its report on
/// that entry, so the anchor must be taken now: resolving it when the run
/// finishes would attach the report to whatever entry the user had activated by
/// then. The remote path gets this for free — it stages into the run directory
/// before submitting. Failures are logged, not fatal; the calculation still runs.
fn reserve_qm_run_dir(state: &mut AppState) {
    let kind = state
        .active_task_run
        .and_then(|task_run_id| state.tasks.task_run(task_run_id))
        .map(|task| task.kind);
    let Some(kind) = kind.filter(|kind| kind.is_qm()) else {
        return;
    };
    if let Err(error) = ensure_active_task_run_dir(state, kind, None) {
        state
            .output_log
            .push(format!("failed to create QM run directory: {error}"));
    }
}

/// Local QM thread count: the per-panel core request when set (`> 0`), otherwise
/// the global core cap; clamped to this machine's logical cores.
fn qm_thread_count(state: &AppState, prefs: &crate::frontend::state::ExecutionPrefs) -> usize {
    let requested = if prefs.cores_per_subtask > 0 {
        prefs.cores_per_subtask as usize
    } else {
        state.config.compute_core_count
    };
    requested.clamp(1, crate::backend::hardware::info().logical_cores)
}

/// The remote host a built-in compute job should run on for `target`, or `None`
/// for local. Resolves leniently: a dangling host id (a since-deleted host) falls
/// back to local rather than erroring. Shared by every task router (QM, docking,
/// MD), each passing its panel's chosen target.
pub(crate) fn resolve_remote_host(
    state: &AppState,
    target: &crate::backend::config::ComputeTarget,
) -> Option<crate::backend::config::RemoteHost> {
    use crate::backend::config::ComputeTarget;
    match target {
        ComputeTarget::Local => None,
        ComputeTarget::Remote(host_id) => state.config.remote_hosts.get(host_id).cloned(),
    }
}

/// The in-core RAM budget the panel's "Estimate memory" reports against, and a
/// label naming the host it belongs to. A remote target with a detected inventory
/// uses that host's RAM and label; otherwise this machine's RAM. The detected
/// inventory is best-effort (the settings "Detect" action fills it); the off-thread
/// submit re-probes and re-checks against the real host before launch regardless.
fn qm_incore_budget_and_location(
    state: &AppState,
    target: &crate::backend::config::ComputeTarget,
) -> (u64, String) {
    use crate::backend::hardware::{qm_incore_budget_bytes, qm_incore_budget_for};
    if let Some(host) = resolve_remote_host(state, target)
        && let Some(ram) = state
            .ui
            .settings
            .remote_hardware
            .get(&host.id)
            .and_then(|info| info.ram_bytes)
    {
        return (qm_incore_budget_for(ram), host.label);
    }
    (qm_incore_budget_bytes(), "this machine".to_string())
}

/// Launch a detached remote QM job on `host`. Resolves the core count
/// (per-job override → per-host default → app-wide; clamped to the host's probed
/// inventory off-thread) and hands off to [`start_remote_engine`].
fn start_remote_qm(
    state: &mut AppState,
    job: crate::engines::qm::QmJob,
    host: crate::backend::config::RemoteHost,
    mut resources: crate::backend::config::JobResources,
) {
    resources.cpus_per_task = Some(crate::frontend::remote_jobs::resolve_requested_cores(
        resources.cpus_per_task.map(|value| value as usize),
        &host,
        state.config.compute_core_count,
    ) as u32);
    start_remote_engine(state, host, crate::wire::Engine::Qm(job), resources);
}

pub(crate) fn cancel_pending_qm_request(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::QmPrompt);
    if state.jobs.qm_running() {
        let _ = crate::frontend::jobs::cancel_controlled_job(
            state,
            &crate::frontend::jobs::JobControlId::Local(crate::frontend::jobs::LocalJobSlot::Qm),
        );
        state.ui.pending_qm = None;
        state.set_message("QM calculation stopping".to_string());
        close_active_task_panel(state);
        return;
    }
    state.ui.pending_qm = None;
    state.set_message("QM calculation canceled".to_string());
    complete_active_qm_task(state, TaskStatus::Failed);
    close_active_task_panel(state);
}

/// Build the warning shown when a pending QM job would exceed the RAM budget.
/// `ExceedsCanDirect` offers a one-click switch to integral-direct; otherwise the
/// only path forward is editing the job, so the warning is acknowledge-only.
fn qm_memory_notification(verdict: &MemoryVerdict, location: &str) -> Option<Notification> {
    let detail = verdict.detail(location)?;
    let title = "This calculation may run out of memory";
    match verdict {
        // Unreachable: detail()? above already returned None for Ok; arm kept for exhaustiveness.
        MemoryVerdict::Ok => None,
        MemoryVerdict::ExceedsCanDirect { .. } => Some(Notification {
            severity: NotificationSeverity::Warning,
            title: title.into(),
            body: format!(
                "{detail} Integral-direct SCF runs the same single point with far less memory."
            ),
            buttons: vec![
                NotificationButton {
                    label: "Run with integral-direct".into(),
                    action: AppAction::StartQmWithDirectBackend,
                    primary: true,
                },
                NotificationButton {
                    label: "Cancel".into(),
                    action: AppAction::DismissNotification,
                    primary: false,
                },
            ],
        }),
        MemoryVerdict::ExceedsMustReduce { .. } => Some(Notification {
            severity: NotificationSeverity::Warning,
            title: title.into(),
            body: format!(
                "{detail} This calculation type needs in-core integrals — choose a smaller basis set or a smaller system."
            ),
            buttons: vec![NotificationButton {
                label: "OK".into(),
                action: AppAction::DismissNotification,
                primary: true,
            }],
        }),
    }
}

/// Memory-guard escape hatch: flip the pending job to integral-direct and re-run.
pub(crate) fn start_qm_with_direct_backend(state: &mut AppState) {
    if let Some(prompt) = state.ui.pending_qm.as_mut() {
        prompt.options.scf_backend = QmScfBackend::Direct;
    }
    start_pending_qm(state);
}

/// Estimate the pending molecular QM job's peak memory and stash it on the prompt
/// for the panel to display. Periodic jobs have no in-core ERI tensor to model,
/// so the panel hides the button for them and this no-ops if one slips through.
pub(crate) fn estimate_qm_memory(state: &mut AppState) {
    let Some(prompt) = state.ui.pending_qm.as_ref() else {
        return;
    };
    if prompt.periodic {
        return;
    }
    if state.structure().atoms.is_empty() {
        state.set_message("open a structure before estimating QM memory".to_string());
        return;
    }
    // The product does not change the in-core estimate (the guess shares the
    // reactant's composition), so estimate against the reactant alone.
    let prompt = prompt.clone();
    let request = prompt.to_request(state.structure().clone(), None);
    let signature = prompt.memory_signature(state.structure());
    let (budget, location) = qm_incore_budget_and_location(state, &prompt.prefs.target);
    match crate::engines::qm::estimate_request_memory(&request, budget) {
        Ok(report) => {
            if let Some(prompt) = state.ui.pending_qm.as_mut() {
                prompt.memory_report = Some(crate::frontend::state::QmMemoryEstimate {
                    report,
                    signature,
                    location,
                });
            }
        }
        Err(error) => {
            if let Some(prompt) = state.ui.pending_qm.as_mut() {
                prompt.memory_report = None;
            }
            state.set_message(format!("could not estimate QM memory: {error}"));
        }
    }
}

#[cfg(test)]
mod memory_guard_tests {
    use super::*;
    use crate::engines::qm::MemoryVerdict;

    #[test]
    fn notification_offers_direct_for_can_direct_only() {
        let can = MemoryVerdict::ExceedsCanDirect {
            estimate: 20_000_000_000,
            budget: 16_000_000_000,
        };
        let n = qm_memory_notification(&can, "this machine").expect("should warn");
        assert_eq!(n.buttons.len(), 2);
        assert!(matches!(
            n.buttons[0].action,
            AppAction::StartQmWithDirectBackend
        ));
        assert!(n.buttons[0].primary);

        let must = MemoryVerdict::ExceedsMustReduce {
            estimate: 20_000_000_000,
            budget: 16_000_000_000,
        };
        let n = qm_memory_notification(&must, "this machine").expect("should warn");
        assert!(
            !n.buttons
                .iter()
                .any(|b| matches!(b.action, AppAction::StartQmWithDirectBackend))
        );

        assert!(qm_memory_notification(&MemoryVerdict::Ok, "this machine").is_none());
    }

    #[test]
    fn start_with_direct_flips_backend_and_reruns() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let mut prompt = crate::frontend::state::QmPrompt::default();
        prompt.options.scf_backend = crate::engines::qm::QmScfBackend::InCore;
        state.ui.pending_qm = Some(prompt);
        start_qm_with_direct_backend(&mut state);
        // The handler flips the backend before re-running; with no atoms the
        // re-run no-ops, but the backend choice must have changed.
        // (pending_qm is cleared on a successful spawn; with an empty structure
        // start_pending_qm returns early, leaving pending_qm intact.)
        assert_eq!(
            state.ui.pending_qm.as_ref().unwrap().options.scf_backend,
            crate::engines::qm::QmScfBackend::Direct
        );
    }
}
