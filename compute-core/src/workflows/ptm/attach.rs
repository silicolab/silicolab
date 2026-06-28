//! The single welding step shared by every PTM family: turn a [`Fragment`] into a
//! [`DonorSpec`] and hand it, with the host [`AcceptorSpec`], to the one shared
//! [`condense`](crate::workflows::assembly::condense) attachment path.

use anyhow::Result;

use crate::domain::{BondType, Structure};
use crate::workflows::assembly::condense::{self, AcceptorSpec, DonorSpec};

use super::fragments::Fragment;

/// Weld `fragment` onto `protein` at `acceptor`, bonding the fragment donor to
/// the host anchor and dropping both leaving groups.
pub(crate) fn weld(
    protein: &Structure,
    acceptor: AcceptorSpec,
    fragment: &Fragment,
    bond_length: f32,
    bond_type: BondType,
    title_suffix: &str,
) -> Result<Structure> {
    let donor = DonorSpec {
        donor_atom: fragment.donor,
        remove: fragment.leaving.clone(),
        outward: fragment.outward,
    };
    condense::attach_fragment(
        protein,
        acceptor,
        &fragment.structure,
        donor,
        bond_length,
        bond_type,
        title_suffix,
    )
}
