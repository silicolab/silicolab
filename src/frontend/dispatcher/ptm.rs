//! Dispatcher logic for the Modify Protein (PTM) task. The panel edits a
//! [`PendingPtm`] draft; this turns it into a [`PtmRequest`] and routes it
//! through the shared [`apply_ptm`] seam — the same entry point the `.sls`
//! console verbs use — so the modification dispatch lives in exactly one place.

use anyhow::{Result, bail};

use crate::domain::ResidueId;
use crate::frontend::entry_ref::parse_anchor;
use crate::frontend::ptm_commands::{PtmRequest, apply_ptm};
use crate::frontend::state::{PendingPtm, PtmUiKind};

use super::*;

/// Apply an edit to the PTM draft, if one is present (mirrors
/// [`with_disorder_prompt`]).
pub(crate) fn with_ptm_prompt(state: &mut AppState, edit: impl FnOnce(&mut PendingPtm)) {
    if let Some(prompt) = state.ui.pending_ptm.as_mut() {
        edit(prompt);
    }
}

/// Apply the drafted modification to the active protein, adding the product as a
/// new entry. PTM builds are instant, so this runs synchronously and surfaces
/// failures as a status message while keeping the panel open.
pub(crate) fn start_pending_ptm(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::PtmPrompt);
    let Some(prompt) = state.ui.pending_ptm.clone() else {
        return;
    };
    let request = match build_ptm_request(&prompt) {
        Ok(request) => request,
        Err(error) => {
            state.status_neutral(error.to_string());
            return;
        }
    };
    match apply_ptm(state, "active", request, Some(&prompt.output_name)) {
        Ok(outcome) => {
            state.ui.pending_ptm = None;
            let entry_id = outcome.entry_id;
            let detail = outcome
                .detail
                .map(|detail| format!(" ({detail})"))
                .unwrap_or_default();
            state.status_success(format!(
                "{} applied as entry #{entry_id}{detail}",
                prompt.family.label()
            ));
            complete_active_task(state, TaskKind::ModifyProteinPtm, TaskStatus::Completed);
            close_active_task_panel(state);
        }
        Err(error) => state.status_error(format!("modification failed: {error}")),
    }
}

pub(crate) fn cancel_pending_ptm_request(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::PtmPrompt);
    state.ui.pending_ptm = None;
    state.status_neutral("Protein modification canceled".to_string());
    complete_active_task(state, TaskKind::ModifyProteinPtm, TaskStatus::Failed);
    close_active_task_panel(state);
}

/// Build the request from the draft. The anchor residue is parsed through the
/// shared [`parse_anchor`] grammar so chain validation matches the console.
fn build_ptm_request(prompt: &PendingPtm) -> Result<PtmRequest> {
    let residue = ptm_residue(prompt)?;
    Ok(match prompt.family {
        PtmUiKind::Phosphorylate => PtmRequest::Phosphorylate { residue },
        PtmUiKind::Acetylate => PtmRequest::Acetylate {
            residue: Some(residue),
            n_terminal: prompt.n_terminal,
        },
        PtmUiKind::Methylate => PtmRequest::Methylate {
            residue,
            degree: prompt.degree,
        },
        PtmUiKind::Lipidate => PtmRequest::Lipidate {
            residue,
            kind: prompt.lipid,
        },
        PtmUiKind::Ubiquitinate => PtmRequest::Ubiquitinate {
            residue,
            ubl: prompt.ubl,
            with_entry: prompt.ubl_override,
        },
        PtmUiKind::Glycosylate => {
            let iupac = prompt.glycan_iupac.trim();
            if iupac.is_empty() {
                bail!("glycosylation requires an IUPAC-condensed glycan notation");
            }
            PtmRequest::Glycosylate {
                residue,
                iupac: iupac.to_string(),
                kind: prompt.glyco_kind,
                root_anomer: prompt.glyco_root_anomer,
            }
        }
    })
}

fn ptm_residue(prompt: &PendingPtm) -> Result<ResidueId> {
    parse_anchor(&format!("{}:{}", prompt.chain.trim(), prompt.res_seq))
}
