use std::str::FromStr;

use nalgebra::{Matrix3, Point3, Vector3};

use crate::domain::{biopolymer::Biopolymer, chemistry};

#[derive(Debug, Clone)]
pub struct Atom {
    pub element: String,
    pub position: Point3<f32>,
    pub charge: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BondType {
    #[default]
    Single,
    Double,
    Triple,
    Aromatic,
}

impl BondType {
    pub fn from_mol2_token(token: &str) -> Self {
        match token {
            "1" => Self::Single,
            "2" => Self::Double,
            "3" => Self::Triple,
            "ar" => Self::Aromatic,
            "am" => Self::Single,
            _ => token.parse::<Self>().unwrap_or_default(),
        }
    }

    pub fn to_mol2_token(self) -> &'static str {
        match self {
            Self::Single => "1",
            Self::Double => "2",
            Self::Triple => "3",
            Self::Aromatic => "ar",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "Single",
            Self::Double => "Double",
            Self::Triple => "Triple",
            Self::Aromatic => "Aromatic",
        }
    }

    pub fn all() -> &'static [BondType] {
        &[Self::Single, Self::Double, Self::Triple, Self::Aromatic]
    }
}

impl FromStr for BondType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "1" => Ok(Self::Single),
            "2" => Ok(Self::Double),
            "3" => Ok(Self::Triple),
            "4" => Ok(Self::Aromatic),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Bond {
    pub a: usize,
    pub b: usize,
    pub bond_type: BondType,
}

impl Bond {
    pub fn with_type(a: usize, b: usize, bond_type: BondType) -> Self {
        Self { a, b, bond_type }
    }
}

#[derive(Debug, Clone)]
pub struct Structure {
    pub title: String,
    pub atoms: Vec<Atom>,
    pub bonds: Vec<Bond>,
    pub cell: Option<UnitCell>,
    pub biopolymer: Option<Biopolymer>,
}

#[derive(Debug, Clone)]
pub struct UnitCell {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub alpha: f32,
    pub beta: f32,
    pub gamma: f32,
    pub vectors: [Vector3<f32>; 3],
}

impl Structure {
    pub fn empty() -> Self {
        Self {
            title: "Untitled".to_string(),
            atoms: Vec::new(),
            bonds: Vec::new(),
            cell: None,
            biopolymer: None,
        }
    }

    pub fn new(title: impl Into<String>, atoms: Vec<Atom>) -> Self {
        let bonds = chemistry::infer_bonds_with_cell(&atoms, None);

        Self {
            title: title.into(),
            atoms,
            bonds,
            cell: None,
            biopolymer: None,
        }
    }

    pub fn with_cell(title: impl Into<String>, atoms: Vec<Atom>, cell: UnitCell) -> Self {
        let bonds = chemistry::infer_bonds_with_cell(&atoms, Some(&cell));

        Self {
            title: title.into(),
            atoms,
            bonds,
            cell: Some(cell),
            biopolymer: None,
        }
    }

    pub fn with_bonds(title: impl Into<String>, atoms: Vec<Atom>, bonds: Vec<Bond>) -> Self {
        Self {
            title: title.into(),
            atoms,
            bonds,
            cell: None,
            biopolymer: None,
        }
    }

    pub fn with_cell_and_bonds(
        title: impl Into<String>,
        atoms: Vec<Atom>,
        bonds: Vec<Bond>,
        cell: UnitCell,
    ) -> Self {
        Self {
            title: title.into(),
            atoms,
            bonds,
            cell: Some(cell),
            biopolymer: None,
        }
    }

    pub fn recompute_bonds(&mut self) {
        self.bonds = chemistry::infer_bonds_with_cell(&self.atoms, self.cell.as_ref());
    }

    pub fn add_bond(&mut self, a: usize, b: usize, bond_type: BondType) {
        if a >= self.atoms.len() || b >= self.atoms.len() || a == b {
            return;
        }
        if self
            .bonds
            .iter()
            .any(|bond| (bond.a == a && bond.b == b) || (bond.a == b && bond.b == a))
        {
            return;
        }
        self.bonds.push(Bond::with_type(a, b, bond_type));
    }

    pub fn remove_bond(&mut self, index: usize) {
        if index < self.bonds.len() {
            self.bonds.remove(index);
        }
    }

