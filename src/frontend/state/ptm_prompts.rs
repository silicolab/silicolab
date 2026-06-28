//! Draft state for the Modify Protein (PTM) task panel — the GUI counterpart of
//! the `phosphorylate`/`acetylate`/`methylate`/`lipidate`/`ubiquitinate` console
//! verbs. The panel edits a [`PendingPtm`]; the dispatcher turns it into a
//! [`crate::frontend::ptm_commands::PtmRequest`] and routes it through the shared
//! `apply_ptm` seam, so the console and panel never re-implement dispatch.

use crate::domain::modification::{MethylDegree, UblKind};
use crate::frontend::ptm_commands::LipidKind;
use crate::workflows::glycan::GlycosylationKind;

/// The post-translational-modification family selected in the panel. Maps to a
/// [`crate::frontend::ptm_commands::PtmRequest`] variant when the user applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PtmUiKind {
    #[default]
    Phosphorylate,
    Acetylate,
    Methylate,
    Lipidate,
    Ubiquitinate,
    Glycosylate,
}

impl PtmUiKind {
    /// Every family, in panel display order.
    pub const ALL: [PtmUiKind; 6] = [
        PtmUiKind::Phosphorylate,
        PtmUiKind::Acetylate,
        PtmUiKind::Methylate,
        PtmUiKind::Lipidate,
        PtmUiKind::Ubiquitinate,
        PtmUiKind::Glycosylate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PtmUiKind::Phosphorylate => "Phosphorylate",
            PtmUiKind::Acetylate => "Acetylate",
            PtmUiKind::Methylate => "Methylate",
            PtmUiKind::Lipidate => "Lipidate",
            PtmUiKind::Ubiquitinate => "Ubiquitinate",
            PtmUiKind::Glycosylate => "Glycosylation",
        }
    }

    /// Which residue the family expects at the anchor, shown under the anchor row
    /// so the user picks a compatible site (the apply seam enforces it).
    pub fn target_hint(self) -> &'static str {
        match self {
            PtmUiKind::Phosphorylate => "Targets a Ser, Thr, Tyr, or His residue.",
            PtmUiKind::Acetylate => "Targets a Lys side-chain (or the chain N-terminus).",
            PtmUiKind::Methylate => "Targets a Lys or Arg side-chain.",
            PtmUiKind::Lipidate => {
                "Targets a Cys side-chain (palmitoyl/prenyl) or an N-terminal Gly (myristoyl)."
            }
            PtmUiKind::Ubiquitinate => "Targets a Lys side-chain.",
            PtmUiKind::Glycosylate => "Targets Asn (N-linked) or Ser/Thr (O-linked).",
        }
    }
}

/// Draft for a Modify Protein (PTM) launch: the family, the anchor residue
/// (`chain` + `res_seq`), the family-specific selectors, and the result name.
/// Consumed by `start_pending_ptm`, which always modifies the active entry.
#[derive(Debug, Clone)]
pub struct PendingPtm {
    pub family: PtmUiKind,
    /// Anchor residue chain id (a single character).
    pub chain: String,
    /// Anchor residue sequence number.
    pub res_seq: i32,
    /// Degree for [`PtmUiKind::Methylate`].
    pub degree: MethylDegree,
    /// Lipid for [`PtmUiKind::Lipidate`].
    pub lipid: LipidKind,
    /// Ubiquitin-like modifier for [`PtmUiKind::Ubiquitinate`].
    pub ubl: UblKind,
    /// Open entry supplying a UBL template in place of the bundled one (`None`
    /// uses the built-in template).
    pub ubl_override: Option<u64>,
    /// Acetylate the chain N-terminus instead of the Lys side-chain NZ.
    pub n_terminal: bool,
    /// IUPAC-condensed glycan notation for [`PtmUiKind::Glycosylate`].
    pub glycan_iupac: String,
    /// N-linked / O-linked selector for [`PtmUiKind::Glycosylate`].
    pub glyco_kind: GlycosylationKind,
    /// Name for the resulting entry (blank keeps the auto-generated title).
    pub output_name: String,
}

impl Default for PendingPtm {
    fn default() -> Self {
        Self {
            family: PtmUiKind::default(),
            chain: "A".to_string(),
            res_seq: 1,
            degree: MethylDegree::Mono,
            lipid: LipidKind::Palmitoyl,
            ubl: UblKind::Ubiquitin,
            ubl_override: None,
            n_terminal: false,
            glycan_iupac: String::new(),
            glyco_kind: GlycosylationKind::NLinked,
            output_name: String::new(),
        }
    }
}
