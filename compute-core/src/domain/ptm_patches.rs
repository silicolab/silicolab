//! PTM → native CHARMM36 modified-residue mapping, the post-translational
//! analogue of [`glycan::patches`](crate::domain::glycan::patches).
//!
//! Unlike a glycan — which has no native single-residue protein representation
//! and is welded on as a separately typed moleculetype with an approximate
//! junction charge patch — phospho/acetyl/methyl modifications correspond to
//! *complete* CHARMM36 modified residues (SEP, TPO, PTR, ALY, MLZ, MLY, M3L, …).
//! Their force-field charges are redistributed across the whole residue and the
//! protonation state is fixed by the force field, so they cannot be decomposed
//! into "unmodified residue + separable group + junction patch" without
//! fabricating the split. The scientifically correct mapping is therefore to
//! rename the modified residue to its native name and let the rtp own the
//! parameters. This module only fixes that mapping; the structural rename lives
//! in the engine layer.

use std::fmt;

use crate::domain::ProteinAnchor;
use crate::domain::modification::{MethylDegree, PtmKind};

/// A native CHARMM36 modified residue a PTM maps onto: the residue name the
/// force field knows, the host anchor heavy atom carrying the junction, the
/// modifying-group atom name(s) bonded to that anchor (in the rtp's own
/// labelling), and the documented integral net charge of the rtp residue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtmResidue {
    pub charmm_name: &'static str,
    pub anchor_atom: &'static str,
    pub junction_partners: &'static [&'static str],
    pub net_charge: i32,
}

/// Why a requested PTM cannot be made MD-ready from the bundled force field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtmSupportError {
    /// The bundled CHARMM36 force field carries no parameters for this group, so
    /// MD is gated rather than fabricated (lipidation, prenylation, ubiquitin-like).
    RequiresForceFieldAssets { detail: String },
    /// The native residue exists in the force field but its structural rename is
    /// not implemented.
    RenameNotWired {
        native: &'static str,
        detail: String,
    },
}

impl fmt::Display for PtmSupportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PtmSupportError::RequiresForceFieldAssets { detail } => {
                write!(f, "requires force-field assets: {detail}")
            }
            PtmSupportError::RenameNotWired { native, detail } => {
                write!(
                    f,
                    "requires force-field assets: native residue {native} exists but {detail}"
                )
            }
        }
    }
}

impl std::error::Error for PtmSupportError {}

/// Resolve the native CHARMM36 modified residue a `(kind, anchor)` maps onto, or
/// the reason it is gated. Phospho-Ser/Thr/Tyr, acetyl-Lys, and methyl-Lys are
/// supported; methyl-Arg, phospho-His, N-terminal acetyl, and the lipid/prenyl/
/// ubiquitin-like families are gated with a clear, honest reason.
pub fn native_ptm_residue(
    kind: PtmKind,
    anchor: ProteinAnchor,
) -> Result<PtmResidue, PtmSupportError> {
    match kind {
        PtmKind::Phosphoryl => phospho_residue(anchor),
        PtmKind::Acetyl { n_terminal } => acetyl_residue(n_terminal, anchor),
        PtmKind::Methyl { degree } => methyl_residue(degree, anchor),
        PtmKind::Acyl(_) => Err(PtmSupportError::RequiresForceFieldAssets {
            detail: "lipidation (palmitoyl/myristoyl) has no bundled CHARMM36 lipid parameters"
                .to_string(),
        }),
        PtmKind::Prenyl(_) => Err(PtmSupportError::RequiresForceFieldAssets {
            detail: "prenylation (farnesyl/geranylgeranyl) has no bundled CHARMM36 parameters"
                .to_string(),
        }),
        PtmKind::Ubl(_) => Err(PtmSupportError::RequiresForceFieldAssets {
            detail: "ubiquitin-like conjugation is a protein–protein isopeptide, out of scope"
                .to_string(),
        }),
    }
}

fn phospho_residue(anchor: ProteinAnchor) -> Result<PtmResidue, PtmSupportError> {
    match anchor {
        ProteinAnchor::SerOg => Ok(PtmResidue {
            charmm_name: "SEP",
            anchor_atom: "OG",
            junction_partners: &["P"],
            net_charge: -1,
        }),
        ProteinAnchor::ThrOg1 => Ok(PtmResidue {
            charmm_name: "TPO",
            anchor_atom: "OG1",
            junction_partners: &["P"],
            net_charge: -1,
        }),
        ProteinAnchor::TyrOh => Ok(PtmResidue {
            charmm_name: "PTR",
            anchor_atom: "OH",
            junction_partners: &["P"],
            net_charge: -1,
        }),
        ProteinAnchor::HisNd1 | ProteinAnchor::HisNe2 => {
            Err(PtmSupportError::RequiresForceFieldAssets {
                detail: "phospho-His (phosphoramidate) has no bundled CHARMM36 residue".to_string(),
            })
        }
        other => Err(PtmSupportError::RequiresForceFieldAssets {
            detail: format!("phosphorylation has no native residue for anchor {other:?}"),
        }),
    }
}

