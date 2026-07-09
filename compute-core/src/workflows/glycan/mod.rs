pub mod builder;
pub mod glycoprotein;
pub mod torsions;

pub use builder::{glycan_to_structure, tree_to_structure};
pub use glycoprotein::{
    Glycosylation, GlycosylationKind, glycosylate_protein, glycosylation_kind_for_residue,
};

use crate::domain::glycan;

/// The canonical notation a build resolves to, with every anomeric configuration
/// stated — the internal ones in their linkages, the reducing end's in an open
/// linkage. Echoing this back shows the stereochemistry that was actually built
/// rather than the shorthand that was typed.
pub fn canonical_notation(
    notation: &str,
    kind: Option<GlycosylationKind>,
    root_anomer: Option<crate::domain::glycan::Anomer>,
) -> anyhow::Result<String> {
    let mut tree = glycan::parse(notation)?;
    glycan::resolve_root_anomer(&mut tree, kind, root_anomer)?;
    Ok(glycan::to_iupac(&tree))
}
