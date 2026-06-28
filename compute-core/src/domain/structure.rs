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
        // A deposited biomolecule is bonded non-periodically (see the PDB/mmCIF
        // readers); only genuine periodic materials bond through the cell. Honor
        // the same invariant here so the GUI "recompute bonds" action can't
        // re-introduce the periodic path's freeze and spurious cross-cell bonds.
        let bonding_cell = self.cell.as_ref().filter(|_| self.biopolymer.is_none());
        self.bonds = chemistry::infer_bonds_with_cell(&self.atoms, bonding_cell);
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
            if biopolymer::is_carbohydrate_residue(name) {
                return AtomCategory::Carbohydrate;
            }
            return AtomCategory::Ligand;
        }

        if chemistry::is_monatomic_ion_element(element) {
            AtomCategory::Ion
        } else {
            AtomCategory::Other
        }
    }

    /// Whether the atom's residue carries a complete peptide backbone (N/CA/C) —
    /// the topological test for ribbon-drawability. Decided from atoms alone, so
    /// it holds for force-field-protonated, disulfide, and otherwise renamed
    /// protein residues exactly as for their canonical forms. Lets the cartoon
    /// overlay default on for protein backbone without consulting residue names
    /// (kept separate from [`Self::atom_category`], which stays name-based for
    /// classification, selection, and MD).
    pub fn atom_has_peptide_backbone(&self, atom_index: usize) -> bool {
        self.biopolymer
            .as_ref()
            .filter(|biopolymer| biopolymer.is_compatible_with_atom_count(self.atoms.len()))
            .and_then(|biopolymer| {
                let residue_index = (*biopolymer.residue_for_atom.get(atom_index)?)?;
                biopolymer.residues.get(residue_index)
            })
            .is_some_and(|residue| residue.has_peptide_backbone())
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
mod tests;
