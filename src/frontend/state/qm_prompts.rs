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

/// Which near-saddle guess the transition-state panel builds. The UI-side mirror
/// of [`crate::engines::qm::QmTsGuess`]'s variant choice (kept separate because
/// the panel holds the product as an *entry id*, resolved to a structure only at
/// launch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TsRouteKind {
    /// Climb from the current geometry.
    #[default]
    Single,
    /// Reactant → product guess (IDPP or NEB).
    TwoEndpoint,
    /// Distinguished-coordinate scan.
    CoordinateScan,
}

/// Internal-coordinate type for the panel's distinguished-coordinate scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanCoordKind {
    #[default]
    Bond,
    Angle,
    Dihedral,
}

impl ScanCoordKind {
    /// How many atom indices this coordinate uses (2 / 3 / 4).
    pub fn arity(self) -> usize {
        match self {
            ScanCoordKind::Bond => 2,
            ScanCoordKind::Angle => 3,
            ScanCoordKind::Dihedral => 4,
        }
    }
}

/// Panel-side editing state for a transition-state search. Resolved into a
/// [`crate::engines::qm::QmTsConfig`] by [`QmPrompt::to_request`].
#[derive(Debug, Clone)]
pub struct TsPromptForm {
    pub algorithm: crate::engines::qm::QmTsAlgorithm,
    pub coordinates: crate::engines::qm::QmTsCoordinates,
    pub confirm_irc: bool,
    pub route: TsRouteKind,
    /// Two-endpoint route: the product structure, chosen from the open entries.
    pub product_entry: Option<u64>,
    pub use_neb: bool,
    pub neb_images: u32,
    pub neb_climbing: bool,
    pub map_atoms: bool,
    /// IDPP route: scan this many path points for the energy peak; `0` = single
    /// geometric image.
    pub idpp_scan_points: u32,
    /// Coordinate-scan route.
    pub scan_kind: ScanCoordKind,
    /// 1-based atom indices; the first [`ScanCoordKind::arity`] are used.
    pub scan_atoms: [u32; 4],
    /// Scan range start/end (Ångström for a bond, degrees for an angle/dihedral).
    pub scan_start: f64,
    pub scan_end: f64,
    pub scan_points: u32,
}

impl Default for TsPromptForm {
    fn default() -> Self {
        Self {
            algorithm: crate::engines::qm::QmTsAlgorithm::default(),
            coordinates: crate::engines::qm::QmTsCoordinates::default(),
            confirm_irc: false,
            route: TsRouteKind::default(),
            product_entry: None,
            use_neb: false,
            neb_images: 8,
            neb_climbing: true,
            map_atoms: true,
            idpp_scan_points: 0,
            scan_kind: ScanCoordKind::default(),
            scan_atoms: [1, 2, 3, 4],
            scan_start: 1.0,
            scan_end: 3.0,
            scan_points: 7,
        }
    }
}

