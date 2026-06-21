//! Direct crystallographic generators for 2D material families.
//!
//! Each family produces a primitive hexagonal cell; the sheet is then replicated
//! with [`Structure::make_supercell`] and bonded by periodic neighbour inference
//! restricted to the chemically meaningful element pairs. Inference runs on the
//! replicated sheet (not the primitive) because the minimum-image bond model can
//! only record one bond per atom-index pair — graphene's three C–C contacts are
//! only distinct once the two-atom basis has been repeated in-plane.

use anyhow::{Result, bail};
use nalgebra::{Point3, Vector3};

use crate::domain::{Atom, Bond, Structure, UnitCell, chemistry::infer_bonds_with_cell};

use super::recipe::{
    CarbonNitrideNode, CarbonNitrideParams, HoneycombParams, NanosheetSpec, RING_BOND, SheetKind,
    TmdParams, TmdPolytype,
};

const SQRT3_2: f32 = 0.866_025_4;
/// Lower bound on the c lattice vector, so a sheet always has a non-degenerate cell.
const MIN_C: f32 = 2.0;

pub fn build_nanosheet(spec: &NanosheetSpec) -> Result<Structure> {
    let gap = spec.interlayer_spacing;
    let (atoms, cell, allowed) = match &spec.kind {
        SheetKind::Honeycomb(params) => honeycomb_cell(params, gap)?,
        SheetKind::Tmd(params) => tmd_cell(params, gap)?,
        SheetKind::CarbonNitride(params) => carbon_nitride_cell(params, gap)?,
    };

    let mut structure = Structure::with_cell_and_bonds(spec.name.clone(), atoms, Vec::new(), cell);
    structure.wrap_atoms_into_cell_preserving_bonds();
    structure.make_supercell(spec.supercell);
    structure.bonds = filtered_periodic_bonds(&structure, &allowed);

    Ok(structure)
}

/// A hexagonal cell with the standard 60° in-plane vectors and a c axis along z.
fn hexagonal_cell(a: f32, c: f32) -> UnitCell {
    UnitCell::from_vectors([
        Vector3::new(a, 0.0, 0.0),
        Vector3::new(a * 0.5, a * SQRT3_2, 0.0),
        Vector3::new(0.0, 0.0, c),
    ])
}

fn atom(element: &str, position: Point3<f32>) -> Atom {
    Atom {
        element: element.to_string(),
        position,
        charge: 0.0,
    }
}

fn polar(radius: f32, degrees: f32) -> Vector3<f32> {
    let theta = degrees.to_radians();
    Vector3::new(radius * theta.cos(), radius * theta.sin(), 0.0)
}

fn require_element(label: &str, element: &str) -> Result<()> {
    if element.trim().is_empty() {
        bail!("{label} element must be set");
    }
    Ok(())
}

/// Bonds inferred across the periodic cell, keeping only the element pairs that
/// are actually bonded in the material (e.g. Mo–S but not the metal–metal
/// contacts that bare covalent radii would otherwise report).
fn filtered_periodic_bonds(structure: &Structure, allowed: &[(String, String)]) -> Vec<Bond> {
    infer_bonds_with_cell(&structure.atoms, structure.cell.as_ref())
        .into_iter()
        .filter(|bond| {
            let first = &structure.atoms[bond.a].element;
            let second = &structure.atoms[bond.b].element;
            allowed
                .iter()
                .any(|(x, y)| (x == first && y == second) || (x == second && y == first))
        })
        .collect()
}

type CellAtoms = (Vec<Atom>, UnitCell, Vec<(String, String)>);

fn honeycomb_cell(params: &HoneycombParams, gap: f32) -> Result<CellAtoms> {
    require_element("Sublattice A", &params.element_a)?;
    require_element("Sublattice B", &params.element_b)?;
    if params.lattice_a <= 0.0 {
        bail!("honeycomb lattice constant must be positive");
    }

    let c = gap.max(MIN_C);
    let cell = hexagonal_cell(params.lattice_a, c);
    let buckle = Vector3::new(0.0, 0.0, params.buckling * 0.5);
    let site_a = cell.fractional_to_cartesian(1.0 / 3.0, 1.0 / 3.0, 0.5);
    let site_b = cell.fractional_to_cartesian(2.0 / 3.0, 2.0 / 3.0, 0.5);

    let atoms = vec![
        atom(&params.element_a, site_a + buckle),
        atom(&params.element_b, site_b - buckle),
    ];
    let allowed = vec![(params.element_a.clone(), params.element_b.clone())];
    Ok((atoms, cell, allowed))
}

