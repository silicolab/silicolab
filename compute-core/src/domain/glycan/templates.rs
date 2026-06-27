use std::collections::HashMap;

use nalgebra::{Point3, Rotation3, Unit, Vector3};

use super::dictionary::{self, MonosaccharideEntry};
use super::{AbsConfig, Anomer, Monosaccharide, SugarKind};
use crate::domain::BondType;

#[derive(Debug, Clone)]
pub struct TemplateAtom {
    pub name: String,
    pub element: String,
    pub position: Point3<f32>,
}

#[derive(Debug, Clone)]
pub struct TemplateBond {
    pub a: usize,
    pub b: usize,
    pub bond_type: BondType,
}

#[derive(Debug, Clone)]
pub struct CoordinationSite {
    pub binding_atom: usize,
    pub coordination_position: Vector3<f32>,
}

#[derive(Debug, Clone)]
pub struct RingTemplate {
    pub atoms: Vec<TemplateAtom>,
    pub bonds: Vec<TemplateBond>,
    pub donor_site: CoordinationSite,
    pub acceptor_sites: Vec<(u8, CoordinationSite)>,
}

// A regular `4C1` chair: six ring atoms on a hexagon of radius 1.446 Å with the
// puckering height alternating by ±0.234 Å. That yields uniform 1.52 Å ring
// bonds at 111° internal angles — close enough to ideal pyranose geometry that
// the satellites placed off it land clash-free without an energy minimization.
const CHAIR_COORDS: [(f32, f32, f32); 6] = [
    (1.446, 0.0, 0.234),
    (0.723, 1.252, -0.234),
    (-0.723, 1.252, 0.234),
    (-1.446, 0.0, -0.234),
    (-0.723, -1.252, 0.234),
    (0.723, -1.252, -0.234),
];

const HEXOSE_RING: [&str; 6] = ["C1", "C2", "C3", "C4", "C5", "O5"];

const SIALIC_RING: [&str; 6] = ["C2", "C3", "C4", "C5", "C6", "O6"];

fn ring_order(mono: Monosaccharide) -> [&'static str; 6] {
    if matches!(mono.kind, SugarKind::Neu5Ac | SugarKind::Neu5Gc) {
        SIALIC_RING
    } else {
        HEXOSE_RING
    }
}

fn ring_position(name: &str, mono: Monosaccharide) -> Option<Point3<f32>> {
    ring_order(mono)
        .iter()
        .position(|atom| *atom == name)
        .map(|i| {
            let (x, y, z) = CHAIR_COORDS[i];
            Point3::new(x, y, z)
        })
}

fn carbon_index(ring_atom: &str, mono: Monosaccharide) -> Option<usize> {
    ring_order(mono)
        .iter()
        .position(|atom| *atom == ring_atom && atom.starts_with('C'))
}

fn outward_direction(carbon_index: usize, axial: bool) -> Vector3<f32> {
    let angle = std::f32::consts::FRAC_PI_3 * carbon_index as f32;
    let radial: Vector3<f32> = Vector3::new(angle.cos(), angle.sin(), 0.0);
    let sign = if axial { 1.0 } else { -1.0 };
    let vertical: Vector3<f32> = Vector3::new(0.0, 0.0, sign);
    (radial + 0.6 * vertical).normalize()
}

pub fn ring_template(mono: Monosaccharide) -> Option<RingTemplate> {
    let entry = entry_for(mono)?;
    Some(build_pyranose(entry, mono))
}

fn entry_for(mono: Monosaccharide) -> Option<MonosaccharideEntry> {
    dictionary::supported_tokens()
        .into_iter()
        .filter_map(dictionary::lookup)
        .find(|entry| entry.mono == mono)
}

