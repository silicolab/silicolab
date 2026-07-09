use super::*;

use crate::frontend::actions::ResidueSelectionMode;

pub(crate) fn select_residue(state: &mut AppState, residue_index: usize, toggle: bool) {
    let atom_count = state.structure().atoms.len();
    let Some(atom_indices) = residue_atom_indices(state, residue_index, atom_count) else {
        state.ui.selection.retain_valid(atom_count);
        state.set_message("Residue selection is unavailable for the active entry".to_string());
        return;
    };

    select_residue_atoms(
        state,
        atom_indices,
        if toggle {
            ResidueSelectionMode::Toggle
        } else {
            ResidueSelectionMode::Replace
        },
        "Selected residue has no valid atoms",
    );
}

pub(crate) fn select_residue_range(
    state: &mut AppState,
    chain_id: char,
    start: usize,
    end: usize,
    toggle: bool,
) {
    let atom_count = state.structure().atoms.len();
    let Some(atom_indices) = residue_range_atom_indices(state, chain_id, start, end, atom_count)
    else {
        state.ui.selection.retain_valid(atom_count);
        state
            .set_message("Residue range selection is unavailable for the active entry".to_string());
        return;
    };

    select_residue_atoms(
        state,
        atom_indices,
        if toggle {
            ResidueSelectionMode::Toggle
        } else {
            ResidueSelectionMode::Replace
        },
        "Selected residue range has no valid atoms",
    );
}

pub(crate) fn select_residues(
    state: &mut AppState,
    residue_indices: Vec<usize>,
    mode: ResidueSelectionMode,
) {
    let atom_count = state.structure().atoms.len();
    let Some(atom_indices) = residue_set_atom_indices(state, residue_indices, atom_count) else {
        state.ui.selection.retain_valid(atom_count);
        state.set_message("Residue selection is unavailable for the active entry".to_string());
        return;
    };

    select_residue_atoms(
        state,
        atom_indices,
        mode,
        "Selected residues have no valid atoms",
    );
}

fn select_residue_atoms(
    state: &mut AppState,
    atom_indices: Vec<usize>,
    mode: ResidueSelectionMode,
    empty_message: &str,
) {
    let atom_count = state.structure().atoms.len();
    if atom_indices.is_empty() {
        state.ui.selection.retain_valid(atom_count);
        state.set_message(empty_message.to_string());
        return;
    }

    match mode {
        ResidueSelectionMode::Replace => state.ui.selection.select_indices(atom_indices),
        ResidueSelectionMode::Add => {
            for atom_index in atom_indices {
                state.ui.selection.add(atom_index);
            }
        }
        ResidueSelectionMode::Toggle => {
            for atom_index in atom_indices {
                state.ui.selection.toggle(atom_index);
            }
        }
        ResidueSelectionMode::Remove => {
            for atom_index in atom_indices {
                state.ui.selection.remove(atom_index);
            }
        }
    }
    state.ui.selection.retain_valid(atom_count);

    if state.ui.selection.is_empty() {
        state.set_message("Cleared atom selection".to_string());
    } else {
        state.set_message(format!("Selected {} atom(s)", state.ui.selection.len()));
    }
}

fn residue_set_atom_indices(
    state: &AppState,
    residue_indices: Vec<usize>,
    atom_count: usize,
) -> Option<Vec<usize>> {
    let biopolymer = state.structure().biopolymer.as_ref()?;
    if !biopolymer.is_compatible_with_atom_count(atom_count) {
        return None;
    }

    let mut deduped = std::collections::BTreeSet::new();
    Some(
        residue_indices
            .into_iter()
            .filter(|residue_index| deduped.insert(*residue_index))
            .filter_map(|residue_index| biopolymer.residues.get(residue_index))
            .flat_map(|residue| valid_residue_atoms(residue, atom_count))
            .collect(),
    )
}

fn residue_atom_indices(
    state: &AppState,
    residue_index: usize,
    atom_count: usize,
) -> Option<Vec<usize>> {
    let biopolymer = state.structure().biopolymer.as_ref()?;
    if !biopolymer.is_compatible_with_atom_count(atom_count) {
        return None;
    }
    let residue = biopolymer.residues.get(residue_index)?;
    Some(valid_residue_atoms(residue, atom_count))
}

