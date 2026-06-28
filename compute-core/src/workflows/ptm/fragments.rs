//! Idealized modifying-group builders for protein post-translational
//! modifications. Each returns a small [`Fragment`] — a one-residue structure
//! carrying a biopolymer overlay — plus the donor/leaving metadata the shared
//! [`condense`](crate::workflows::assembly::condense) path needs to weld it onto
//! a host anchor. Geometry is idealized (template bond lengths/angles), never
//! energy-minimized; the condense step orients the fragment at the anchor.

use nalgebra::{Point3, Vector3};

use crate::domain::{
    Atom, Biopolymer, Bond, BondType, ChainRecord, ResidueId, ResidueRecord, Structure,
};

/// A modifying group ready to attach to a protein anchor: its own structure with
/// a biopolymer overlay, the donor atom that bonds to the host, the leaving atoms
/// deleted on attachment, and the outward bond direction (donor toward leaving).
pub struct Fragment {
    pub structure: Structure,
    pub donor: usize,
    pub leaving: Vec<usize>,
    pub outward: Vector3<f32>,
}

struct Builder {
    names: Vec<String>,
    atoms: Vec<Atom>,
    bonds: Vec<Bond>,
}

impl Builder {
    fn new() -> Self {
        Self {
            names: Vec::new(),
            atoms: Vec::new(),
            bonds: Vec::new(),
        }
    }

    fn atom(&mut self, name: &str, element: &str, position: Point3<f32>) -> usize {
        let index = self.atoms.len();
        self.names.push(name.to_string());
        self.atoms.push(Atom {
            element: element.to_string(),
            position,
            charge: 0.0,
        });
        index
    }

    fn bond(&mut self, a: usize, b: usize, bond_type: BondType) {
        self.bonds.push(Bond::with_type(a, b, bond_type));
    }

    fn position(&self, index: usize) -> Point3<f32> {
        self.atoms[index].position
    }

    fn finish(
        self,
        residue_name: &str,
        donor: usize,
        leaving: Vec<usize>,
        outward: Vector3<f32>,
    ) -> Fragment {
        let count = self.atoms.len();
        let residue = ResidueRecord {
            id: ResidueId::new('A', 1, ' '),
            residue_name: residue_name.to_string(),
            atom_indices: (0..count).collect(),
            alpha_carbon: None,
            backbone_nitrogen: None,
            backbone_carbon: None,
            backbone_oxygen: None,
            is_standard_amino_acid: false,
        };
        let biopolymer = Biopolymer {
            residues: vec![residue],
            chains: vec![ChainRecord {
                id: 'A',
                residue_indices: vec![0],
            }],
            secondary_structures: Vec::new(),
            residue_for_atom: vec![Some(0); count],
            atom_name_for_atom: self.names.into_iter().map(Some).collect(),
        };
        let mut structure = Structure::with_bonds(residue_name.to_string(), self.atoms, self.bonds);
        structure.biopolymer = Some(biopolymer);
        Fragment {
            structure,
            donor,
            leaving,
            outward,
        }
    }
}

/// Neutral phosphoric-acid group O=P(OH)3, the phosphomonoester precursor.
/// Donor P; leaving group one hydroxyl (O4P + its H), so the product is
/// anchorO–P(=O)(OH)2.
pub fn phosphate() -> Fragment {
    let mut b = Builder::new();
    let p = b.atom("P", "P", Point3::origin());
    let directions = tetrahedral();
    let po = 1.55;
    let o1 = b.atom("O1P", "O", Point3::from(directions[0].normalize() * po));
    let o2 = b.atom("O2P", "O", Point3::from(directions[1].normalize() * po));
    let o3 = b.atom("O3P", "O", Point3::from(directions[2].normalize() * po));
    let o4 = b.atom("O4P", "O", Point3::from(directions[3].normalize() * po));
    b.bond(p, o1, BondType::Double);
    b.bond(p, o2, BondType::Single);
    b.bond(p, o3, BondType::Single);
    b.bond(p, o4, BondType::Single);

    add_hydroxyl_hydrogen(&mut b, p, o2, "HO2P");
    add_hydroxyl_hydrogen(&mut b, p, o3, "HO3P");
    let h4 = add_hydroxyl_hydrogen(&mut b, p, o4, "HO4P");

    let outward = (b.position(o4) - b.position(p)).normalize();
    b.finish("PO4", p, vec![o4, h4], outward)
}

