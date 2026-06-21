use nalgebra::Point3;

use crate::domain::BondType;

#[derive(Debug, Clone, Copy)]
pub struct ElementStyle {
    pub color: Point3<f32>,
    pub covalent_radius: f32,
    pub display_radius: f32,
}

/// Whether an element is commonly present as a free monatomic ion in molecular
/// systems (counter-ions, salt baths, structural metals). Used to classify lone
/// atoms that are not part of a polymer or solvent residue.
pub fn is_monatomic_ion_element(symbol: &str) -> bool {
    matches!(
        symbol,
        "Li" | "Na"
            | "K"
            | "Rb"
            | "Cs"
            | "Mg"
            | "Ca"
            | "Sr"
            | "Ba"
            | "Zn"
            | "Fe"
            | "Cu"
            | "Mn"
            | "Cl"
            | "Br"
            | "I"
            | "F"
    )
}

pub fn element_style(symbol: &str) -> ElementStyle {
    match normalized_symbol(symbol).as_str() {
        "H" => ElementStyle {
            color: Point3::new(0.95, 0.95, 0.95),
            covalent_radius: 0.31,
            display_radius: 0.22,
        },
        "He" => ElementStyle {
            color: Point3::new(0.85, 1.00, 1.00),
            covalent_radius: 0.28,
            display_radius: 0.20,
        },
        "Li" => ElementStyle {
            color: Point3::new(0.80, 0.00, 0.00),
            covalent_radius: 1.28,
            display_radius: 0.44,
        },
        "Be" => ElementStyle {
            color: Point3::new(0.00, 0.50, 0.00),
            covalent_radius: 0.96,
            display_radius: 0.38,
        },
        "B" => ElementStyle {
            color: Point3::new(1.00, 0.71, 0.71),
            covalent_radius: 0.84,
            display_radius: 0.36,
        },
        "C" => ElementStyle {
            color: Point3::new(0.30, 0.30, 0.32),
            covalent_radius: 0.76,
            display_radius: 0.34,
        },
        "N" => ElementStyle {
            color: Point3::new(0.10, 0.25, 0.95),
            covalent_radius: 0.71,
            display_radius: 0.33,
        },
        "O" => ElementStyle {
            color: Point3::new(0.90, 0.05, 0.05),
            covalent_radius: 0.66,
            display_radius: 0.34,
        },
        "F" => ElementStyle {
            color: Point3::new(0.45, 0.90, 0.25),
            covalent_radius: 0.57,
            display_radius: 0.32,
        },
        "Ne" => ElementStyle {
            color: Point3::new(0.70, 0.89, 0.96),
            covalent_radius: 0.58,
            display_radius: 0.30,
        },
        "Na" => ElementStyle {
            color: Point3::new(0.45, 0.35, 0.90),
            covalent_radius: 1.66,
            display_radius: 0.48,
        },
        "Mg" => ElementStyle {
            color: Point3::new(0.00, 0.59, 0.00),
            covalent_radius: 1.41,
            display_radius: 0.46,
        },
        "Al" => ElementStyle {
            color: Point3::new(0.75, 0.65, 0.65),
            covalent_radius: 1.21,
            display_radius: 0.44,
        },
        "Si" => ElementStyle {
            color: Point3::new(0.94, 0.78, 0.63),
            covalent_radius: 1.11,
            display_radius: 0.42,
        },
        "P" => ElementStyle {
            color: Point3::new(1.00, 0.55, 0.10),
            covalent_radius: 1.07,
            display_radius: 0.42,
        },
        "S" => ElementStyle {
            color: Point3::new(0.95, 0.82, 0.10),
            covalent_radius: 1.05,
            display_radius: 0.42,
        },
        "Cl" => ElementStyle {
            color: Point3::new(0.10, 0.75, 0.20),
            covalent_radius: 1.02,
            display_radius: 0.42,
        },
        "Ar" => ElementStyle {
            color: Point3::new(0.50, 0.82, 0.89),
            covalent_radius: 1.06,
            display_radius: 0.38,
        },
        "K" => ElementStyle {
            color: Point3::new(0.56, 0.00, 0.56),
            covalent_radius: 2.03,
            display_radius: 0.52,
        },
        "Ca" => ElementStyle {
            color: Point3::new(0.00, 0.59, 0.00),
            covalent_radius: 1.76,
            display_radius: 0.50,
        },
        "Sc" => ElementStyle {
            color: Point3::new(0.90, 0.90, 0.90),
            covalent_radius: 1.70,
            display_radius: 0.46,
        },
        "Ti" => ElementStyle {
            color: Point3::new(0.75, 0.76, 0.78),
            covalent_radius: 1.36,
            display_radius: 0.46,
        },
        "V" => ElementStyle {
            color: Point3::new(0.65, 0.65, 0.67),
            covalent_radius: 1.25,
            display_radius: 0.44,
        },
        "Cr" => ElementStyle {
            color: Point3::new(0.54, 0.60, 0.78),
            covalent_radius: 1.39,
            display_radius: 0.46,
        },
        "Mn" => ElementStyle {
            color: Point3::new(0.61, 0.48, 0.69),
            covalent_radius: 1.39,
            display_radius: 0.46,
        },
        "Fe" => ElementStyle {
            color: Point3::new(0.88, 0.40, 0.20),
            covalent_radius: 1.32,
            display_radius: 0.44,
        },
        "Co" => ElementStyle {
            color: Point3::new(0.94, 0.48, 0.40),
            covalent_radius: 1.26,
            display_radius: 0.44,
        },
        "Ni" => ElementStyle {
            color: Point3::new(0.31, 0.62, 0.31),
            covalent_radius: 1.24,
            display_radius: 0.44,
        },
        "Cu" => ElementStyle {
            color: Point3::new(0.78, 0.50, 0.20),
            covalent_radius: 1.32,
            display_radius: 0.44,
        },
        "Zn" => ElementStyle {
            color: Point3::new(0.49, 0.50, 0.55),
            covalent_radius: 1.22,
            display_radius: 0.44,
        },
        "Ga" => ElementStyle {
            color: Point3::new(0.75, 0.65, 0.65),
            covalent_radius: 1.22,
            display_radius: 0.44,
        },
        "Ge" => ElementStyle {
            color: Point3::new(0.40, 0.56, 0.56),
            covalent_radius: 1.20,
            display_radius: 0.42,
        },
        "As" => ElementStyle {
            color: Point3::new(0.74, 0.50, 0.89),
            covalent_radius: 1.19,
            display_radius: 0.42,
        },
        "Se" => ElementStyle {
            color: Point3::new(0.78, 0.48, 0.00),
            covalent_radius: 1.20,
            display_radius: 0.42,
        },
        "Br" => ElementStyle {
            color: Point3::new(0.65, 0.16, 0.16),
            covalent_radius: 1.20,
            display_radius: 0.42,
        },
        "Kr" => ElementStyle {
            color: Point3::new(0.36, 0.72, 0.82),
            covalent_radius: 1.16,
            display_radius: 0.38,
        },
        "Rb" => ElementStyle {
            color: Point3::new(0.44, 0.18, 0.69),
            covalent_radius: 2.20,
            display_radius: 0.54,
        },
        "Sr" => ElementStyle {
            color: Point3::new(0.00, 0.59, 0.00),
            covalent_radius: 1.95,
            display_radius: 0.52,
        },
        "Y" => ElementStyle {
            color: Point3::new(0.58, 1.00, 1.00),
            covalent_radius: 1.90,
            display_radius: 0.48,
        },
        "Zr" => ElementStyle {
            color: Point3::new(0.59, 0.58, 0.58),
            covalent_radius: 1.75,
            display_radius: 0.48,
        },
        "Nb" => ElementStyle {
            color: Point3::new(0.45, 0.76, 0.79),
            covalent_radius: 1.64,
            display_radius: 0.46,
        },
        "Mo" => ElementStyle {
            color: Point3::new(0.33, 0.71, 0.71),
            covalent_radius: 1.54,
            display_radius: 0.46,
        },
        "Ru" => ElementStyle {
            color: Point3::new(0.14, 0.56, 0.56),
            covalent_radius: 1.46,
            display_radius: 0.44,
        },
        "Rh" => ElementStyle {
            color: Point3::new(0.04, 0.49, 0.55),
            covalent_radius: 1.42,
            display_radius: 0.44,
        },
        "Pd" => ElementStyle {
            color: Point3::new(0.00, 0.41, 0.52),
            covalent_radius: 1.39,
            display_radius: 0.44,
        },
        "Ag" => ElementStyle {
            color: Point3::new(0.75, 0.75, 0.75),
            covalent_radius: 1.45,
            display_radius: 0.44,
        },
        "Cd" => ElementStyle {
            color: Point3::new(1.00, 0.85, 0.56),
            covalent_radius: 1.44,
            display_radius: 0.46,
        },
        "In" => ElementStyle {
            color: Point3::new(0.65, 0.46, 0.45),
            covalent_radius: 1.42,
            display_radius: 0.46,
        },
        "Sn" => ElementStyle {
            color: Point3::new(0.40, 0.50, 0.50),
            covalent_radius: 1.39,
            display_radius: 0.46,
        },
        "Sb" => ElementStyle {
            color: Point3::new(0.62, 0.39, 0.71),
            covalent_radius: 1.39,
            display_radius: 0.46,
        },
        "Te" => ElementStyle {
            color: Point3::new(0.83, 0.48, 0.00),
            covalent_radius: 1.38,
            display_radius: 0.46,
        },
        "I" => ElementStyle {
            color: Point3::new(0.58, 0.00, 0.58),
            covalent_radius: 1.39,
            display_radius: 0.46,
        },
        "Xe" => ElementStyle {
            color: Point3::new(0.26, 0.62, 0.69),
            covalent_radius: 1.40,
            display_radius: 0.40,
        },
        "Cs" => ElementStyle {
            color: Point3::new(0.44, 0.18, 0.69),
            covalent_radius: 2.44,
            display_radius: 0.56,
        },
        "Ba" => ElementStyle {
            color: Point3::new(0.00, 0.59, 0.00),
            covalent_radius: 2.15,
            display_radius: 0.54,
        },
        "La" => ElementStyle {
            color: Point3::new(0.44, 0.83, 1.00),
            covalent_radius: 2.07,
            display_radius: 0.50,
        },
        "Ce" => ElementStyle {
            color: Point3::new(1.00, 1.00, 0.78),
            covalent_radius: 2.04,
            display_radius: 0.50,
        },
        "W" => ElementStyle {
            color: Point3::new(0.13, 0.58, 0.13),
            covalent_radius: 1.62,
            display_radius: 0.46,
        },
        "Os" => ElementStyle {
            color: Point3::new(0.14, 0.42, 0.42),
            covalent_radius: 1.44,
            display_radius: 0.44,
        },
        "Ir" => ElementStyle {
            color: Point3::new(0.09, 0.33, 0.53),
            covalent_radius: 1.41,
            display_radius: 0.44,
        },
        "Pt" => ElementStyle {
            color: Point3::new(0.82, 0.82, 0.88),
            covalent_radius: 1.36,
            display_radius: 0.44,
        },
        "Au" => ElementStyle {
            color: Point3::new(1.00, 0.84, 0.00),
            covalent_radius: 1.36,
            display_radius: 0.44,
        },
        "Hg" => ElementStyle {
            color: Point3::new(0.72, 0.72, 0.82),
            covalent_radius: 1.32,
            display_radius: 0.44,
        },
        "Tl" => ElementStyle {
            color: Point3::new(0.65, 0.33, 0.30),
            covalent_radius: 1.45,
            display_radius: 0.46,
        },
        "Pb" => ElementStyle {
            color: Point3::new(0.34, 0.35, 0.38),
            covalent_radius: 1.46,
            display_radius: 0.46,
        },
        "Bi" => ElementStyle {
            color: Point3::new(0.62, 0.31, 0.71),
            covalent_radius: 1.48,
            display_radius: 0.46,
        },
        "U" => ElementStyle {
            color: Point3::new(0.00, 0.56, 0.00),
            covalent_radius: 1.96,
            display_radius: 0.50,
        },
        _ => ElementStyle {
            color: Point3::new(0.55, 0.55, 0.60),
            covalent_radius: 0.80,
            display_radius: 0.35,
        },
    }
}