fn residue_range_atom_indices(
    state: &AppState,
    chain_id: char,
    start: usize,
    end: usize,
    atom_count: usize,
) -> Option<Vec<usize>> {
    let biopolymer = state.structure().biopolymer.as_ref()?;
    if !biopolymer.is_compatible_with_atom_count(atom_count) {
        return None;
    }
    let chain = biopolymer
        .chains
        .iter()
        .find(|chain| chain.id == chain_id)?;
    let start_pos = chain
        .residue_indices
        .iter()
        .position(|&residue_index| residue_index == start)?;
    let end_pos = chain
        .residue_indices
        .iter()
        .position(|&residue_index| residue_index == end)?;
    let (lo, hi) = if start_pos <= end_pos {
        (start_pos, end_pos)
    } else {
        (end_pos, start_pos)
    };

    Some(
        chain.residue_indices[lo..=hi]
            .iter()
            .filter_map(|&residue_index| biopolymer.residues.get(residue_index))
            .flat_map(|residue| valid_residue_atoms(residue, atom_count))
            .collect(),
    )
}

fn valid_residue_atoms(
    residue: &crate::domain::biopolymer::ResidueRecord,
    atom_count: usize,
) -> Vec<usize> {
    residue
        .atom_indices
        .iter()
        .copied()
        .filter(|&atom_index| atom_index < atom_count)
        .collect()
}

#[cfg(test)]
mod tests {
    use eframe::egui::Context;
    use nalgebra::Point3;

    use crate::{
        backend::project::WorkspaceSession,
        domain::{Atom, PdbAtomAnnotation, Structure, build_biopolymer},
        frontend::{
            actions::{AppAction, ResidueSelectionMode},
            state::AppState,
        },
    };

    fn annotated_atom(element: &str, x: f32) -> Atom {
        Atom {
            element: element.to_string(),
            position: Point3::new(x, 0.0, 0.0),
            charge: 0.0,
        }
    }

    fn annotation(atom_name: &str, residue_name: &str, residue_seq: i32) -> PdbAtomAnnotation {
        annotation_on_chain(atom_name, residue_name, 'A', residue_seq)
    }

    fn annotation_on_chain(
        atom_name: &str,
        residue_name: &str,
        chain_id: char,
        residue_seq: i32,
    ) -> PdbAtomAnnotation {
        PdbAtomAnnotation {
            atom_name: atom_name.to_string(),
            residue_name: residue_name.to_string(),
            chain_id,
            residue_seq,
            insertion_code: ' ',
        }
    }

    fn residue_structure() -> Structure {
        let mut structure = Structure::new(
            "protein",
            vec![
                annotated_atom("N", 0.0),
                annotated_atom("C", 1.0),
                annotated_atom("C", 2.0),
                annotated_atom("O", 3.0),
            ],
        );
        let annotations = vec![
            annotation("N", "ALA", 1),
            annotation("CA", "ALA", 1),
            annotation("C", "GLY", 2),
            annotation("O", "GLY", 2),
        ];
        structure.biopolymer = build_biopolymer(&annotations, Vec::new());
        structure
    }

    fn interleaved_chain_structure() -> Structure {
        let mut structure = Structure::new(
            "interleaved protein",
            vec![
                annotated_atom("N", 0.0),
                annotated_atom("C", 1.0),
                annotated_atom("N", 2.0),
                annotated_atom("C", 3.0),
                annotated_atom("N", 4.0),
                annotated_atom("C", 5.0),
            ],
        );
        let annotations = vec![
            annotation_on_chain("N", "ALA", 'A', 1),
            annotation_on_chain("CA", "ALA", 'A', 1),
            annotation_on_chain("N", "GLY", 'B', 1),
            annotation_on_chain("CA", "GLY", 'B', 1),
            annotation_on_chain("N", "SER", 'A', 2),
            annotation_on_chain("CA", "SER", 'A', 2),
        ];
        structure.biopolymer = build_biopolymer(&annotations, Vec::new());
        structure
    }

    fn scratch_state(structure: Structure) -> AppState {
        AppState::new(
            structure,
            None,
            WorkspaceSession::Scratch,
            Default::default(),
            Vec::new(),
            None,
        )
    }