    pub fn set_bond_type(&mut self, index: usize, bond_type: BondType) {
        if let Some(bond) = self.bonds.get_mut(index) {
            bond.bond_type = bond_type;
        }
    }

    pub fn remove_bonds_for_atom(&mut self, atom_index: usize) {
        self.bonds
            .retain(|bond| bond.a != atom_index && bond.b != atom_index);
    }

    pub fn adjust_bond_indices_after_removal(&mut self, removed_index: usize) {
        for bond in &mut self.bonds {
            if bond.a > removed_index {
                bond.a -= 1;
            }
            if bond.b > removed_index {
                bond.b -= 1;
            }
        }
    }

    pub fn add_missing_hydrogens(&mut self) -> usize {
        chemistry::add_missing_hydrogens(&mut self.atoms, &mut self.bonds)
    }

    pub fn wrap_atoms_into_cell(&mut self) {
        let Some(cell) = &self.cell else {
            return;
        };

        for atom in &mut self.atoms {
            let mut frac = cell.cartesian_to_fractional(atom.position);

            frac.x = frac.x.rem_euclid(1.0);
            frac.y = frac.y.rem_euclid(1.0);
            frac.z = frac.z.rem_euclid(1.0);
            atom.position = cell.fractional_to_cartesian(frac.x, frac.y, frac.z);
        }

        self.recompute_bonds();
    }

    pub fn wrap_atoms_into_cell_preserving_bonds(&mut self) {
        let Some(cell) = &self.cell else {
            return;
        };

        for atom in &mut self.atoms {
            let mut frac = cell.cartesian_to_fractional(atom.position);

            frac.x = frac.x.rem_euclid(1.0);
            frac.y = frac.y.rem_euclid(1.0);
            frac.z = frac.z.rem_euclid(1.0);
            atom.position = cell.fractional_to_cartesian(frac.x, frac.y, frac.z);
        }
    }

    pub fn make_supercell(&mut self, repeats: [u32; 3]) {
        let Some(cell) = &self.cell else {
            return;
        };

        let nx = repeats[0].max(1);
        let ny = repeats[1].max(1);
        let nz = repeats[2].max(1);

        if nx == 1 && ny == 1 && nz == 1 {
            return;
        }

        let source_atom_count = self.atoms.len();
        let expanded_cell = UnitCell::from_parameters(
            cell.a * nx as f32,
            cell.b * ny as f32,
            cell.c * nz as f32,
            cell.alpha,
            cell.beta,
            cell.gamma,
        );

        let mut atoms = Vec::with_capacity(source_atom_count * (nx * ny * nz) as usize);
        let mut bonds = Vec::new();

        for ix in 0..nx {
            for iy in 0..ny {
                for iz in 0..nz {
                    for atom in &self.atoms {
                        let frac = cell.cartesian_to_fractional(atom.position);
                        let expanded_frac = Vector3::new(
                            (frac.x + ix as f32) / nx as f32,
                            (frac.y + iy as f32) / ny as f32,
                            (frac.z + iz as f32) / nz as f32,
                        );

                        atoms.push(Atom {
                            element: atom.element.clone(),
                            position: expanded_cell.fractional_to_cartesian(
                                expanded_frac.x,
                                expanded_frac.y,
                                expanded_frac.z,
                            ),
                            charge: atom.charge,
                        });
                    }
                }
            }
        }

        for ix in 0..nx {
            for iy in 0..ny {
                for iz in 0..nz {
                    for bond in &self.bonds {
                        let shift = bond_cell_shift(cell, &self.atoms, bond);
                        let jx = (ix as i32 + shift.0).rem_euclid(nx as i32) as u32;
                        let jy = (iy as i32 + shift.1).rem_euclid(ny as i32) as u32;
                        let jz = (iz as i32 + shift.2).rem_euclid(nz as i32) as u32;
                        let a = supercell_index(ix, iy, iz, bond.a, ny, nz, source_atom_count);
                        let b = supercell_index(jx, jy, jz, bond.b, ny, nz, source_atom_count);

                        if a != b {
                            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                            if !bonds
                                .iter()
                                .any(|existing: &Bond| existing.a == lo && existing.b == hi)
                            {
                                bonds.push(Bond::with_type(lo, hi, bond.bond_type));
                            }
                        }
                    }
                }
            }
        }

        self.atoms = atoms;
        self.bonds = bonds;
        self.cell = Some(expanded_cell);
        self.biopolymer = None;
    }