fn build_pyranose(entry: MonosaccharideEntry, mono: Monosaccharide) -> RingTemplate {
    let mut atoms: Vec<TemplateAtom> = Vec::new();
    let mut index_by_name: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    let mut bonds: Vec<TemplateBond> = Vec::new();

    let positions = idealized_positions(entry, mono);

    for &name in entry.atoms {
        let position = positions.get(name).copied().unwrap_or_else(Point3::origin);
        let element = element_for(name);
        index_by_name.insert(name, atoms.len());
        atoms.push(TemplateAtom {
            name: name.to_string(),
            element,
            position,
        });
    }

    for (a, b) in roster_bonds(entry, mono) {
        if let (Some(&ai), Some(&bi)) = (index_by_name.get(a), index_by_name.get(b)) {
            bonds.push(TemplateBond {
                a: ai,
                b: bi,
                bond_type: BondType::Single,
            });
        }
    }

    let anomeric = entry.anomeric_carbon;
    let anomeric_carbon = format!("C{anomeric}");
    let donor_binding = *index_by_name
        .get(anomeric_carbon.as_str())
        .or_else(|| index_by_name.get("C1"))
        .unwrap_or(&0);
    let donor_dir = outward_direction(0, mono.anomer == Anomer::Alpha);
    let donor_site = CoordinationSite {
        binding_atom: donor_binding,
        coordination_position: donor_dir,
    };

    let mut acceptor_sites: Vec<(u8, CoordinationSite)> = Vec::new();
    for parent_pos in [2u8, 3, 4, 6] {
        let oxygen = format!("O{parent_pos}");
        let Some(&binding) = index_by_name.get(oxygen.as_str()) else {
            continue;
        };
        let carbon = format!("C{parent_pos}");
        let dir = match carbon_index(carbon.as_str(), mono) {
            Some(ci) => outward_direction(ci, false),
            None => Vector3::new(0.0, 0.0, 1.0),
        };
        acceptor_sites.push((
            parent_pos,
            CoordinationSite {
                binding_atom: binding,
                coordination_position: dir,
            },
        ));
    }

    RingTemplate {
        atoms,
        bonds,
        donor_site,
        acceptor_sites,
    }
}

/// Place every roster atom at idealized internal-coordinate geometry. Ring atoms
/// seed the chair; every substituent is then completed off its already-placed
/// neighbours at standard covalent bond lengths and sp3/sp2 angles by walking the
/// roster bond graph outward, breadth-first. The single-residue output is clean
/// apart from the multi-tailed sialic acids, whose acetamido and glycerol arms are
/// built pointing toward each other and rely on the assembly-stage rigid-fragment
/// declash to swing apart — no energy minimization is used anywhere.
fn idealized_positions(
    entry: MonosaccharideEntry,
    mono: Monosaccharide,
) -> HashMap<&'static str, Point3<f32>> {
    let adjacency = adjacency(entry, mono);
    let mut pos: HashMap<&'static str, Point3<f32>> = HashMap::new();

    for &name in entry.atoms {
        if let Some(point) = ring_position(name, mono) {
            pos.insert(name, point);
        }
    }

    // Each pass completes the unplaced neighbours of every placed centre whose
    // local frame is already determined. A connected roster converges in a few
    // passes (ring -> first shell -> exocyclic chains).
    loop {
        let mut round: Vec<(&'static str, Point3<f32>)> = Vec::new();
        let mut claimed: std::collections::HashSet<&'static str> = std::collections::HashSet::new();
        for &center in entry.atoms {
            let Some(&center_pos) = pos.get(center) else {
                continue;
            };
            let unplaced: Vec<&'static str> = adjacency[center]
                .iter()
                .copied()
                .filter(|n| !pos.contains_key(n) && !claimed.contains(n))
                .collect();
            if unplaced.is_empty() {
                continue;
            }
            for (name, point) in
                place_neighbors(center, center_pos, &unplaced, &adjacency, &pos, entry, mono)
            {
                if claimed.insert(name) {
                    round.push((name, point));
                }
            }
        }
        if round.is_empty() {
            break;
        }
        for (name, point) in round {
            pos.entry(name).or_insert(point);
        }
    }

    // Defensive: a correct roster is fully connected, but never hand a caller a
    // NaN if some atom was unreachable — pin it to the ring centroid.
    for &name in entry.atoms {
        pos.entry(name).or_insert_with(Point3::origin);
    }

    pos
}