    #[test]
    fn select_residue_selects_all_atoms_in_residue() {
        let ctx = Context::default();
        let mut state = scratch_state(residue_structure());
        let fingerprint = state.entries_fingerprint();

        crate::frontend::dispatcher::dispatch(
            &mut state,
            AppAction::SelectResidue {
                residue_index: 1,
                toggle: false,
            },
            &ctx,
        );

        assert_eq!(state.ui.selection.ordered_indices(), vec![2, 3]);
        assert_eq!(state.entries_fingerprint(), fingerprint);
    }

    #[test]
    fn select_residue_invalid_index_does_not_panic() {
        let ctx = Context::default();
        let mut state = scratch_state(residue_structure());
        state.ui.selection.select_indices([0, 99]);

        crate::frontend::dispatcher::dispatch(
            &mut state,
            AppAction::SelectResidue {
                residue_index: 99,
                toggle: false,
            },
            &ctx,
        );

        assert_eq!(state.ui.selection.ordered_indices(), vec![0]);
    }

    #[test]
    fn select_residue_range_selects_atoms_in_inclusive_range() {
        let ctx = Context::default();
        let mut state = scratch_state(residue_structure());
        let fingerprint = state.entries_fingerprint();

        crate::frontend::dispatcher::dispatch(
            &mut state,
            AppAction::SelectResidueRange {
                chain_id: 'A',
                start: 1,
                end: 0,
                toggle: false,
            },
            &ctx,
        );

        assert_eq!(state.ui.selection.ordered_indices(), vec![0, 1, 2, 3]);
        assert_eq!(state.entries_fingerprint(), fingerprint);
    }

    #[test]
    fn select_residue_range_toggle_updates_existing_selection() {
        let ctx = Context::default();
        let mut state = scratch_state(residue_structure());
        state.ui.selection.select_indices([0, 2]);

        crate::frontend::dispatcher::dispatch(
            &mut state,
            AppAction::SelectResidueRange {
                chain_id: 'A',
                start: 0,
                end: 1,
                toggle: true,
            },
            &ctx,
        );

        assert_eq!(state.ui.selection.ordered_indices(), vec![1, 3]);
    }

    #[test]
    fn select_residue_range_stays_within_chain_record() {
        let ctx = Context::default();
        let mut state = scratch_state(interleaved_chain_structure());

        crate::frontend::dispatcher::dispatch(
            &mut state,
            AppAction::SelectResidueRange {
                chain_id: 'A',
                start: 0,
                end: 2,
                toggle: false,
            },
            &ctx,
        );

        assert_eq!(state.ui.selection.ordered_indices(), vec![0, 1, 4, 5]);
    }

    #[test]
    fn select_residues_supports_add_and_remove_modes() {
        let ctx = Context::default();
        let mut state = scratch_state(residue_structure());

        crate::frontend::dispatcher::dispatch(
            &mut state,
            AppAction::SelectResidues {
                residue_indices: vec![0],
                mode: ResidueSelectionMode::Replace,
            },
            &ctx,
        );
        assert_eq!(state.ui.selection.ordered_indices(), vec![0, 1]);

        crate::frontend::dispatcher::dispatch(
            &mut state,
            AppAction::SelectResidues {
                residue_indices: vec![1],
                mode: ResidueSelectionMode::Add,
            },
            &ctx,
        );
        assert_eq!(state.ui.selection.ordered_indices(), vec![0, 1, 2, 3]);

        crate::frontend::dispatcher::dispatch(
            &mut state,
            AppAction::SelectResidues {
                residue_indices: vec![0],
                mode: ResidueSelectionMode::Remove,
            },
            &ctx,
        );
        assert_eq!(state.ui.selection.ordered_indices(), vec![2, 3]);
    }

    #[test]
    fn select_residues_deduplicates_toggle_input() {
        let ctx = Context::default();
        let mut state = scratch_state(residue_structure());
        state.ui.selection.select_indices([0, 2]);

        crate::frontend::dispatcher::dispatch(
            &mut state,
            AppAction::SelectResidues {
                residue_indices: vec![0, 0, 1],
                mode: ResidueSelectionMode::Toggle,
            },
            &ctx,
        );

        assert_eq!(state.ui.selection.ordered_indices(), vec![1, 3]);
    }
}