    /// Classify an atom into a broad chemical category for quick selection and
    /// default styling. Uses residue metadata when the biopolymer covers all
    /// atoms (e.g. after solvation); otherwise falls back to an element-based
    /// guess for lone ions.
    pub fn atom_category(&self, atom_index: usize) -> crate::domain::AtomCategory {
        use crate::domain::{AtomCategory, biopolymer};

        let element = self
            .atoms
            .get(atom_index)
            .map(|atom| atom.element.as_str())
            .unwrap_or("");

        if let Some(biopolymer) = self
            .biopolymer
            .as_ref()
            .filter(|biopolymer| biopolymer.is_compatible_with_atom_count(self.atoms.len()))
            && let Some(Some(residue_index)) = biopolymer.residue_for_atom.get(atom_index)
            && let Some(residue) = biopolymer.residues.get(*residue_index)
        {
            let name = residue.residue_name.as_str();
            if residue.is_standard_amino_acid {
                return AtomCategory::Protein;
            }
            if biopolymer::is_nucleic_acid_residue(name) {
                return AtomCategory::NucleicAcid;
            }
            if biopolymer::is_water_residue(name) {
                return AtomCategory::Solvent;
            }
            if chemistry::is_monatomic_ion_element(element) {
                return AtomCategory::Ion;
            }
            return AtomCategory::Ligand;
        }

        if chemistry::is_monatomic_ion_element(element) {
            AtomCategory::Ion
        } else {
            AtomCategory::Other
        }
    }

    pub fn center(&self) -> Point3<f32> {
        if let Some(cell) = &self.cell {
            let corners = cell.corners();
            let sum = corners
                .iter()
                .fold(Vector3::zeros(), |acc, corner| acc + corner.coords);

            return Point3::from(sum / corners.len() as f32);
        }

        if self.atoms.is_empty() {
            return Point3::origin();
        }

        let sum = self
            .atoms
            .iter()
            .fold(nalgebra::Vector3::zeros(), |acc, atom| {
                acc + atom.position.coords
            });

        Point3::from(sum / self.atoms.len() as f32)
    }

    pub fn radius(&self) -> f32 {
        let center = self.center();
        let atom_radius = self
            .atoms
            .iter()
            .map(|atom| nalgebra::distance(&center, &atom.position))
            .fold(1.0_f32, f32::max);

        if let Some(cell) = &self.cell {
            return cell
                .corners()
                .iter()
                .map(|corner| nalgebra::distance(&center, corner))
                .fold(atom_radius, f32::max);
        }

        atom_radius
    }
}

impl UnitCell {
    pub fn from_vectors(vectors: [Vector3<f32>; 3]) -> Self {
        let [avec, bvec, cvec] = vectors;
        let a = avec.norm();
        let b = bvec.norm();
        let c = cvec.norm();
        let alpha = angle_degrees(bvec, cvec);
        let beta = angle_degrees(avec, cvec);
        let gamma = angle_degrees(avec, bvec);

        Self {
            a,
            b,
            c,
            alpha,
            beta,
            gamma,
            vectors,
        }
    }

    /// Whether this is the `1 × 1 × 1` / 90°-90°-90° placeholder cell that some
    /// modeling tools write into a `CRYST1` record for a non-periodic molecule.
    /// It is not a real lattice: using it for periodic distance or bond
    /// inference collapses every atom onto a neighbor's image and connects
    /// everything, so callers must treat it as "no cell".
    pub fn is_placeholder(&self) -> bool {
        const TOLERANCE: f32 = 0.001;
        (self.a - 1.0).abs() < TOLERANCE
            && (self.b - 1.0).abs() < TOLERANCE
            && (self.c - 1.0).abs() < TOLERANCE
            && (self.alpha - 90.0).abs() < TOLERANCE
            && (self.beta - 90.0).abs() < TOLERANCE
            && (self.gamma - 90.0).abs() < TOLERANCE
    }

