use nalgebra::{Matrix3, Point3, Rotation3, Vector3};

use crate::domain::glycan::TemplateAtom;

#[derive(Debug, Clone)]
pub struct PlacedAtom {
    pub name: String,
    pub element: String,
    pub position: Point3<f32>,
}

#[derive(Debug, Clone, Copy)]
pub struct PlacedBond {
    pub a: usize,
    pub b: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct AcceptorSite {
    pub oxygen_atom: usize,
    pub outward: Vector3<f32>,
}

#[derive(Debug, Clone, Copy)]
pub struct DonorSite {
    pub anomeric_atom: usize,
    pub outward: Vector3<f32>,
}

#[derive(Debug, Clone)]
pub struct FragmentPlacement {
    pub atoms: Vec<PlacedAtom>,
    pub bonds: Vec<PlacedBond>,
    pub donor_atom: usize,
}

pub fn compute_constrained_rotation(
    source_dir: Vector3<f32>,
    target_dir: Vector3<f32>,
    core_normal: Vector3<f32>,
    fragment_atoms: &[TemplateAtom],
    binding_atom: usize,
) -> Rotation3<f32> {
    let mut ref_vec = Vector3::zeros();
    for (i, atom) in fragment_atoms.iter().enumerate() {
        if i == binding_atom {
            continue;
        }
        let delta: Vector3<f32> = atom.position - fragment_atoms[binding_atom].position;
        let perp = delta - source_dir * source_dir.dot(&delta);
        if perp.norm() > ref_vec.norm() {
            ref_vec = perp;
        }
    }

    if ref_vec.norm() < 1e-6 {
        return Rotation3::rotation_between(&source_dir, &target_dir)
            .unwrap_or_else(Rotation3::identity);
    }
    ref_vec = ref_vec.normalize();

    let source_normal = source_dir.cross(&ref_vec).normalize();

    let target_ref = core_normal.cross(&target_dir);
    let target_ref_norm = target_ref.norm();
    let (target_ref, target_normal) = if target_ref_norm > 1e-6 {
        let tr = target_ref / target_ref_norm;
        let tn = target_dir.cross(&tr).normalize();
        (tr, tn)
    } else {
        return Rotation3::rotation_between(&source_dir, &target_dir)
            .unwrap_or_else(Rotation3::identity);
    };

    let source_basis = Matrix3::from_columns(&[source_dir, ref_vec, source_normal]);
    let target_basis = Matrix3::from_columns(&[target_dir, target_ref, target_normal]);
    Rotation3::from_matrix(&(target_basis * source_basis.transpose()))
}

pub fn place_fragment(
    child_atoms: &[TemplateAtom],
    child_bonds: &[(usize, usize)],
    donor: DonorSite,
    acceptor: AcceptorSite,
    acceptor_position: Point3<f32>,
    bond_length: f32,
    reference_normal: Vector3<f32>,
) -> FragmentPlacement {
    let source_dir = donor.outward;
    let target_dir = -acceptor.outward;
    let rotation = compute_constrained_rotation(
        source_dir,
        target_dir,
        reference_normal,
        child_atoms,
        donor.anomeric_atom,
    );

    let donor_local = child_atoms[donor.anomeric_atom].position.coords;
    let target_anomeric = acceptor_position + acceptor.outward * bond_length;
    let origin = target_anomeric - rotation * donor_local;

    let atoms = child_atoms
        .iter()
        .map(|atom| PlacedAtom {
            name: atom.name.clone(),
            element: atom.element.clone(),
            position: origin + rotation * atom.position.coords,
        })
        .collect();

    let bonds = child_bonds
        .iter()
        .map(|&(a, b)| PlacedBond { a, b })
        .collect();

    FragmentPlacement {
        atoms,
        bonds,
        donor_atom: donor.anomeric_atom,
    }
}