pub(crate) fn typical_valence(element: &str) -> Option<usize> {
    match normalized_symbol(element).as_str() {
        "C" => Some(4),
        "N" => Some(3),
        "O" => Some(2),
        "P" => Some(3),
        "S" => Some(2),
        "F" | "Cl" | "Br" | "I" | "H" => Some(1),
        _ => None,
    }
}

pub(crate) fn is_aromatic_element(element: &str) -> bool {
    matches!(normalized_symbol(element).as_str(), "C" | "N" | "O" | "S")
}

pub(crate) fn bond_order_value(bond_type: BondType) -> f32 {
    match bond_type {
        BondType::Single => 1.0,
        BondType::Double => 2.0,
        BondType::Triple => 3.0,
        BondType::Aromatic => 1.5,
    }
}

pub fn normalized_symbol(symbol: &str) -> String {
    let mut chars = symbol.trim().chars();
    let Some(first) = chars.next() else {
        return String::new();
    };

    let mut normalized = first.to_uppercase().collect::<String>();

    if let Some(second) = chars.next() {
        normalized.push_str(&second.to_lowercase().collect::<String>());
    }

    normalized
}

/// Every element symbol (H–Og), in proper case.
const ELEMENT_SYMBOLS: &[&str] = &[
    "H", "He", "Li", "Be", "B", "C", "N", "O", "F", "Ne", "Na", "Mg", "Al", "Si", "P", "S", "Cl",
    "Ar", "K", "Ca", "Sc", "Ti", "V", "Cr", "Mn", "Fe", "Co", "Ni", "Cu", "Zn", "Ga", "Ge", "As",
    "Se", "Br", "Kr", "Rb", "Sr", "Y", "Zr", "Nb", "Mo", "Tc", "Ru", "Rh", "Pd", "Ag", "Cd", "In",
    "Sn", "Sb", "Te", "I", "Xe", "Cs", "Ba", "La", "Ce", "Pr", "Nd", "Pm", "Sm", "Eu", "Gd", "Tb",
    "Dy", "Ho", "Er", "Tm", "Yb", "Lu", "Hf", "Ta", "W", "Re", "Os", "Ir", "Pt", "Au", "Hg", "Tl",
    "Pb", "Bi", "Po", "At", "Rn", "Fr", "Ra", "Ac", "Th", "Pa", "U", "Np", "Pu", "Am", "Cm", "Bk",
    "Cf", "Es", "Fm", "Md", "No", "Lr", "Rf", "Db", "Sg", "Bh", "Hs", "Mt", "Ds", "Rg", "Cn", "Nh",
    "Fl", "Mc", "Lv", "Ts", "Og",
];