/// Acetic acid CH3-C(=O)-OH. Donor the carbonyl carbon; leaving the hydroxyl
/// (O + H), so amide/ester condensation onto an anchor forms anchorX–C(=O)CH3.
pub fn acetyl() -> Fragment {
    let mut b = Builder::new();
    let c = b.atom("C", "C", Point3::origin());
    let chain = Vector3::new(-1.0, 0.0, 0.0);
    let methyl = b.atom("CH3", "C", Point3::from(chain * 1.50));
    let o = b.atom("O", "O", Point3::from(rotate_z(chain, 120.0) * 1.22));
    let oh = b.atom("OH", "O", Point3::from(rotate_z(chain, -120.0) * 1.34));
    b.bond(c, methyl, BondType::Single);
    b.bond(c, o, BondType::Double);
    b.bond(c, oh, BondType::Single);
    let h = add_hydroxyl_hydrogen(&mut b, c, oh, "HO");

    let outward = (b.position(oh) - b.position(c)).normalize();
    saturate_carbons(&mut b);
    b.finish("ACE", c, vec![oh, h], outward)
}

/// Methane CH4. Donor the carbon; leaving one hydrogen, so the product is the
/// N-/O-methyl anchorX–CH3.
pub fn methyl() -> Fragment {
    let mut b = Builder::new();
    let c = b.atom("C", "C", Point3::origin());
    saturate_carbons(&mut b);
    // saturate_carbons appends the four methane hydrogens immediately after the
    // lone carbon, so the first sits at index 1.
    let leaving = 1;
    let outward = (b.position(leaving) - b.position(c)).normalize();
    b.finish("CH3", c, vec![leaving], outward)
}

/// Saturated fatty acid CH3(CH2)_{n-2}C(=O)OH as an idealized all-anti zig-zag.
/// Donor the carbonyl carbon; leaving the hydroxyl (O + H). Palmitoyl is 16
/// carbons, myristoyl 14.
pub fn acyl(n_carbons: usize) -> Fragment {
    let mut b = Builder::new();
    let n = n_carbons.max(2);
    let carbons = zigzag_carbons(&mut b, n, 1.54, 109.5);
    for k in 0..n - 1 {
        b.bond(carbons[k], carbons[k + 1], BondType::Single);
    }

    let c1 = carbons[0];
    let chain = (b.position(carbons[1]) - b.position(c1)).normalize();
    let o = b.atom("O", "O", b.position(c1) + rotate_z(chain, 120.0) * 1.22);
    let oh = b.atom("OXT", "O", b.position(c1) + rotate_z(chain, -120.0) * 1.34);
    b.bond(c1, o, BondType::Double);
    b.bond(c1, oh, BondType::Single);
    let h = add_hydroxyl_hydrogen(&mut b, c1, oh, "HO");

    let outward = (b.position(oh) - b.position(c1)).normalize();
    saturate_carbons(&mut b);
    b.finish("ACY", c1, vec![oh, h], outward)
}

/// Polyprenyl group from concatenated isoprene units -CH2-CH=C(CH3)-CH2- with
/// trans double bonds and methyl branches. Donor the C1 methylene; leaving the
/// C1 hydroxyl, so a thioether forms anchorS–CH2–…. Farnesyl is 3 units (C15),
/// geranylgeranyl 4 (C20).
pub fn isoprenoid(units: usize) -> Fragment {
    let mut b = Builder::new();
    let u = units.max(1);
    let backbone = zigzag_carbons(&mut b, 4 * u, 1.47, 120.0);
    for j in 0..backbone.len() - 1 {
        // Within each unit the B=C double bond sits at the second backbone bond.
        let bond_type = if j % 4 == 1 {
            BondType::Double
        } else {
            BondType::Single
        };
        b.bond(backbone[j], backbone[j + 1], bond_type);
    }

    let mut methyl_label = 0;
    for k in (2..backbone.len()).step_by(4) {
        let n0 = (b.position(backbone[k - 1]) - b.position(backbone[k])).normalize();
        let n1 = (b.position(backbone[k + 1]) - b.position(backbone[k])).normalize();
        let direction = -(n0 + n1).normalize();
        methyl_label += 1;
        let methyl = b.atom(
            &format!("CM{methyl_label}"),
            "C",
            b.position(backbone[k]) + direction * 1.51,
        );
        b.bond(backbone[k], methyl, BondType::Single);
    }

    let c1 = backbone[0];
    let chain = (b.position(backbone[1]) - b.position(c1)).normalize();
    let oh = b.atom("O1", "O", b.position(c1) + rotate_z(chain, 125.0) * 1.43);
    b.bond(c1, oh, BondType::Single);
    let h = add_hydroxyl_hydrogen(&mut b, c1, oh, "HO1");

    let outward = (b.position(oh) - b.position(c1)).normalize();
    saturate_carbons(&mut b);
    b.finish("PRE", c1, vec![oh, h], outward)
}