fn acetyl_residue(n_terminal: bool, anchor: ProteinAnchor) -> Result<PtmResidue, PtmSupportError> {
    if n_terminal {
        return Err(PtmSupportError::RequiresForceFieldAssets {
            detail: "N-terminal acetyl (ACE cap) is an N-terminus patch, absent from the bundled \
                     aminoacids.n.tdb"
                .to_string(),
        });
    }
    match anchor {
        ProteinAnchor::LysNz => Ok(PtmResidue {
            charmm_name: "ALY",
            anchor_atom: "NZ",
            junction_partners: &["CH"],
            net_charge: 0,
        }),
        other => Err(PtmSupportError::RequiresForceFieldAssets {
            detail: format!("side-chain acetylation has no native residue for anchor {other:?}"),
        }),
    }
}

fn methyl_residue(
    degree: MethylDegree,
    anchor: ProteinAnchor,
) -> Result<PtmResidue, PtmSupportError> {
    match anchor {
        ProteinAnchor::LysNz => Ok(match degree {
            MethylDegree::Mono => PtmResidue {
                charmm_name: "MLZ",
                anchor_atom: "NZ",
                junction_partners: &["CM"],
                net_charge: 1,
            },
            MethylDegree::Di => PtmResidue {
                charmm_name: "MLY",
                anchor_atom: "NZ",
                junction_partners: &["CH1", "CH2"],
                net_charge: 1,
            },
            MethylDegree::Tri => PtmResidue {
                charmm_name: "M3L",
                anchor_atom: "NZ",
                junction_partners: &["CM1", "CM2", "CM3"],
                net_charge: 1,
            },
        }),
        ProteinAnchor::ArgNh1 | ProteinAnchor::ArgNh2 => {
            let native = match degree {
                MethylDegree::Mono => "AGM",
                MethylDegree::Di => "2MR",
                MethylDegree::Tri => {
                    return Err(PtmSupportError::RequiresForceFieldAssets {
                        detail: "tri-methyl-Arg is not a physiological modification".to_string(),
                    });
                }
            };
            Err(PtmSupportError::RenameNotWired {
                native,
                detail: "methyl-Arg guanidinium atom-name remap (CQ/CE2/NE1) is not yet wired"
                    .to_string(),
            })
        }
        other => Err(PtmSupportError::RequiresForceFieldAssets {
            detail: format!("methylation has no native residue for anchor {other:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phospho_anchors_map_to_native_residues() {
        assert_eq!(
            native_ptm_residue(PtmKind::Phosphoryl, ProteinAnchor::SerOg)
                .unwrap()
                .charmm_name,
            "SEP"
        );
        assert_eq!(
            native_ptm_residue(PtmKind::Phosphoryl, ProteinAnchor::ThrOg1)
                .unwrap()
                .charmm_name,
            "TPO"
        );
        assert_eq!(
            native_ptm_residue(PtmKind::Phosphoryl, ProteinAnchor::TyrOh)
                .unwrap()
                .charmm_name,
            "PTR"
        );
    }

    #[test]
    fn acetyl_lysine_and_methyl_lysine_map_to_native_residues() {
        assert_eq!(
            native_ptm_residue(PtmKind::Acetyl { n_terminal: false }, ProteinAnchor::LysNz)
                .unwrap()
                .charmm_name,
            "ALY"
        );
        for (degree, expected) in [
            (MethylDegree::Mono, "MLZ"),
            (MethylDegree::Di, "MLY"),
            (MethylDegree::Tri, "M3L"),
        ] {
            assert_eq!(
                native_ptm_residue(PtmKind::Methyl { degree }, ProteinAnchor::LysNz)
                    .unwrap()
                    .charmm_name,
                expected
            );
        }
    }

    #[test]
    fn deferred_families_gate_with_a_clear_error() {
        use crate::domain::modification::{AcylKind, PrenylKind, UblKind};
        for kind in [
            PtmKind::Acyl(AcylKind::Palmitoyl),
            PtmKind::Prenyl(PrenylKind::Farnesyl),
            PtmKind::Ubl(UblKind::Ubiquitin),
        ] {
            let err = native_ptm_residue(kind, ProteinAnchor::CysSg).unwrap_err();
            assert!(
                err.to_string().contains("requires force-field assets"),
                "deferred PTM must gate clearly, got {err}"
            );
        }
    }

    #[test]
    fn n_terminal_acetyl_and_phospho_his_are_gated() {
        assert!(
            native_ptm_residue(PtmKind::Acetyl { n_terminal: true }, ProteinAnchor::LysNz).is_err()
        );
        assert!(native_ptm_residue(PtmKind::Phosphoryl, ProteinAnchor::HisNd1).is_err());
    }

    #[test]
    fn methyl_arginine_names_its_native_residue_but_is_not_wired() {
        let err = native_ptm_residue(
            PtmKind::Methyl {
                degree: MethylDegree::Di,
            },
            ProteinAnchor::ArgNh1,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            PtmSupportError::RenameNotWired { native: "2MR", .. }
        ));
    }
}