/// Deduplicated bond adjacency over the residue's roster.
fn adjacency(
    entry: MonosaccharideEntry,
    mono: Monosaccharide,
) -> HashMap<&'static str, Vec<&'static str>> {
    let mut adj: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
    for &name in entry.atoms {
        adj.entry(name).or_default();
    }
    for (a, b) in roster_bonds(entry, mono) {
        if !entry.atoms.contains(&a) || !entry.atoms.contains(&b) {
            continue;
        }
        let neighbors_a = adj.get_mut(a).expect("atom present");
        if !neighbors_a.contains(&b) {
            neighbors_a.push(b);
        }
        let neighbors_b = adj.get_mut(b).expect("atom present");
        if !neighbors_b.contains(&a) {
            neighbors_b.push(a);
        }
    }
    adj
}

/// Position the unplaced neighbours of `center` from its already-placed bonds.
fn place_neighbors(
    center: &'static str,
    center_pos: Point3<f32>,
    unplaced: &[&'static str],
    adjacency: &HashMap<&'static str, Vec<&'static str>>,
    pos: &HashMap<&'static str, Point3<f32>>,
    entry: MonosaccharideEntry,
    mono: Monosaccharide,
) -> Vec<(&'static str, Point3<f32>)> {
    let placed_dirs: Vec<Vector3<f32>> = adjacency[center]
        .iter()
        .filter_map(|n| pos.get(n))
        .filter_map(|p| (p - center_pos).try_normalize(1.0e-5))
        .collect();

    // Ring carbons carry one axial + one equatorial substituent; place them with
    // the sugar's stereochemistry so epimers and anomers come out distinct.
    if is_ring_atom(center, mono) && element_for(center) == "C" && placed_dirs.len() == 2 {
        return place_ring_substituents(
            center,
            center_pos,
            unplaced,
            placed_dirs[0],
            placed_dirs[1],
            entry,
            mono,
        );
    }

    let reference = grandparent_reference(center, center_pos, adjacency, pos);
    let trigonal = is_trigonal_center(center, mono);
    let dirs: Vec<Vector3<f32>> = if trigonal {
        if placed_dirs.len() >= 2 {
            vec![sp2_third(placed_dirs[0], placed_dirs[1])]
        } else if placed_dirs.len() == 1 {
            sp2_open_pair(placed_dirs[0], reference).to_vec()
        } else {
            sp2_open_pair(Vector3::z(), Vector3::x()).to_vec()
        }
    } else if placed_dirs.len() >= 3 {
        vec![tetrahedral_fourth(
            placed_dirs[0],
            placed_dirs[1],
            placed_dirs[2],
        )]
    } else if placed_dirs.len() == 2 {
        let (a, b) = tetrahedral_complete_two(placed_dirs[0], placed_dirs[1]);
        vec![a, b]
    } else if placed_dirs.len() == 1 {
        tetrahedral_open_triple(placed_dirs[0], reference).to_vec()
    } else {
        tetrahedral_open_triple(Vector3::z(), Vector3::x()).to_vec()
    };

    unplaced
        .iter()
        .zip(dirs.iter())
        .map(|(&name, &dir)| (name, center_pos + dir * bond_length(center, name, trigonal)))
        .collect()
}

/// Place a ring carbon's two exocyclic substituents (one axial, one equatorial).
fn place_ring_substituents(
    center: &'static str,
    center_pos: Point3<f32>,
    unplaced: &[&'static str],
    ring_dir_a: Vector3<f32>,
    ring_dir_b: Vector3<f32>,
    entry: MonosaccharideEntry,
    mono: Monosaccharide,
) -> Vec<(&'static str, Point3<f32>)> {
    let (slot_one, slot_two) = tetrahedral_complete_two(ring_dir_a, ring_dir_b);
    // The chair's normal is ~z, so the more vertical completion is the axial bond.
    let (axial, equatorial) = if slot_one.z.abs() >= slot_two.z.abs() {
        (slot_one, slot_two)
    } else {
        (slot_two, slot_one)
    };

    let anomeric_carbon = if entry.anomeric_carbon == 2 {
        "C2"
    } else {
        "C1"
    };
    let anomeric_oxygen = if entry.anomeric_carbon == 2 {
        "O2"
    } else {
        "O1"
    };

    let mut placed = Vec::with_capacity(unplaced.len());
    if center == anomeric_carbon {
        // The glycosidic oxygen's face inverts with the absolute configuration
        // exactly like every other ring carbon: axial for α-D / β-L, equatorial for
        // β-D / α-L. Its partner (the ring H, or the sialic carboxyl carbon) takes
        // the remaining slot.
        let oxygen_axial = (mono.anomer == Anomer::Alpha) == (mono.config == AbsConfig::D);
        let (oxygen_dir, partner_dir) = if oxygen_axial {
            (axial, equatorial)
        } else {
            (equatorial, axial)
        };
        for &name in unplaced {
            let dir = if name == anomeric_oxygen {
                oxygen_dir
            } else {
                partner_dir
            };
            placed.push((name, center_pos + dir * bond_length(center, name, false)));
        }
    } else {
        let heavy = unplaced.iter().copied().find(|n| element_for(n) != "H");
        let (substituent_dir, hydrogen_dir) = if substituent_axial(center, mono) {
            (axial, equatorial)
        } else {
            (equatorial, axial)
        };
        let mut spare = [axial, equatorial].into_iter();
        for &name in unplaced {
            let dir = if Some(name) == heavy {
                substituent_dir
            } else if heavy.is_some() {
                hydrogen_dir
            } else {
                spare.next().unwrap_or(axial)
            };
            placed.push((name, center_pos + dir * bond_length(center, name, false)));
        }
    }
    placed
}

fn is_ring_atom(name: &str, mono: Monosaccharide) -> bool {
    ring_order(mono).contains(&name)
}

/// Trigonal (sp2, ~120°, planar) centres: the acetamido carbonyl carbon and amide
/// nitrogen, plus the carboxyl/carboxylate carbons of sialic and uronic acids.
fn is_trigonal_center(name: &str, mono: Monosaccharide) -> bool {
    let sialic = matches!(mono.kind, SugarKind::Neu5Ac | SugarKind::Neu5Gc);
    let uronate = matches!(
        mono.kind,
        SugarKind::GlcA | SugarKind::IdoA | SugarKind::GalA
    );
    match name {
        "C" | "N" => true,
        "C1" => sialic,
        "C6" => uronate,
        _ => false,
    }
}

/// Standard covalent bond lengths (Å); carbonyl/carboxylate C=O is shortened.
fn bond_length(center: &str, neighbor: &str, center_trigonal: bool) -> f32 {
    let a = element_for(center);
    let b = element_for(neighbor);
    if center_trigonal && a == "C" && b == "O" {
        return 1.25;
    }
    match (a.as_str(), b.as_str()) {
        ("C", "H") | ("H", "C") => 1.10,
        ("O", "H") | ("H", "O") => 0.97,
        ("N", "H") | ("H", "N") => 1.01,
        ("C", "O") | ("O", "C") => 1.43,
        ("C", "N") | ("N", "C") => 1.46,
        ("C", "C") => 1.52,
        _ => 1.45,
    }
}

/// Direction toward a placed grandparent, so tripods and sp2 planes stagger
/// against the existing chain rather than landing at an arbitrary azimuth.
fn grandparent_reference(
    center: &str,
    center_pos: Point3<f32>,
    adjacency: &HashMap<&'static str, Vec<&'static str>>,
    pos: &HashMap<&'static str, Point3<f32>>,
) -> Vector3<f32> {
    for &parent in &adjacency[center] {
        let Some(&parent_pos) = pos.get(parent) else {
            continue;
        };
        for &grand in &adjacency[parent] {
            if grand == center {
                continue;
            }
            if let Some(&grand_pos) = pos.get(grand)
                && let Some(dir) = (grand_pos - center_pos).try_normalize(1.0e-5)
            {
                return dir;
            }
        }
        if let Some(dir) = (parent_pos - center_pos).try_normalize(1.0e-5) {
            return dir;
        }
    }
    Vector3::x()
}

/// The two sp3 bond directions completing a tetrahedral centre whose other two
/// bonds point along `a` and `b`.
fn tetrahedral_complete_two(a: Vector3<f32>, b: Vector3<f32>) -> (Vector3<f32>, Vector3<f32>) {
    const COS_HALF: f32 = 0.577_350_3; // cos(54.7356°)
    const SIN_HALF: f32 = 0.816_496_6; // sin(54.7356°)
    let bisector = (-(a + b))
        .try_normalize(1.0e-5)
        .unwrap_or_else(|| arbitrary_perpendicular(a));
    let normal = a
        .cross(&b)
        .try_normalize(1.0e-5)
        .unwrap_or_else(|| arbitrary_perpendicular(bisector));
    (
        COS_HALF * bisector + SIN_HALF * normal,
        COS_HALF * bisector - SIN_HALF * normal,
    )
}

/// The three sp3 directions completing a centre with a single existing bond
/// `axis`, staggered about it using `reference`.
fn tetrahedral_open_triple(axis: Vector3<f32>, reference: Vector3<f32>) -> [Vector3<f32>; 3] {
    const COS_T: f32 = -0.333_333_3; // cos(109.47°)
    const SIN_T: f32 = 0.942_809; // sin(109.47°)
    let (u, v) = perpendicular_basis(axis, reference);
    let mut directions = [Vector3::zeros(); 3];
    for (k, slot) in directions.iter_mut().enumerate() {
        let phi = std::f32::consts::PI + k as f32 * std::f32::consts::TAU / 3.0;
        *slot = axis * COS_T + (u * phi.cos() + v * phi.sin()) * SIN_T;
    }
    directions
}

/// The remaining sp3 direction of a centre with three existing bonds.
fn tetrahedral_fourth(a: Vector3<f32>, b: Vector3<f32>, c: Vector3<f32>) -> Vector3<f32> {
    (-(a + b + c))
        .try_normalize(1.0e-5)
        .unwrap_or_else(|| arbitrary_perpendicular(a))
}

/// The third in-plane bond of a trigonal centre with two existing bonds.
fn sp2_third(a: Vector3<f32>, b: Vector3<f32>) -> Vector3<f32> {
    (-(a + b))
        .try_normalize(1.0e-5)
        .unwrap_or_else(|| arbitrary_perpendicular(a))
}

/// Two in-plane bonds 120° off `axis` for a trigonal centre with one existing
/// bond, coplanar with `axis` and `reference`.
fn sp2_open_pair(axis: Vector3<f32>, reference: Vector3<f32>) -> [Vector3<f32>; 2] {
    let normal = axis
        .cross(&reference)
        .try_normalize(1.0e-5)
        .unwrap_or_else(|| arbitrary_perpendicular(axis));
    let rotation = Rotation3::from_axis_angle(&Unit::new_normalize(normal), 120.0_f32.to_radians());
    [rotation * axis, rotation.inverse() * axis]
}

fn arbitrary_perpendicular(v: Vector3<f32>) -> Vector3<f32> {
    let helper = if v.x.abs() < 0.9 {
        Vector3::x()
    } else {
        Vector3::y()
    };
    v.cross(&helper)
        .try_normalize(1.0e-5)
        .unwrap_or_else(Vector3::z)
}

fn perpendicular_basis(
    axis: Vector3<f32>,
    reference: Vector3<f32>,
) -> (Vector3<f32>, Vector3<f32>) {
    let u = (reference - axis * axis.dot(&reference))
        .try_normalize(1.0e-5)
        .unwrap_or_else(|| arbitrary_perpendicular(axis));
    let v = axis.cross(&u);
    (u, v)
}

fn substituent_axial(carbon: &str, mono: Monosaccharide) -> bool {
    let epimeric = matches!(
        (mono.kind, carbon),
        (SugarKind::Gal, "C4")
            | (SugarKind::GalNAc, "C4")
            | (SugarKind::GalA, "C4")
            // Fucose is 6-deoxy-L-galactose, so it carries the galacto C4 config.
            | (SugarKind::Fuc, "C4")
            | (SugarKind::Man, "C2")
            | (SugarKind::ManNAc, "C2")
            | (SugarKind::IdoA, "C5")
    );
    if mono.config == AbsConfig::L {
        return !epimeric;
    }
    epimeric
}

fn element_for(name: &str) -> String {
    if name.starts_with("HO") || name.starts_with("HN") || name.starts_with("HT") {
        return "H".to_string();
    }
    let first = name.chars().next().unwrap_or('C');
    match first {
        'C' => "C".to_string(),
        'O' => "O".to_string(),
        'N' => "N".to_string(),
        'H' => "H".to_string(),
        other => other.to_string(),
    }
}

fn roster_bonds(
    entry: MonosaccharideEntry,
    mono: Monosaccharide,
) -> Vec<(&'static str, &'static str)> {
    let has = |name: &str| entry.atoms.contains(&name);
    let sialic = matches!(mono.kind, SugarKind::Neu5Ac | SugarKind::Neu5Gc);
    let mut bonds = if sialic {
        vec![
            ("C2", "O6"),
            ("C2", "C3"),
            ("C3", "C4"),
            ("C4", "C5"),
            ("C5", "C6"),
            ("C6", "O6"),
        ]
    } else {
        vec![
            ("C1", "O5"),
            ("C1", "C2"),
            ("C2", "C3"),
            ("C3", "C4"),
            ("C4", "C5"),
            ("C5", "O5"),
        ]
    };
    // The N-acetyl/N-glycolyl nitrogen attaches to C2 on hexosamines but to C5 on
    // sialic acids. Both rosters carry C2, C5 and N, so this must key off the sugar
    // class rather than mere atom presence — otherwise the nitrogen picks up a
    // spurious second ring bond.
    if has("N") {
        bonds.push(if sialic { ("C5", "N") } else { ("C2", "N") });
    }
    for (a, b) in [
        ("C1", "H1"),
        ("C1", "O1"),
        ("O1", "HO1"),
        ("C1", "O11"),
        ("C1", "O12"),
        ("C2", "C1"),
        ("C2", "H2"),
        ("C2", "O2"),
        ("O2", "HO2"),
        ("N", "HN"),
        ("N", "C"),
        ("C", "O"),
        ("C", "CT"),
        ("CT", "HT1"),
        ("CT", "HT2"),
        ("CT", "HT3"),
        ("C", "C10"),
        ("C10", "H10"),
        ("C10", "H11"),
        ("C10", "O13"),
        ("O13", "HO13"),
        ("C3", "H3"),
        ("C3", "H31"),
        ("C3", "H32"),
        ("C3", "O3"),
        ("O3", "HO3"),
        ("C4", "H4"),
        ("C4", "O4"),
        ("O4", "HO4"),
        ("C5", "H5"),
        ("C5", "C6"),
        ("C5", "H51"),
        ("C5", "H52"),
        ("C6", "H6"),
        ("C6", "H61"),
        ("C6", "H62"),
        ("C6", "H63"),
        ("C6", "O6"),
        ("O6", "HO6"),
        ("C6", "O61"),
        ("C6", "O62"),
        ("C6", "C7"),
        ("C7", "H7"),
        ("C7", "O7"),
        ("O7", "HO7"),
        ("C7", "C8"),
        ("C8", "H8"),
        ("C8", "O8"),
        ("O8", "HO8"),
        ("C8", "C9"),
        ("C9", "H91"),
        ("C9", "H92"),
        ("C9", "O9"),
        ("O9", "HO9"),
    ] {
        if has(a) && has(b) {
            bonds.push((a, b));
        }
    }
    bonds
}

#[cfg(test)]
mod tests;