    pub fn from_parameters(a: f32, b: f32, c: f32, alpha: f32, beta: f32, gamma: f32) -> Self {
        let alpha_rad = alpha.to_radians();
        let beta_rad = beta.to_radians();
        let gamma_rad = gamma.to_radians();

        let avec = Vector3::new(a, 0.0, 0.0);
        let bvec = Vector3::new(b * gamma_rad.cos(), b * gamma_rad.sin(), 0.0);
        let cx = c * beta_rad.cos();
        let cy = c * (alpha_rad.cos() - beta_rad.cos() * gamma_rad.cos()) / gamma_rad.sin();
        let cz = (c.powi(2) - cx.powi(2) - cy.powi(2)).max(0.0).sqrt();
        let cvec = Vector3::new(cx, cy, cz);

        Self {
            a,
            b,
            c,
            alpha,
            beta,
            gamma,
            vectors: [avec, bvec, cvec],
        }
    }

    pub fn fractional_to_cartesian(&self, x: f32, y: f32, z: f32) -> Point3<f32> {
        Point3::from(self.vectors[0] * x + self.vectors[1] * y + self.vectors[2] * z)
    }

    pub fn cartesian_to_fractional(&self, point: Point3<f32>) -> Vector3<f32> {
        let basis = Matrix3::from_columns(&self.vectors);

        basis
            .try_inverse()
            .map(|inverse| inverse * point.coords)
            .unwrap_or_else(Vector3::zeros)
    }

    pub fn corners(&self) -> [Point3<f32>; 8] {
        let [a, b, c] = self.vectors;

        [
            Point3::origin(),
            Point3::from(a),
            Point3::from(b),
            Point3::from(c),
            Point3::from(a + b),
            Point3::from(a + c),
            Point3::from(b + c),
            Point3::from(a + b + c),
        ]
    }
}