fn tetrahedral() -> [Vector3<f32>; 4] {
    [
        Vector3::new(1.0, 1.0, 1.0),
        Vector3::new(1.0, -1.0, -1.0),
        Vector3::new(-1.0, 1.0, -1.0),
        Vector3::new(-1.0, -1.0, 1.0),
    ]
}

fn rotate_z(v: Vector3<f32>, degrees: f32) -> Vector3<f32> {
    let (s, c) = degrees.to_radians().sin_cos();
    Vector3::new(v.x * c - v.y * s, v.x * s + v.y * c, v.z)
}

fn zigzag_carbons(b: &mut Builder, n: usize, length: f32, angle: f32) -> Vec<usize> {
    let half = (angle / 2.0).to_radians();
    let dx = length * half.sin();
    let dy = length * half.cos();
    (0..n)
        .map(|k| {
            let y = if k % 2 == 0 { 0.0 } else { dy };
            b.atom(
                &format!("C{}", k + 1),
                "C",
                Point3::new(k as f32 * dx, y, 0.0),
            )
        })
        .collect()
}

fn add_hydroxyl_hydrogen(b: &mut Builder, heavy: usize, oxygen: usize, name: &str) -> usize {
    let direction = (b.position(oxygen) - b.position(heavy)).normalize();
    let h = b.atom(name, "H", b.position(oxygen) + direction * 0.97);
    b.bond(oxygen, h, BondType::Single);
    h
}

/// Fill every carbon to its sigma valence (sp3 → 4, sp2 → 3) with idealized
/// hydrogens, derived from the carbon's existing heavy-neighbor geometry.
fn saturate_carbons(b: &mut Builder) {
    let count = b.atoms.len();
    let mut degree = vec![0usize; count];
    let mut sp2 = vec![false; count];
    let mut directions: Vec<Vec<Vector3<f32>>> = vec![Vec::new(); count];
    for bond in &b.bonds {
        degree[bond.a] += 1;
        degree[bond.b] += 1;
        if bond.bond_type == BondType::Double {
            sp2[bond.a] = true;
            sp2[bond.b] = true;
        }
        let delta = b.atoms[bond.b].position - b.atoms[bond.a].position;
        directions[bond.a].push(delta);
        directions[bond.b].push(-delta);
    }

    let targets: Vec<(usize, Vec<Vector3<f32>>, usize)> = (0..count)
        .filter_map(|i| {
            if b.atoms[i].element != "C" {
                return None;
            }
            let valence: usize = if sp2[i] { 3 } else { 4 };
            let needed = valence.saturating_sub(degree[i]);
            (needed > 0).then(|| (i, directions[i].clone(), needed))
        })
        .collect();

    let mut label = 0;
    for (carbon, neighbors, needed) in targets {
        for position in hydrogens(b.position(carbon), &neighbors, needed, 1.09) {
            label += 1;
            let h = b.atom(&format!("H{label}"), "H", position);
            b.bond(carbon, h, BondType::Single);
        }
    }
}

