use anyhow::{Result, bail};

use crate::domain::{BondType, Structure};

/// UFF atomic parameters.
///
/// Values follow the published Universal Force Field parameters of Rappe et al.,
/// J. Am. Chem. Soc. 1992, 114, 10024-10035.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UffAtomParameters {
    pub key: &'static str,
    pub r1: f32,
    pub theta0_degrees: f32,
    pub x1: f32,
    pub d1: f32,
    pub z1: f32,
    /// sp3 torsional barrier (Vi in UFF paper)
    pub v_sp3: f32,
    /// sp2 torsional barrier (Uj in UFF paper)
    pub v_sp2: f32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TypedAtom {
    pub params: UffAtomParameters,
}

const UFF_PARAMETERS: &[UffAtomParameters] = &[
    UffAtomParameters {
        key: "H_",
        r1: 0.354,
        theta0_degrees: 180.0,
        x1: 2.886,
        d1: 0.044,
        z1: 0.712,
        v_sp3: 0.0,
        v_sp2: 0.0,
    },
    UffAtomParameters {
        key: "C_3",
        r1: 0.757,
        theta0_degrees: 109.47,
        x1: 3.851,
        d1: 0.105,
        z1: 1.912,
        v_sp3: 2.119,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "C_R",
        r1: 0.729,
        theta0_degrees: 120.0,
        x1: 3.851,
        d1: 0.105,
        z1: 1.912,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "C_2",
        r1: 0.732,
        theta0_degrees: 120.0,
        x1: 3.851,
        d1: 0.105,
        z1: 1.912,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "C_1",
        r1: 0.706,
        theta0_degrees: 180.0,
        x1: 3.851,
        d1: 0.105,
        z1: 1.912,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "N_3",
        r1: 0.700,
        theta0_degrees: 106.7,
        x1: 3.660,
        d1: 0.069,
        z1: 2.544,
        v_sp3: 0.45,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "N_R",
        r1: 0.699,
        theta0_degrees: 120.0,
        x1: 3.660,
        d1: 0.069,
        z1: 2.544,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "N_2",
        r1: 0.685,
        theta0_degrees: 111.2,
        x1: 3.660,
        d1: 0.069,
        z1: 2.544,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "N_1",
        r1: 0.656,
        theta0_degrees: 180.0,
        x1: 3.660,
        d1: 0.069,
        z1: 2.544,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "O_3",
        r1: 0.658,
        theta0_degrees: 104.51,
        x1: 3.500,
        d1: 0.060,
        z1: 2.300,
        v_sp3: 0.018,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "O_2",
        r1: 0.634,
        theta0_degrees: 120.0,
        x1: 3.500,
        d1: 0.060,
        z1: 2.300,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "O_R",
        r1: 0.680,
        theta0_degrees: 110.0,
        x1: 3.500,
        d1: 0.060,
        z1: 2.300,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "F_",
        r1: 0.668,
        theta0_degrees: 180.0,
        x1: 3.364,
        d1: 0.050,
        z1: 1.735,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "P_3+5",
        r1: 1.056,
        theta0_degrees: 109.47,
        x1: 4.147,
        d1: 0.305,
        z1: 2.863,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "S_3+2",
        r1: 1.064,
        theta0_degrees: 92.1,
        x1: 4.035,
        d1: 0.274,
        z1: 2.703,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "S_3+4",
        r1: 1.049,
        theta0_degrees: 103.2,
        x1: 4.035,
        d1: 0.274,
        z1: 2.703,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "S_3+6",
        r1: 1.027,
        theta0_degrees: 109.47,
        x1: 4.035,
        d1: 0.274,
        z1: 2.703,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "S_R",
        r1: 1.077,
        theta0_degrees: 92.2,
        x1: 4.035,
        d1: 0.274,
        z1: 2.703,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "S_2",
        r1: 0.854,
        theta0_degrees: 120.0,
        x1: 4.035,
        d1: 0.274,
        z1: 2.703,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "Cl",
        r1: 1.044,
        theta0_degrees: 180.0,
        x1: 3.947,
        d1: 0.227,
        z1: 2.348,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "Br",
        r1: 1.192,
        theta0_degrees: 180.0,
        x1: 4.189,
        d1: 0.251,
        z1: 2.519,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "I_",
        r1: 1.382,
        theta0_degrees: 180.0,
        x1: 4.500,
        d1: 0.339,
        z1: 2.650,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
];

pub(crate) fn typed_atoms(structure: &Structure) -> Result<Vec<TypedAtom>> {
    let neighbors = bonded_neighbors(structure);

    structure
        .atoms
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let key = uff_type_for_atom(structure, &neighbors, index)?;
            let params = parameter_by_key(key)
                .ok_or_else(|| anyhow::anyhow!("missing UFF parameters for atom type {key}"))?;

            Ok(TypedAtom { params })
        })
        .collect()
}