impl TsPromptForm {
    /// Resolve into the engine's [`QmTsConfig`]. `product` is the structure the
    /// chosen [`Self::product_entry`] resolves to (the caller fetches it from the
    /// open entries); when the two-endpoint route is selected but no product is
    /// available the route degrades to a single guess, so a memory estimate or a
    /// half-filled form never fails to build.
    pub fn to_config(
        &self,
        product: Option<crate::domain::Structure>,
    ) -> crate::engines::qm::QmTsConfig {
        use crate::engines::qm::{
            QmInternalCoordinate, QmTsConfig, QmTsCoordinateScan, QmTsEndpoints, QmTsGuess,
        };
        let guess = match self.route {
            TsRouteKind::Single => QmTsGuess::Single,
            TsRouteKind::TwoEndpoint => match product {
                Some(product) => {
                    let mut endpoints = QmTsEndpoints::new(product);
                    endpoints.use_neb = self.use_neb;
                    endpoints.scan_points =
                        (self.idpp_scan_points >= 3).then_some(self.idpp_scan_points as usize);
                    endpoints.neb_images = self.neb_images.max(1) as usize;
                    endpoints.neb_climbing = self.neb_climbing;
                    endpoints.map_atoms = self.map_atoms;
                    QmTsGuess::TwoEndpoint(Box::new(endpoints))
                }
                None => QmTsGuess::Single,
            },
            TsRouteKind::CoordinateScan => {
                let a = self.scan_atoms.map(|i| i as usize);
                let coordinate = match self.scan_kind {
                    ScanCoordKind::Bond => QmInternalCoordinate::Bond(a[0], a[1]),
                    ScanCoordKind::Angle => QmInternalCoordinate::Angle(a[0], a[1], a[2]),
                    ScanCoordKind::Dihedral => {
                        QmInternalCoordinate::Dihedral(a[0], a[1], a[2], a[3])
                    }
                };
                QmTsGuess::CoordinateScan(QmTsCoordinateScan {
                    coordinate,
                    start: self.scan_start,
                    end: self.scan_end,
                    n_points: self.scan_points.max(3) as usize,
                })
            }
        };
        QmTsConfig {
            guess,
            algorithm: self.algorithm,
            coordinates: self.coordinates,
            confirm_irc: self.confirm_irc,
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
    /// Transition-state search form; consulted when `kind` is
    /// [`crate::engines::qm::QmKind::TransitionState`].
    pub ts: TsPromptForm,
    /// Whether the panel is in periodic (crystalline) mode. Only selectable when
    /// the active structure carries a real unit cell; the molecular fields above
    /// are ignored while this is set.
    pub periodic: bool,
    /// Settings for a periodic calculation, used when [`Self::periodic`] is set.
    pub periodic_form: PeriodicQmForm,
    /// Last on-demand memory estimate (the panel's "Estimate memory" button).
    /// `None` until the user asks; hidden by the panel once the live config drifts
    /// from the fingerprint the estimate was computed for.
    pub memory_report: Option<QmMemoryEstimate>,
    /// Where the job runs plus its resource envelope (compute target, CPU cores),
    /// seeded from the global defaults when the panel opens.
    pub prefs: ExecutionPrefs,
}

/// A memory estimate plus the input fingerprint it was computed for, so the panel
/// never shows a figure computed for one configuration against another.
#[derive(Debug, Clone)]
pub struct QmMemoryEstimate {
    pub report: crate::engines::qm::QmMemoryReport,
    /// [`QmPrompt::memory_signature`] of the inputs at estimate time.
    pub signature: u64,
    /// Host the budget belongs to ("this machine" or a remote host's label),
    /// captured at estimate time so the displayed label matches `report.budget_bytes`.
    pub location: String,
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
        use crate::engines::qm::{QmDispersion, QmKind, QmMethod, QmOptions};
        // Production defaults: B3LYP-D3(BJ), with def2-SVP for geometry
        // optimization / frequencies and the larger def2-TZVP for single-point
        // energies (where the higher cost buys a noticeably better number).
        let basis = match kind {
            QmKind::SinglePoint => "def2-tzvp",
            QmKind::Optimize | QmKind::Frequencies | QmKind::TransitionState => "def2-svp",
        };
        Self {
            method: QmMethod::Dft("b3lyp".to_string()),
            custom_functional: String::new(),
            basis: basis.to_string(),
            charge: 0,
            multiplicity: 1,
            kind,
            default_kind: kind,
            options: QmOptions {
                dispersion: Some(QmDispersion::D3Bj),
                ..QmOptions::default()
            },
            ts: TsPromptForm::default(),
            periodic: false,
            periodic_form: PeriodicQmForm::default(),
            memory_report: None,
            prefs: ExecutionPrefs::default(),
        }
    }

    /// Build the molecular engine request from this form against `structure`.
    /// `product` is the resolved product structure for a two-endpoint
    /// transition-state search (the caller fetches it from the open entries);
    /// pass `None` for every other calculation.
    pub fn to_request(
        &self,
        structure: crate::domain::Structure,
        product: Option<crate::domain::Structure>,
    ) -> crate::engines::qm::QmRequest {
        use crate::engines::qm::QmKind;
        let ts = (self.kind == QmKind::TransitionState).then(|| self.ts.to_config(product));
        crate::engines::qm::QmRequest {
            structure,
            method: self.method.clone(),
            basis: self.basis.clone(),
            charge: self.charge,
            multiplicity: self.multiplicity,
            kind: self.kind,
            options: self.options.clone(),
            ts,
        }
    }

    /// Build the engine job from this form against `structure`: a periodic job in
    /// periodic mode, otherwise the molecular request. `product` carries the
    /// two-endpoint TS product (see [`Self::to_request`]).
    pub fn to_job(
        &self,
        structure: crate::domain::Structure,
        product: Option<crate::domain::Structure>,
    ) -> crate::engines::qm::QmJob {
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
            QmJob::Molecular(self.to_request(structure, product))
        }
    }