/// Idealized hydrogen positions completing a carbon's tetrahedral/trigonal
/// valence from the unit directions toward its heavy neighbors.
fn hydrogens(
    center: Point3<f32>,
    neighbors: &[Vector3<f32>],
    count: usize,
    bond: f32,
) -> Vec<Point3<f32>> {
    let place = |direction: Vector3<f32>| center + direction.normalize() * bond;
    match (neighbors.len(), count) {
        (0, k) => tetrahedral().iter().take(k).map(|d| place(*d)).collect(),
        (1, 3) => {
            let axis = neighbors[0].normalize();
            let (e1, e2) = orthonormal(axis);
            let lateral = (8.0_f32 / 9.0).sqrt();
            (0..3)
                .map(|k| {
                    let phi = k as f32 * std::f32::consts::TAU / 3.0;
                    place(-axis / 3.0 + (e1 * phi.cos() + e2 * phi.sin()) * lateral)
                })
                .collect()
        }
        (2, 2) => {
            let bisector = (neighbors[0].normalize() + neighbors[1].normalize()).normalize();
            let perp = neighbors[0].cross(&neighbors[1]).normalize();
            let tilt = 54.75_f32.to_radians();
            vec![
                place(-bisector * tilt.cos() + perp * tilt.sin()),
                place(-bisector * tilt.cos() - perp * tilt.sin()),
            ]
        }
        (2, 1) => vec![place(
            -(neighbors[0].normalize() + neighbors[1].normalize()),
        )],
        (1, 1) => vec![place(-neighbors[0].normalize())],
        _ => Vec::new(),
    }
}

fn orthonormal(axis: Vector3<f32>) -> (Vector3<f32>, Vector3<f32>) {
    let reference = if axis.x.abs() < 0.9 {
        Vector3::x()
    } else {
        Vector3::y()
    };
    let e1 = axis.cross(&reference).normalize();
    let e2 = axis.cross(&e1).normalize();
    (e1, e2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carbon_count(fragment: &Fragment) -> usize {
        fragment
            .structure
            .atoms
            .iter()
            .filter(|atom| atom.element == "C")
            .count()
    }

    fn check_fragment(fragment: &Fragment) {
        let structure = &fragment.structure;
        let bio = structure.biopolymer.as_ref().expect("biopolymer overlay");
        assert!(
            bio.is_compatible_with_atom_count(structure.atoms.len()),
            "overlay must cover every atom"
        );
        assert_eq!(bio.residues.len(), 1, "fragment is a single residue");
        assert!(
            bio.atom_name_for_atom.iter().all(Option::is_some),
            "every atom carries a name"
        );
        assert!(fragment.donor < structure.atoms.len(), "donor in range");
        assert!(!fragment.leaving.is_empty(), "leaving set non-empty");
        for &leaving in &fragment.leaving {
            assert!(leaving < structure.atoms.len(), "leaving atom in range");
        }
        assert!(fragment.outward.norm() > 0.5, "outward is a real direction");
    }

    #[test]
    fn phosphate_builds() {
        let fragment = phosphate();
        check_fragment(&fragment);
        assert_eq!(fragment.structure.atoms.len(), 8);
        assert_eq!(fragment.structure.atoms[fragment.donor].element, "P");
        assert_eq!(fragment.leaving.len(), 2);
    }

    #[test]
    fn acetyl_builds() {
        let fragment = acetyl();
        check_fragment(&fragment);
        assert_eq!(fragment.structure.atoms.len(), 8);
        assert_eq!(fragment.structure.atoms[fragment.donor].element, "C");
        assert_eq!(fragment.leaving.len(), 2);
    }

    #[test]
    fn methyl_builds() {
        let fragment = methyl();
        check_fragment(&fragment);
        assert_eq!(fragment.structure.atoms.len(), 5);
        assert_eq!(fragment.leaving.len(), 1);
        assert_eq!(
            fragment.structure.atoms[fragment.leaving[0]].element, "H",
            "a single hydrogen leaves"
        );
    }

    #[test]
    fn acyl_chains_have_expected_carbon_and_atom_counts() {
        for n in [14usize, 16] {
            let fragment = acyl(n);
            check_fragment(&fragment);
            assert_eq!(carbon_count(&fragment), n);
            assert_eq!(fragment.structure.atoms.len(), 3 * n + 2);
        }
    }

    #[test]
    fn isoprenoid_units_yield_five_carbons_each() {
        for u in [3usize, 4] {
            let fragment = isoprenoid(u);
            check_fragment(&fragment);
            assert_eq!(carbon_count(&fragment), 5 * u);
            let doubles = fragment
                .structure
                .bonds
                .iter()
                .filter(|bond| bond.bond_type == BondType::Double)
                .count();
            assert_eq!(doubles, u, "one trans double bond per isoprene unit");
        }
    }
}