fn tmd_cell(params: &TmdParams, gap: f32) -> Result<CellAtoms> {
    require_element("Metal", &params.metal)?;
    require_element("Chalcogen", &params.chalcogen)?;
    if params.lattice_a <= 0.0 {
        bail!("TMD lattice constant must be positive");
    }
    if params.chalcogen_separation < 0.0 {
        bail!("chalcogen separation cannot be negative");
    }

    let thickness = params.chalcogen_separation;
    let c = (thickness + gap).max(MIN_C);
    let cell = hexagonal_cell(params.lattice_a, c);
    let half = Vector3::new(0.0, 0.0, thickness * 0.5);

    // Metal triangular net at the cell origin; the two hollow sites are at
    // (1/3,1/3) and (2/3,2/3). 1H eclipses both chalcogens over one hollow
    // (trigonal prismatic); 1T staggers them onto opposite hollows (octahedral).
    let metal_site = cell.fractional_to_cartesian(0.0, 0.0, 0.5);
    let top_site = cell.fractional_to_cartesian(1.0 / 3.0, 1.0 / 3.0, 0.5);
    let bottom_site = match params.polytype {
        TmdPolytype::H => top_site,
        TmdPolytype::T => cell.fractional_to_cartesian(2.0 / 3.0, 2.0 / 3.0, 0.5),
    };

    let atoms = vec![
        atom(&params.metal, metal_site),
        atom(&params.chalcogen, top_site + half),
        atom(&params.chalcogen, bottom_site - half),
    ];
    let allowed = vec![(params.metal.clone(), params.chalcogen.clone())];
    Ok((atoms, cell, allowed))
}

fn carbon_nitride_cell(params: &CarbonNitrideParams, gap: f32) -> Result<CellAtoms> {
    if params.lattice_a <= 0.0 {
        bail!("carbon nitride lattice constant must be positive");
    }

    let c = gap.max(MIN_C);
    let cell = hexagonal_cell(params.lattice_a, c);
    // Node centre on sublattice A; the bridging nitrogen on sublattice B sits
    // along the +30° direction shared by the node's outward-pointing carbons.
    let centre = cell.fractional_to_cartesian(1.0 / 3.0, 1.0 / 3.0, 0.5);
    let bridge = cell.fractional_to_cartesian(2.0 / 3.0, 2.0 / 3.0, 0.5);

    let motif = match params.node {
        CarbonNitrideNode::Triazine => triazine_motif(),
        CarbonNitrideNode::Heptazine => heptazine_motif(),
    };

    let mut atoms: Vec<Atom> = motif
        .into_iter()
        .map(|(element, offset)| atom(element, centre + offset))
        .collect();
    atoms.push(atom("N", bridge));

    let allowed = vec![("C".to_string(), "N".to_string())];
    Ok((atoms, cell, allowed))
}

/// s-triazine ring (C3N3): carbons point outward toward the bridging nitrogens
/// at 30/150/270°, ring nitrogens fill 90/210/330°.
fn triazine_motif() -> Vec<(&'static str, Vector3<f32>)> {
    let mut motif = Vec::with_capacity(6);
    for angle in [30.0, 150.0, 270.0] {
        motif.push(("C", polar(RING_BOND, angle)));
    }
    for angle in [90.0, 210.0, 330.0] {
        motif.push(("N", polar(RING_BOND, angle)));
    }
    motif
}

