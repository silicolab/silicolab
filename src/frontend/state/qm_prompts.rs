use super::*;

use crate::frontend::AtomSelection;

#[derive(Debug, Clone, Copy)]
pub struct OptimizationPrompt {
    pub cell: crate::engines::forcefield::CellOptimizationOptions,
    pub coordinate_scope: CoordinateOptimizationScope,
    pub allow_cell_optimization: bool,
}

impl OptimizationPrompt {
    pub fn new(allow_cell_optimization: bool, selection: &AtomSelection) -> Self {
        Self {
            cell: if allow_cell_optimization {
                crate::engines::forcefield::CellOptimizationOptions::lengths_only()
            } else {
                crate::engines::forcefield::CellOptimizationOptions::default()
            },
            coordinate_scope: if selection.is_empty() {
                CoordinateOptimizationScope::AllAtoms
            } else {
                CoordinateOptimizationScope::SelectedAtoms
            },
            allow_cell_optimization,
        }
    }

    pub fn options(
        &self,
        selection: &AtomSelection,
    ) -> crate::engines::forcefield::OptimizationOptions {
        crate::engines::forcefield::OptimizationOptions {
            atoms: match self.coordinate_scope {
                CoordinateOptimizationScope::AllAtoms => {
                    crate::engines::forcefield::AtomOptimizationScope::All
                }
                CoordinateOptimizationScope::SelectedAtoms => {
                    crate::engines::forcefield::AtomOptimizationScope::Selected(
                        selection.ordered_indices(),
                    )
                }
            },
            cell: if self.allow_cell_optimization {
                self.cell
            } else {
                crate::engines::forcefield::CellOptimizationOptions::default()
            },
            ..crate::engines::forcefield::OptimizationOptions::default()
        }
    }
}

/// User-editable configuration for a quantum-chemistry (hartree) calculation.
#[derive(Debug, Clone)]
pub struct QmPrompt {
    pub method: crate::engines::qm::QmMethod,
    /// Free-text functional name backing the "custom functional" field. When the
    /// method dropdown selects "Custom functional…", the panel reads this into
    /// [`crate::engines::qm::QmMethod::Dft`].
    pub custom_functional: String,
    pub basis: String,
    pub charge: i32,
    pub multiplicity: u32,
    pub kind: crate::engines::qm::QmKind,
    /// The calculation type the task opened with. `kind` is user-editable in the
    /// panel; this stays fixed so re-opening the panel (e.g. on an entry switch)
    /// doesn't clobber the user's choice, while switching to a different QM task
    /// re-defaults the panel.
    pub default_kind: crate::engines::qm::QmKind,
    /// All advanced hartree options (dispersion, solvation, SCF backend, …).
    pub options: crate::engines::qm::QmOptions,
    /// Whether the panel is in periodic (crystalline) mode. Only selectable when
    /// the active structure carries a real unit cell; the molecular fields above
    /// are ignored while this is set.
    pub periodic: bool,
    /// Settings for a periodic calculation, used when [`Self::periodic`] is set.
    pub periodic_form: PeriodicQmForm,
}

/// Panel form for a periodic (PBC) QM calculation — the periodic counterpart of
/// the molecular fields on [`QmPrompt`]. Mirrors
/// [`crate::engines::qm::PeriodicQmRequest`] minus the structure.
#[derive(Debug, Clone)]
pub struct PeriodicQmForm {
    pub functional: crate::engines::qm::PeriodicFunctional,
    pub basis: String,
    pub kmesh: [u32; 3],
    pub e_cut_ry: f64,
    pub max_iter: u32,
    pub forces: bool,
    pub stress: bool,
}

impl Default for PeriodicQmForm {
    fn default() -> Self {
        use crate::engines::qm::periodic;
        Self {
            functional: crate::engines::qm::PeriodicFunctional::default(),
            basis: periodic::DEFAULT_PERIODIC_BASIS.to_string(),
            kmesh: [1, 1, 1],
            e_cut_ry: periodic::DEFAULT_E_CUT_RY,
            max_iter: periodic::DEFAULT_MAX_ITER,
            forces: false,
            stress: false,
        }
    }
}

impl QmPrompt {
    pub fn new(kind: crate::engines::qm::QmKind) -> Self {
        Self {
            // Default to r2scan-3c: a robust, batteries-included production
            // composite (functional + basis + dispersion + corrections).
            method: crate::engines::qm::QmMethod::Composite("r2scan-3c".to_string()),
            custom_functional: String::new(),
            basis: "def2-svp".to_string(),
            charge: 0,
            multiplicity: 1,
            kind,
            default_kind: kind,
            options: crate::engines::qm::QmOptions::default(),
            periodic: false,
            periodic_form: PeriodicQmForm::default(),
        }
    }

    /// Build the molecular engine request from this form against `structure`.
    pub fn to_request(&self, structure: crate::domain::Structure) -> crate::engines::qm::QmRequest {
        crate::engines::qm::QmRequest {
            structure,
            method: self.method.clone(),
            basis: self.basis.clone(),
            charge: self.charge,
            multiplicity: self.multiplicity,
            kind: self.kind,
            options: self.options.clone(),
        }
    }

    /// Build the engine job from this form against `structure`: a periodic job in
    /// periodic mode, otherwise the molecular request.
    pub fn to_job(&self, structure: crate::domain::Structure) -> crate::engines::qm::QmJob {
        use crate::engines::qm::{KMesh, PeriodicQmRequest, QmJob};
        if self.periodic {
            let form = &self.periodic_form;
            QmJob::Periodic(PeriodicQmRequest {
                structure,
                functional: form.functional,
                basis: form.basis.clone(),
                kmesh: KMesh {
                    divisions: form.kmesh,
                },
                e_cut_ry: form.e_cut_ry,
                max_iter: form.max_iter,
                forces: form.forces,
                stress: form.stress,
            })
        } else {
            QmJob::Molecular(self.to_request(structure))
        }
    }
}

impl Default for QmPrompt {
    fn default() -> Self {
        Self::new(crate::engines::qm::QmKind::SinglePoint)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SupercellPrompt {
    pub repeats: [u32; 3],
}

/// User-editable configuration for the Protein Preparation task. This round
/// exposes only hydrogen completion; the other fields are placeholders for
/// future steps (protonation states, terminus patching, missing-atom repair)
/// and are not yet wired.
#[derive(Debug, Clone, Copy)]
pub struct ProteinPrepPrompt {
    /// Add missing hydrogens with chemistry heuristics.
    pub add_hydrogens: bool,
}

impl Default for ProteinPrepPrompt {
    fn default() -> Self {
        Self {
            add_hydrogens: true,
        }
    }
}
