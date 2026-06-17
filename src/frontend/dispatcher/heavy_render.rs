//! The heavy-structure render gate.
//!
//! Rather than silently downgrading a very large structure to a simplified
//! "dots" view (which produced unpredictable visuals), the viewport holds the
//! full-detail render and offers the user a faster wireframe first. Declining
//! renders at full detail; the decision is remembered per entry so the prompt is
//! not raised again.

use crate::domain::AtomCategory;
use crate::frontend::actions::{AppAction, Notification, NotificationSeverity};
use crate::frontend::state::{AppState, AtomStyle};
use crate::frontend::viewport_defaults::{HEAVY_RENDER_ATOM_THRESHOLD, heavy_render_atom_count};

/// Re-evaluate the gate after an entry is shown. Clears any prior gate/prompt,
/// then — for an undecided, heavy active entry — gates its render and posts the
/// wireframe suggestion.
pub(crate) fn maybe_gate_heavy_render(state: &mut AppState) {
    // The gate currently owns the notification slot, so clearing it on every
    // (re)load also dismisses a stale prompt left over from another entry.
    state.ui.pending_heavy_gate = None;
    state.ui.notification = None;

    let Some(entry_id) = state.entries.active_entry_id() else {
        return;
    };
    if state.ui.heavy_render_decided.contains(&entry_id) {
        return;
    }
    let count = heavy_render_atom_count(state.structure(), &state.ui.viewport);
    if count <= HEAVY_RENDER_ATOM_THRESHOLD {
        return;
    }

    state.ui.pending_heavy_gate = Some(entry_id);
    state.ui.notification = Some(
        Notification::new(
            NotificationSeverity::Warning,
            "Large structure",
            format!(
                "{count} atoms would render in full detail, which may be slow. \
                 Show them as a faster wireframe instead?"
            ),
        )
        .button(
            "Use wireframe",
            true,
            AppAction::UseWireframeForHeavyEntry(entry_id),
        )
        .button(
            "Render in full detail",
            false,
            AppAction::RenderHeavyEntryAtFull(entry_id),
        ),
    );
}

/// Accept the suggestion: switch every atom of `entry_id` to wireframe, then let
/// it render. The decision is recorded even if the entry is no longer active, so
/// the prompt does not return.
pub(crate) fn use_wireframe_for_heavy_entry(state: &mut AppState, entry_id: u64) {
    if state.entries.active_entry_id() == Some(entry_id) {
        let items: Vec<(usize, AtomCategory)> = {
            let structure = state.structure();
            (0..structure.atoms.len())
                .map(|index| (index, structure.atom_category(index)))
                .collect()
        };
        let count = items.len();
        state
            .ui
            .viewport
            .apply_atom_styles(items, AtomStyle::Wireframe);
        state.set_message(format!("Showing {count} atom(s) as wireframe"));
    }
    state.ui.heavy_render_decided.insert(entry_id);
    state.ui.pending_heavy_gate = None;
}

/// Decline the suggestion: record the choice and let the entry render at full
/// detail. The render is never silently simplified.
pub(crate) fn render_heavy_entry_at_full(state: &mut AppState, entry_id: u64) {
    state.ui.heavy_render_decided.insert(entry_id);
    state.ui.pending_heavy_gate = None;
    if state.entries.active_entry_id() == Some(entry_id) {
        state.set_message("Rendering at full detail".to_string());
    }
}