/// Heptazine / tri-s-triazine (C6N7): a central nitrogen bonded to three shared
/// bridgehead carbons, with three fused triazine lobes pointing at 30/150/270°.
/// Each lobe contributes one apex carbon (carrying the inter-node bridge bond)
/// and two ring nitrogens.
fn heptazine_motif() -> Vec<(&'static str, Vector3<f32>)> {
    let mut motif = vec![("N", Vector3::zeros())]; // central nitrogen
    for angle in [90.0, 210.0, 330.0] {
        motif.push(("C", polar(RING_BOND, angle))); // shared bridgehead carbons
    }
    for lobe in [30.0, 150.0, 270.0] {
        let lobe_centre = polar(RING_BOND, lobe);
        motif.push(("C", lobe_centre + polar(RING_BOND, lobe))); // apex carbon
        motif.push(("N", lobe_centre + polar(RING_BOND, lobe + 60.0)));
        motif.push(("N", lobe_centre + polar(RING_BOND, lobe - 60.0)));
    }
    motif
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::nanosheet::recipe::{
        CarbonNitrideParams, NanosheetSpec, TmdParams, presets,
    };

    fn count(structure: &Structure, element: &str) -> usize {
        structure
            .atoms
            .iter()
            .filter(|a| a.element == element)
            .count()
    }

    fn connected_components(structure: &Structure) -> usize {
        let mut visited = vec![false; structure.atoms.len()];
        let mut adjacency = vec![Vec::new(); structure.atoms.len()];
        for bond in &structure.bonds {
            adjacency[bond.a].push(bond.b);
            adjacency[bond.b].push(bond.a);
        }
        let mut components = 0;
        for start in 0..structure.atoms.len() {
            if visited[start] {
                continue;
            }
            components += 1;
            let mut stack = vec![start];
            visited[start] = true;
            while let Some(current) = stack.pop() {
                for &next in &adjacency[current] {
                    if !visited[next] {
                        visited[next] = true;
                        stack.push(next);
                    }
                }
            }
        }
        components
    }

    fn neighbor_counts(structure: &Structure) -> Vec<usize> {
        let mut counts = vec![0usize; structure.atoms.len()];
        for bond in &structure.bonds {
            counts[bond.a] += 1;
            counts[bond.b] += 1;
        }
        counts
    }

    fn spec(kind: SheetKind, supercell: [u32; 3]) -> NanosheetSpec {
        NanosheetSpec {
            name: "Test".to_string(),
            kind,
            interlayer_spacing: 12.0,
            supercell,
        }
    }

    #[test]
    fn graphene_primitive_cell_is_hexagonal() {
        let structure = build_nanosheet(&spec(
            SheetKind::Honeycomb(HoneycombParams::graphene()),
            [1, 1, 1],
        ))
        .expect("graphene");
        let cell = structure.cell.as_ref().expect("cell");

        assert_eq!(structure.atoms.len(), 2);
        assert_eq!(count(&structure, "C"), 2);
        assert!((cell.a - 2.46).abs() < 1e-3);
        assert!((cell.b - 2.46).abs() < 1e-3);
        assert!((cell.gamma - 60.0).abs() < 1e-3);
    }

    #[test]
    fn graphene_sheet_is_threefold_coordinated_and_connected() {
        let structure = build_nanosheet(&spec(
            SheetKind::Honeycomb(HoneycombParams::graphene()),
            [4, 4, 1],
        ))
        .expect("graphene sheet");

        assert_eq!(structure.atoms.len(), 32);
        assert_eq!(connected_components(&structure), 1);
        assert!(
            neighbor_counts(&structure).iter().all(|&n| n == 3),
            "every carbon in a graphene sheet should have three neighbours"
        );

        let cell = structure.cell.as_ref().expect("cell");
        let max_bond = structure
            .bonds
            .iter()
            .map(|b| {
                periodic_distance(
                    cell,
                    structure.atoms[b.a].position,
                    structure.atoms[b.b].position,
                )
            })
            .fold(0.0_f32, f32::max);
        assert!(
            (max_bond - 1.42).abs() < 0.02,
            "C-C bond length off: {max_bond}"
        );
    }

    #[test]
    fn boron_nitride_only_bonds_b_to_n() {
        let structure = build_nanosheet(&spec(
            SheetKind::Honeycomb(HoneycombParams::boron_nitride()),
            [3, 3, 1],
        ))
        .expect("h-BN");

        assert_eq!(count(&structure, "B"), 9);
        assert_eq!(count(&structure, "N"), 9);
        assert!(structure.bonds.iter().all(|bond| {
            let pair = (
                structure.atoms[bond.a].element.as_str(),
                structure.atoms[bond.b].element.as_str(),
            );
            pair == ("B", "N") || pair == ("N", "B")
        }));
    }

    #[test]
    fn mos2_is_sixfold_metal_coordination_without_metal_metal_bonds() {
        let structure =
            build_nanosheet(&spec(SheetKind::Tmd(TmdParams::mos2()), [3, 3, 1])).expect("MoS2");

        assert_eq!(count(&structure, "Mo"), 9);
        assert_eq!(count(&structure, "S"), 18);
        assert_eq!(connected_components(&structure), 1);

        assert!(
            structure.bonds.iter().all(|bond| {
                let pair = (
                    structure.atoms[bond.a].element.as_str(),
                    structure.atoms[bond.b].element.as_str(),
                );
                pair == ("Mo", "S") || pair == ("S", "Mo")
            }),
            "MoS2 must not contain Mo-Mo or S-S bonds"
        );

        let counts = neighbor_counts(&structure);
        for (index, atom) in structure.atoms.iter().enumerate() {
            if atom.element == "Mo" {
                assert_eq!(counts[index], 6, "each Mo should bind six sulfurs");
            }
        }
    }

    #[test]
    fn mos2_1t_differs_from_1h_geometry() {
        let mut params = TmdParams::mos2();
        params.polytype = TmdPolytype::T;
        let octahedral =
            build_nanosheet(&spec(SheetKind::Tmd(params), [1, 1, 1])).expect("1T MoS2");
        let prismatic =
            build_nanosheet(&spec(SheetKind::Tmd(TmdParams::mos2()), [1, 1, 1])).expect("1H MoS2");

        // The bottom chalcogen sits over a different hollow in 1T than in 1H.
        let bottom_1t = octahedral.atoms[2].position;
        let bottom_1h = prismatic.atoms[2].position;
        assert!((bottom_1t - bottom_1h).xy().norm() > 0.5);
    }

    #[test]
    fn triazine_carbon_nitride_has_c3n4_stoichiometry() {
        let structure = build_nanosheet(&spec(
            SheetKind::CarbonNitride(CarbonNitrideParams::triazine()),
            [1, 1, 1],
        ))
        .expect("triazine g-C3N4");

        // 1 triazine ring (C3N3) + 1 bridging N per cell = C3N4.
        assert_eq!(count(&structure, "C"), 3);
        assert_eq!(count(&structure, "N"), 4);

        let counts = neighbor_counts(&structure);
        for (index, atom) in structure.atoms.iter().enumerate() {
            if atom.element == "C" {
                assert_eq!(counts[index], 3, "triazine carbons are three-coordinate");
            }
        }
    }

    #[test]
    fn heptazine_carbon_nitride_has_c3n4_stoichiometry_and_is_connected() {
        let structure = build_nanosheet(&spec(
            SheetKind::CarbonNitride(CarbonNitrideParams::heptazine()),
            [2, 2, 1],
        ))
        .expect("heptazine g-C3N4");

        // 1 heptazine (C6N7) + 1 bridging N per cell = C6N8 = C3N4.
        assert_eq!(count(&structure, "C"), 6 * 4);
        assert_eq!(count(&structure, "N"), 8 * 4);
        assert_eq!(connected_components(&structure), 1);
        assert!(structure.bonds.iter().all(|bond| {
            let pair = (
                structure.atoms[bond.a].element.as_str(),
                structure.atoms[bond.b].element.as_str(),
            );
            pair == ("C", "N") || pair == ("N", "C")
        }));
    }

    #[test]
    fn every_preset_builds() {
        for (label, kind) in presets() {
            let structure =
                build_nanosheet(&spec(kind, [2, 2, 1])).unwrap_or_else(|e| panic!("{label}: {e}"));
            assert!(!structure.atoms.is_empty(), "{label} produced no atoms");
            assert!(!structure.bonds.is_empty(), "{label} produced no bonds");
        }
    }

    fn periodic_distance(cell: &UnitCell, a: Point3<f32>, b: Point3<f32>) -> f32 {
        let fa = cell.cartesian_to_fractional(a);
        let fb = cell.cartesian_to_fractional(b);
        let mut delta = fb - fa;
        delta.x -= delta.x.round();
        delta.y -= delta.y.round();
        delta.z -= delta.z.round();
        (cell.vectors[0] * delta.x + cell.vectors[1] * delta.y + cell.vectors[2] * delta.z).norm()
    }
}