    /// A cheap fingerprint of the inputs that drive the in-core memory estimate,
    /// used to detect when a shown estimate has gone stale. Covers the orbital
    /// count and backend dispatch (method, basis, charge, spin, kind, SCF backend,
    /// frozen-core/RI/grid) plus the elements present; geometry-only moves (which
    /// barely shift the estimate) are deliberately not tracked.
    pub fn memory_signature(&self, structure: &crate::domain::Structure) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let options = &self.options;
        self.method.label().hash(&mut hasher);
        self.basis.hash(&mut hasher);
        self.charge.hash(&mut hasher);
        self.multiplicity.hash(&mut hasher);
        (self.kind as u8).hash(&mut hasher);
        options.scf_backend.label().hash(&mut hasher);
        options.all_electron.hash(&mut hasher);
        options.ri_mp2.hash(&mut hasher);
        options.grid_level.hash(&mut hasher);
        structure.atoms.len().hash(&mut hasher);
        for atom in &structure.atoms {
            atom.element.hash(&mut hasher);
        }
        hasher.finish()
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

#[cfg(test)]
mod tests {
    use super::QmPrompt;
    use crate::domain::{Atom, Structure};
    use crate::engines::qm::{QmDispersion, QmKind, QmMethod};
    use nalgebra::Point3;

    fn water() -> Structure {
        Structure::new(
            "water",
            vec![
                Atom {
                    element: "O".into(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".into(),
                    position: Point3::new(0.0, 0.76, 0.59),
                    charge: 0.0,
                },
                Atom {
                    element: "H".into(),
                    position: Point3::new(0.0, -0.76, 0.59),
                    charge: 0.0,
                },
            ],
        )
    }

    #[test]
    fn memory_signature_tracks_estimate_inputs_only() {
        let structure = water();
        let base = QmPrompt::new(QmKind::SinglePoint);
        let base_sig = base.memory_signature(&structure);

        // A basis change moves the orbital count → the estimate is stale.
        let mut other = base.clone();
        other.basis = "cc-pvqz".into();
        assert_ne!(base_sig, other.memory_signature(&structure));

        // A solvent has no modeled memory cost → the estimate still applies.
        let mut solvated = base.clone();
        solvated.options.solvation = Some(crate::engines::qm::QmSolvation::Smd("water".into()));
        assert_eq!(base_sig, solvated.memory_signature(&structure));
    }

    #[test]
    fn defaults_are_b3lyp_d3bj_with_kind_specific_basis() {
        // Single point → the larger def2-TZVP; optimize/freq → def2-SVP. Both run
        // B3LYP-D3(BJ).
        for (kind, basis) in [
            (QmKind::SinglePoint, "def2-tzvp"),
            (QmKind::Optimize, "def2-svp"),
            (QmKind::Frequencies, "def2-svp"),
            (QmKind::TransitionState, "def2-svp"),
        ] {
            let prompt = QmPrompt::new(kind);
            assert_eq!(prompt.method, QmMethod::Dft("b3lyp".to_string()));
            assert_eq!(prompt.options.dispersion, Some(QmDispersion::D3Bj));
            assert_eq!(prompt.basis, basis, "wrong default basis for {kind:?}");
            assert!(prompt.memory_report.is_none());
        }
    }
}