/// Whether `symbol` is a real chemical element (case-insensitive).
pub fn is_element_symbol(symbol: &str) -> bool {
    let normalized = normalized_symbol(symbol);
    ELEMENT_SYMBOLS.contains(&normalized.as_str())
}

/// Standard atomic weights (u), parallel to [`ELEMENT_SYMBOLS`]. Conventional
/// IUPAC values; the longest-lived isotope's mass is used for elements with no
/// stable form. Used to convert a target mass density to a molecule count when
/// packing a disordered system.
const ATOMIC_MASSES_U: &[f32] = &[
    1.008, 4.0026, 6.94, 9.0122, 10.81, 12.011, 14.007, 15.999, 18.998, 20.180, 22.990, 24.305,
    26.982, 28.085, 30.974, 32.06, 35.45, 39.948, 39.098, 40.078, 44.956, 47.867, 50.942, 51.996,
    54.938, 55.845, 58.933, 58.693, 63.546, 65.38, 69.723, 72.630, 74.922, 78.971, 79.904, 83.798,
    85.468, 87.62, 88.906, 91.224, 92.906, 95.95, 98.0, 101.07, 102.91, 106.42, 107.87, 112.41,
    114.82, 118.71, 121.76, 127.60, 126.90, 131.29, 132.91, 137.33, 138.91, 140.12, 140.91, 144.24,
    145.0, 150.36, 151.96, 157.25, 158.93, 162.50, 164.93, 167.26, 168.93, 173.05, 174.97, 178.49,
    180.95, 183.84, 186.21, 190.23, 192.22, 195.08, 196.97, 200.59, 204.38, 207.2, 208.98, 209.0,
    210.0, 222.0, 223.0, 226.0, 227.0, 232.04, 231.04, 238.03, 237.0, 244.0, 243.0, 247.0, 247.0,
    251.0, 252.0, 257.0, 258.0, 259.0, 262.0, 267.0, 268.0, 269.0, 270.0, 269.0, 278.0, 281.0,
    282.0, 285.0, 286.0, 289.0, 289.0, 293.0, 294.0, 294.0,
];

const _: () = assert!(
    ATOMIC_MASSES_U.len() == ELEMENT_SYMBOLS.len(),
    "atomic-mass table must stay parallel to the element-symbol table",
);

/// The standard atomic weight (u) of an element, or `None` if `symbol` is not a
/// recognized element. Case-insensitive.
pub fn atomic_mass(symbol: &str) -> Option<f32> {
    let normalized = normalized_symbol(symbol);
    ELEMENT_SYMBOLS
        .iter()
        .position(|candidate| *candidate == normalized)
        .map(|index| ATOMIC_MASSES_U[index])
}