fn angle_degrees(first: Vector3<f32>, second: Vector3<f32>) -> f32 {
    let denom = first.norm() * second.norm();
    if denom <= 0.0001 {
        return 90.0;
    }

    (first.dot(&second) / denom)
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

fn bond_cell_shift(cell: &UnitCell, atoms: &[Atom], bond: &Bond) -> (i32, i32, i32) {
    let first = cell.cartesian_to_fractional(atoms[bond.a].position);
    let second = cell.cartesian_to_fractional(atoms[bond.b].position);
    let delta = second - first;

    (
        -delta.x.round() as i32,
        -delta.y.round() as i32,
        -delta.z.round() as i32,
    )
}

fn supercell_index(
    ix: u32,
    iy: u32,
    iz: u32,
    atom: usize,
    ny: u32,
    nz: u32,
    source_atom_count: usize,
) -> usize {
    (((ix * ny + iy) * nz + iz) as usize * source_atom_count) + atom
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use super::{Atom, Bond, BondType, Structure, UnitCell};
    use crate::domain::{AtomCategory, PdbAtomAnnotation, build_biopolymer};

    fn atom(element: &str) -> Atom {
        Atom {
            element: element.to_string(),
            position: Point3::origin(),
            charge: 0.0,
        }
    }

    fn annotation(atom_name: &str, residue_name: &str, seq: i32) -> PdbAtomAnnotation {
        PdbAtomAnnotation {
            atom_name: atom_name.to_string(),
            residue_name: residue_name.to_string(),
            chain_id: 'A',
            residue_seq: seq,
            insertion_code: ' ',
        }
    }

    #[test]
    fn atom_category_uses_residue_metadata() {
        // One alanine atom (protein), one ligand hetero atom, one water oxygen.
        let annotations = vec![
            annotation("CA", "ALA", 1),
            annotation("C1", "LIG", 2),
            annotation("OW", "SOL", 3),
        ];
        let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("biopolymer");
        let structure = Structure {
            title: "t".to_string(),
            atoms: vec![atom("C"), atom("C"), atom("O")],
            bonds: Vec::new(),
            cell: None,
            biopolymer: Some(biopolymer),
        };
        assert_eq!(structure.atom_category(0), AtomCategory::Protein);
        assert_eq!(structure.atom_category(1), AtomCategory::Ligand);
        assert_eq!(structure.atom_category(2), AtomCategory::Solvent);
    }

    #[test]
    fn atom_category_falls_back_to_element_for_lone_ions() {
        let structure = Structure {
            title: "ions".to_string(),
            atoms: vec![atom("Na"), atom("C")],
            bonds: Vec::new(),
            cell: None,
            biopolymer: None,
        };
        assert_eq!(structure.atom_category(0), AtomCategory::Ion);
        assert_eq!(structure.atom_category(1), AtomCategory::Other);
    }

    #[test]
    fn wraps_periodic_atoms_into_unit_cell() {
        let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
        let mut structure = Structure::with_cell(
            "wrapped",
            vec![Atom {
                element: "C".to_string(),
                position: Point3::new(12.0, -1.0, 5.0),
                charge: 0.0,
            }],
            cell,
        );

        structure.wrap_atoms_into_cell();
        let frac = structure
            .cell
            .as_ref()
            .expect("cell")
            .cartesian_to_fractional(structure.atoms[0].position);

        assert!((frac.x - 0.2).abs() < 0.0001);
        assert!((frac.y - 0.9).abs() < 0.0001);
        assert!((frac.z - 0.5).abs() < 0.0001);
    }

    #[test]
    fn wraps_periodic_atoms_without_recomputing_bond_types() {
        let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
        let mut structure = Structure::with_cell_and_bonds(
            "wrapped",
            vec![
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(12.0, -1.0, 5.0),
                    charge: 0.0,
                },
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(-3.0, 11.0, -6.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Aromatic)],
            cell,
        );

        structure.wrap_atoms_into_cell_preserving_bonds();

        let cell = structure.cell.as_ref().expect("cell");
        for atom in &structure.atoms {
            let frac = cell.cartesian_to_fractional(atom.position);
            assert!((0.0..1.0).contains(&frac.x));
            assert!((0.0..1.0).contains(&frac.y));
            assert!((0.0..1.0).contains(&frac.z));
        }
        assert_eq!(structure.bonds.len(), 1);
        assert_eq!(structure.bonds[0].bond_type, BondType::Aromatic);
    }

    #[test]
    fn make_supercell_expands_atoms_and_cell() {
        let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
        let mut structure = Structure::with_cell(
            "test",
            vec![Atom {
                element: "C".to_string(),
                position: Point3::new(2.5, 2.5, 2.5),
                charge: 0.0,
            }],
            cell,
        );

        structure.make_supercell([2, 2, 2]);

        assert_eq!(structure.atoms.len(), 8);
        let expanded_cell = structure.cell.as_ref().expect("cell");
        assert!((expanded_cell.a - 20.0).abs() < 0.001);
        assert!((expanded_cell.b - 20.0).abs() < 0.001);
        assert!((expanded_cell.c - 20.0).abs() < 0.001);
    }

    #[test]
    fn make_supercell_preserves_bond_types() {
        let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
        let mut structure = Structure::with_cell_and_bonds(
            "test",
            vec![
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(1.34, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Double)],
            cell,
        );

        structure.make_supercell([2, 1, 1]);

        assert_eq!(structure.atoms.len(), 4);
        assert!(
            structure
                .bonds
                .iter()
                .any(|b| b.bond_type == BondType::Double)
        );
    }

    #[test]
    fn make_supercell_no_op_for_identity() {
        let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
        let mut structure = Structure::with_cell(
            "test",
            vec![Atom {
                element: "C".to_string(),
                position: Point3::new(2.5, 2.5, 2.5),
                charge: 0.0,
            }],
            cell,
        );

        structure.make_supercell([1, 1, 1]);

        assert_eq!(structure.atoms.len(), 1);
        let expanded_cell = structure.cell.as_ref().expect("cell");
        assert!((expanded_cell.a - 10.0).abs() < 0.001);
    }

    #[test]
    fn make_supercell_no_op_without_cell() {
        let mut structure = Structure::new(
            "test",
            vec![Atom {
                element: "C".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
        );

        structure.make_supercell([2, 2, 2]);

        assert_eq!(structure.atoms.len(), 1);
        assert!(structure.cell.is_none());
    }

    #[test]
    fn make_supercell_handles_cross_boundary_bonds() {
        let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
        let mut structure = Structure::with_cell_and_bonds(
            "test",
            vec![
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.1, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(9.9, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Single)],
            cell,
        );

        structure.make_supercell([2, 1, 1]);

        assert_eq!(structure.atoms.len(), 4);
        assert_eq!(structure.bonds.len(), 2);
        for bond in &structure.bonds {
            assert_eq!(bond.bond_type, BondType::Single);
        }
    }
}