pub(crate) fn bonded_neighbors(structure: &Structure) -> Vec<Vec<(usize, BondType)>> {
    let mut neighbors = vec![Vec::new(); structure.atoms.len()];

    for bond in &structure.bonds {
        neighbors[bond.a].push((bond.b, bond.bond_type));
        neighbors[bond.b].push((bond.a, bond.bond_type));
    }

    neighbors
}

fn uff_type_for_atom(
    structure: &Structure,
    neighbors: &[Vec<(usize, BondType)>],
    atom_index: usize,
) -> Result<&'static str> {
    let element = structure.atoms[atom_index].element.as_str();
    let atom_neighbors = &neighbors[atom_index];

    match element {
        "H" => Ok("H_"),
        "C" if has_bond_type(atom_neighbors, BondType::Aromatic) => Ok("C_R"),
        "C" if has_bond_type(atom_neighbors, BondType::Triple) => Ok("C_1"),
        // Two double bonds on one carbon means a cumulene/allene centre (also
        // CO2, ketene, isocyanate): sp-hybridised and linear, not bent sp2.
        "C" if count_bond_type(atom_neighbors, BondType::Double) >= 2 => Ok("C_1"),
        "C" if has_bond_type(atom_neighbors, BondType::Double) => Ok("C_2"),
        "C" => Ok("C_3"),
        "N" if has_bond_type(atom_neighbors, BondType::Aromatic) => Ok("N_R"),
        "N" if has_bond_type(atom_neighbors, BondType::Triple) => Ok("N_1"),
        // Likewise a nitrogen with two double bonds (azide/diazo centre) is sp.
        "N" if count_bond_type(atom_neighbors, BondType::Double) >= 2 => Ok("N_1"),
        "N" if has_bond_type(atom_neighbors, BondType::Double) => Ok("N_2"),
        "N" => Ok("N_3"),
        "O" if has_bond_type(atom_neighbors, BondType::Aromatic) => Ok("O_R"),
        "O" if has_bond_type(atom_neighbors, BondType::Double) => Ok("O_2"),
        "O" => Ok("O_3"),
        "F" => Ok("F_"),
        "P" => Ok("P_3+5"),
        "S" if has_bond_type(atom_neighbors, BondType::Aromatic) => Ok("S_R"),
        "S" if has_bond_type(atom_neighbors, BondType::Double) => Ok("S_2"),
        "S" if atom_neighbors.len() >= 4 => Ok("S_3+6"),
        "S" if atom_neighbors.len() == 3 => Ok("S_3+4"),
        "S" => Ok("S_3+2"),
        "Cl" => Ok("Cl"),
        "Br" => Ok("Br"),
        "I" => Ok("I_"),
        _ => bail!("unsupported element for UFF atom typing: {element}"),
    }
}

fn has_bond_type(neighbors: &[(usize, BondType)], bond_type: BondType) -> bool {
    neighbors.iter().any(|(_, ty)| *ty == bond_type)
}

fn count_bond_type(neighbors: &[(usize, BondType)], bond_type: BondType) -> usize {
    neighbors.iter().filter(|(_, ty)| *ty == bond_type).count()
}

fn parameter_by_key(key: &str) -> Option<UffAtomParameters> {
    UFF_PARAMETERS
        .iter()
        .copied()
        .find(|params| params.key == key)
}

pub(crate) fn default_parameters_for_element(element: &str) -> Option<UffAtomParameters> {
    let key = match element {
        "H" => "H_",
        "C" => "C_3",
        "N" => "N_3",
        "O" => "O_3",
        "F" => "F_",
        "P" => "P_3+5",
        "S" => "S_3+2",
        "Cl" => "Cl",
        "Br" => "Br",
        "I" => "I_",
        _ => return None,
    };

    parameter_by_key(key)
}
