//! Turn a 2D [`Sketch`] (or a SMILES string) into a 3D [`Structure`].
//!
//! A flat 2D depiction has to be inflated into a chemically sensible 3D
//! conformation. The strategy is the brief's recommended baseline — lift the
//! drawing into 3D and relax it with the in-repo UFF optimiser — but a single
//! relaxation from a flat sheet gets trapped in the plane: a planar ring is a
//! saddle of the angle terms, so gradient descent from it has no symmetry to
//! break and never puckers (a "cyclohexane" comes out flat instead of as a
//! chair). To fix that we:
//!
//!   1. seed several *non-planar* starting geometries — one chair-like seed from
//!      the bond graph's two-colouring plus a spread of varied puckers — and
//!   2. relax each with UFF, keeping the lowest-energy result.
//!
//! Hydrogens are placed with proper local geometry (tetrahedral / trigonal
//! directions derived from the existing bonds, genuinely out of plane) so an
//! `sp3` centre starts roughly tetrahedral rather than with every substituent
//! squashed into the drawing plane.
//!
//! Hydrogen *counts* are taken from [`crate::domain::sketch`] (rather than
//! [`crate::domain::chemistry::add_missing_hydrogens`]) because the sketcher
//! models formal charges and that helper's valence table is charge-unaware — it
//! would, for example, protonate a carboxylate oxygen back to a neutral hydroxyl.
//!
//! UFF only parameterises H, C, N, O, F, P, S, Cl, Br and I. If the sketch uses
//! any other element the relaxation is skipped and the lifted (seeded) geometry
//! is returned as-is — still a valid structure, just not energy-minimised.

use std::time::{Duration, Instant};

use anyhow::Result;
use nalgebra::{Point3, Vector3};

use crate::domain::{Atom, Bond, BondType, Structure, sketch::Sketch, smiles};
use crate::engines::forcefield::{self, OptimizationOptions};

/// Independent relaxations attempted per build; the lowest-energy one is kept.
const MAX_RESTARTS: usize = 6;
/// Wall-clock budget for the *whole* build. Builds run synchronously on the UI
/// thread, so this caps the worst-case freeze: each restart is given only the
/// budget still remaining, so the total never exceeds it. Small molecules finish
/// in a small fraction of it and use every restart.
const BUILD_BUDGET: Duration = Duration::from_millis(4000);
/// Per-restart ceiling, so a single big molecule can't spend the entire budget
/// on one seed and leave nothing for the others.
const RESTART_CAP: Duration = Duration::from_millis(1500);
/// Out-of-plane amplitude (Å) used to seed ring puckering before relaxation.
const PUCKER_SEED: f32 = 0.5;

/// Convert a sketch to a relaxed 3D structure with the given title.
pub fn sketch_to_structure(sketch: &Sketch, title: impl Into<String>) -> Structure {
    let title = title.into();
    let parity = two_colouring(sketch);

    let started = Instant::now();
    let mut best: Option<(f32, Structure)> = None;

    for restart in 0..MAX_RESTARTS {
        let remaining = BUILD_BUDGET.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        let mut structure = embed(sketch, &title, restart, &parity);
        match forcefield::optimize_geometry(
            &mut structure,
            build_options(remaining.min(RESTART_CAP)),
        ) {
            Ok(report) => {
                let energy = report.final_energy;
                if best
                    .as_ref()
                    .is_none_or(|(best_energy, _)| energy < *best_energy)
                {
                    best = Some((energy, structure));
                }
            }
            // An unsupported element can't be relaxed by UFF; the lifted geometry
            // is still a valid structure and further restarts wouldn't change that.
            Err(_) => return structure,
        }
    }

    best.map(|(_, structure)| structure)
        .unwrap_or_else(|| embed(sketch, &title, 0, &parity))
}

/// Parse a SMILES string and convert it to a relaxed 3D structure. The title
/// defaults to the SMILES text when not given.
pub fn smiles_to_structure(input: &str, title: Option<&str>) -> Result<Structure> {
    let sketch = smiles::parse(input)?;
    let title = title
        .map(str::to_string)
        .unwrap_or_else(|| input.to_string());
    Ok(sketch_to_structure(&sketch, title))
}

/// Optimiser settings for a sketch build: a tighter, longer relaxation than the
/// default so each seed reaches a clean geometry. `max_duration` is the time this
/// restart is allowed (the budget still remaining, capped at [`RESTART_CAP`]).
fn build_options(max_duration: Duration) -> OptimizationOptions {
    OptimizationOptions {
        max_steps: 600,
        gradient_tolerance: 5.0e-4,
        initial_step_size: 0.01,
        max_atom_step: 0.1,
        max_duration,
        ..OptimizationOptions::default()
    }
}

/// Build one starting structure: heavy atoms lifted to 3D with a restart-specific
/// out-of-plane seed, then implicit hydrogens placed around them.
fn embed(sketch: &Sketch, title: &str, restart: usize, parity: &[bool]) -> Structure {
    let mut atoms = Vec::with_capacity(sketch.atoms.len());
    for (index, atom) in sketch.atoms.iter().enumerate() {
        let z = seed_z(restart, index, parity.get(index).copied().unwrap_or(false));
        atoms.push(Atom {
            element: atom.element.clone(),
            position: Point3::new(atom.pos.x, atom.pos.y, z),
            charge: atom.charge as f32,
        });
    }

    let mut bonds: Vec<Bond> = sketch
        .bonds
        .iter()
        .map(|bond| Bond::with_type(bond.a, bond.b, bond.order))
        .collect();

    fill_hydrogens(sketch, &mut atoms, &mut bonds);
    Structure::with_bonds(title.to_string(), atoms, bonds)
}

/// The out-of-plane offset to seed an atom with for a given restart.
fn seed_z(restart: usize, index: usize, color: bool) -> f32 {
    if restart == 0 {
        // Put the two colours of the bond graph on opposite faces. For an even
        // ring that two-colouring alternates around the ring — exactly a chair
        // seed — and for anything else it is still a sensible pucker.
        return if color { PUCKER_SEED } else { -PUCKER_SEED };
    }
    // Remaining restarts vary phase and frequency to explore other puckers
    // (boat, twist-boat, …); the lowest-energy relaxation wins.
    let frequency = 0.6 + 0.35 * restart as f32;
    let phase = restart as f32 * 2.399_963;
    PUCKER_SEED * (index as f32 * frequency + phase).sin()
}

/// Two-colour the bond graph (BFS parity per connected component). Neighbouring
/// atoms get opposite colours wherever the graph is bipartite.
fn two_colouring(sketch: &Sketch) -> Vec<bool> {
    let n = sketch.atoms.len();
    let mut adjacency = vec![Vec::new(); n];
    for bond in &sketch.bonds {
        if bond.a < n && bond.b < n {
            adjacency[bond.a].push(bond.b);
            adjacency[bond.b].push(bond.a);
        }
    }

    let mut color = vec![false; n];
    let mut visited = vec![false; n];
    let mut queue = std::collections::VecDeque::new();
    for start in 0..n {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        queue.push_back(start);
        while let Some(current) = queue.pop_front() {
            for &next in &adjacency[current] {
                if !visited[next] {
                    visited[next] = true;
                    color[next] = !color[current];
                    queue.push_back(next);
                }
            }
        }
    }
    color
}

/// Add charge-aware implicit hydrogens to every heavy atom, mirroring the count
/// the canvas shows. Materialised explicit hydrogens already present in the
/// sketch are left untouched (and counted by the valence model).
fn fill_hydrogens(sketch: &Sketch, atoms: &mut Vec<Atom>, bonds: &mut Vec<Bond>) {
    let heavy_count = sketch.atoms.len();
    for heavy in 0..heavy_count {
        if sketch.atoms[heavy].element == "H" {
            continue;
        }
        let count = sketch.implicit_hydrogens(heavy);
        if count == 0 {
            continue;
        }
        let existing = neighbor_directions(atoms, bonds, heavy);
        let directions = spread_directions(&existing, count as usize);
        let bond_length =
            forcefield::equilibrium_bond_length(&atoms[heavy].element, "H", BondType::Single)
                .unwrap_or(1.0);
        let origin = atoms[heavy].position;
        for direction in directions {
            let h_index = atoms.len();
            atoms.push(Atom {
                element: "H".to_string(),
                position: origin + direction * bond_length,
                charge: 0.0,
            });
            bonds.push(Bond::with_type(heavy, h_index, BondType::Single));
        }
    }
}

/// Unit directions from `atom` to each of its current bonded neighbours.
fn neighbor_directions(atoms: &[Atom], bonds: &[Bond], atom: usize) -> Vec<Vector3<f32>> {
    let center = atoms[atom].position;
    bonds
        .iter()
        .filter_map(|bond| {
            let other = if bond.a == atom {
                bond.b
            } else if bond.b == atom {
                bond.a
            } else {
                return None;
            };
            (atoms[other].position - center).try_normalize(1.0e-4)
        })
        .collect()
}

/// Pick `count` unit directions for new hydrogens that complete a sensible local
/// geometry around an atom that already has the bonds in `existing`. The
/// directions are genuinely 3D (out of the existing-bond plane where they should
/// be) so an `sp3` centre starts tetrahedral; UFF then finalises everything.
fn spread_directions(existing: &[Vector3<f32>], count: usize) -> Vec<Vector3<f32>> {
    if count == 0 {
        return Vec::new();
    }
    match existing {
        // Free atom (e.g. lone carbon → methane): tetrahedral fan.
        [] => tetrahedral_subset(count),
        // One bond (e.g. methyl, hydroxyl): cone at the tetrahedral angle
        // (cos = -1/3) about the open side of the existing bond.
        [only] => cone(-only, 1.0 / 3.0, count, 0.0),
        // Two bonds (e.g. ring/chain CH2): the remaining tetrahedral positions,
        // symmetric across the plane of the two existing bonds.
        [first, second] => complete_two(*first, *second, count),
        // Three or more bonds: the remaining direction opposes their resultant.
        many => {
            let sum: Vector3<f32> = many.iter().sum();
            let open = (-sum)
                .try_normalize(1.0e-4)
                .unwrap_or_else(|| perpendicular(many[0]));
            if count == 1 {
                vec![open]
            } else {
                cone(open, 0.0, count, std::f32::consts::FRAC_PI_2)
            }
        }
    }
}

/// `count` unit directions on a cone about `axis`, each making an angle with the
/// axis whose cosine is `axial`, spread evenly in azimuth from `phase`.
fn cone(axis: Vector3<f32>, axial: f32, count: usize, phase: f32) -> Vec<Vector3<f32>> {
    let axis = axis
        .try_normalize(1.0e-4)
        .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
    let u = perpendicular(axis);
    let w = axis.cross(&u).normalize();
    let radial = (1.0 - axial * axial).max(0.0).sqrt();
    (0..count)
        .map(|i| {
            let theta = phase + i as f32 * std::f32::consts::TAU / count as f32;
            (axis * axial + (u * theta.cos() + w * theta.sin()) * radial).normalize()
        })
        .collect()
}

/// The (up to two) tetrahedral directions that complete a centre already bonded
/// along `first` and `second`. The new bonds straddle the plane of the existing
/// pair, putting a CH2's two hydrogens above and below it rather than in-plane.
fn complete_two(first: Vector3<f32>, second: Vector3<f32>, count: usize) -> Vec<Vector3<f32>> {
    let sum = first + second;
    let half = -sum * 0.5;
    let normal = first
        .cross(&second)
        .try_normalize(1.0e-4)
        .unwrap_or_else(|| perpendicular(first));
    let height = (1.0 - half.norm_squared()).max(0.0).sqrt();
    let up = (half + normal * height)
        .try_normalize(1.0e-4)
        .unwrap_or(normal);
    let down = (half - normal * height)
        .try_normalize(1.0e-4)
        .unwrap_or(-normal);
    match count {
        1 => vec![up],
        2 => vec![up, down],
        // More than two (hypervalent) — keep the two tetrahedral slots and fan
        // the rest around the open direction.
        _ => {
            let mut directions = vec![up, down];
            let open = (-sum).try_normalize(1.0e-4).unwrap_or(normal);
            directions.extend(cone(open, 0.0, count - 2, 0.0));
            directions
        }
    }
}

/// `count` directions from a fixed tetrahedron, for an atom with no bonds yet.
fn tetrahedral_subset(count: usize) -> Vec<Vector3<f32>> {
    let vertices = [
        Vector3::new(1.0, 1.0, 1.0),
        Vector3::new(1.0, -1.0, -1.0),
        Vector3::new(-1.0, 1.0, -1.0),
        Vector3::new(-1.0, -1.0, 1.0),
    ];
    vertices
        .iter()
        .cycle()
        .take(count)
        .map(|direction| direction.normalize())
        .collect()
}

/// Any unit vector perpendicular to `v`.
fn perpendicular(v: Vector3<f32>) -> Vector3<f32> {
    let reference = if v.z.abs() < 0.9 {
        Vector3::new(0.0, 0.0, 1.0)
    } else {
        Vector3::new(1.0, 0.0, 0.0)
    };
    v.cross(&reference)
        .try_normalize(1.0e-4)
        .unwrap_or_else(|| Vector3::new(1.0, 0.0, 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::sketch::RingTemplate;
    use nalgebra::{Matrix3, Point2};

    fn ring_sketch(template: RingTemplate, element: &str) -> Sketch {
        let mut sketch = Sketch::new();
        let (positions, bonds) = template.build();
        let indices: Vec<usize> = positions
            .iter()
            .map(|p| sketch.add_atom(element, *p))
            .collect();
        for (a, b, order) in bonds {
            sketch.add_bond(indices[a], indices[b], order);
        }
        sketch
    }

    fn benzene_sketch() -> Sketch {
        ring_sketch(RingTemplate::Benzene, "C")
    }

    fn min_pairwise_distance(structure: &Structure) -> f32 {
        let mut min = f32::MAX;
        for i in 0..structure.atoms.len() {
            for j in (i + 1)..structure.atoms.len() {
                let d = (structure.atoms[i].position - structure.atoms[j].position).norm();
                min = min.min(d);
            }
        }
        min
    }

    /// Unit vectors from `atom` to each bonded neighbour in the built structure.
    fn neighbor_units(structure: &Structure, atom: usize) -> Vec<Vector3<f32>> {
        let center = structure.atoms[atom].position;
        structure
            .bonds
            .iter()
            .filter_map(|bond| {
                let other = if bond.a == atom {
                    bond.b
                } else if bond.b == atom {
                    bond.a
                } else {
                    return None;
                };
                (structure.atoms[other].position - center).try_normalize(1.0e-4)
            })
            .collect()
    }

    fn min_pairwise_angle_degrees(directions: &[Vector3<f32>]) -> f32 {
        let mut min = f32::MAX;
        for i in 0..directions.len() {
            for j in (i + 1)..directions.len() {
                let angle = directions[i].dot(&directions[j]).clamp(-1.0, 1.0).acos();
                min = min.min(angle.to_degrees());
            }
        }
        min
    }

    /// RMS thickness of a set of atoms about their best-fit plane (0 if coplanar).
    fn plane_thickness(structure: &Structure, indices: &[usize]) -> f32 {
        let count = indices.len() as f32;
        let centroid = indices.iter().fold(Vector3::zeros(), |acc, &i| {
            acc + structure.atoms[i].position.coords
        }) / count;
        let mut covariance = Matrix3::zeros();
        for &i in indices {
            let delta = structure.atoms[i].position.coords - centroid;
            covariance += delta * delta.transpose();
        }
        covariance /= count;
        let smallest = covariance
            .symmetric_eigenvalues()
            .iter()
            .copied()
            .fold(f32::MAX, f32::min);
        smallest.max(0.0).sqrt()
    }

    #[test]
    fn benzene_builds_six_carbons_and_six_hydrogens() {
        let structure = sketch_to_structure(&benzene_sketch(), "benzene");
        let carbons = structure.atoms.iter().filter(|a| a.element == "C").count();
        let hydrogens = structure.atoms.iter().filter(|a| a.element == "H").count();
        assert_eq!(carbons, 6);
        assert_eq!(hydrogens, 6);
        // 6 ring + 6 C–H bonds.
        assert_eq!(structure.bonds.len(), 12);
    }

    #[test]
    fn benzene_geometry_is_non_degenerate() {
        let structure = sketch_to_structure(&benzene_sketch(), "benzene");
        // No two atoms collapsed onto each other.
        assert!(min_pairwise_distance(&structure) > 0.6);
        // Carbons are spread in 3D, not a single point.
        assert!(structure.radius() > 1.0);
    }

    #[test]
    fn cyclohexane_relaxes_to_a_puckered_ring_of_tetrahedral_carbons() {
        let structure = sketch_to_structure(&ring_sketch(RingTemplate::Cyclohexane, "C"), "C6H12");
        let carbons: Vec<usize> = (0..structure.atoms.len())
            .filter(|&i| structure.atoms[i].element == "C")
            .collect();
        assert_eq!(carbons.len(), 6);
        assert_eq!(
            structure.atoms.iter().filter(|a| a.element == "H").count(),
            12
        );

        // A chair (or any real conformer) is not flat: the six carbons must not
        // be coplanar the way the 2D drawing is. (Empirically this seeds a clean
        // chair: ring torsions relax to ±57°, thickness ≈ 0.24 Å.)
        let thickness = plane_thickness(&structure, &carbons);
        assert!(thickness > 0.1, "ring is too flat: thickness {thickness}");

        // Every ring carbon is sp3: four bonds, none squashed toward a planar
        // ~120° arrangement, and the four bond vectors nearly cancel.
        for &carbon in &carbons {
            let directions = neighbor_units(&structure, carbon);
            assert_eq!(
                directions.len(),
                4,
                "carbon {carbon} should have four bonds"
            );
            let min_angle = min_pairwise_angle_degrees(&directions);
            assert!(
                min_angle > 95.0,
                "carbon {carbon} has a flattened bond angle of {min_angle}°"
            );
            let resultant: Vector3<f32> = directions.iter().sum();
            assert!(
                resultant.norm() < 0.7,
                "carbon {carbon} is not tetrahedral (|Σ| = {})",
                resultant.norm()
            );
        }
    }

    #[test]
    fn methane_is_tetrahedral() {
        let mut sketch = Sketch::new();
        sketch.add_atom("C", Point2::origin());
        let structure = sketch_to_structure(&sketch, "methane");
        assert_eq!(structure.atoms.len(), 5); // C + 4 H
        assert_eq!(structure.bonds.len(), 4);
        // The carbon is index 0 (heavy atoms are placed before hydrogens).
        let directions = neighbor_units(&structure, 0);
        assert_eq!(directions.len(), 4);
        assert!(
            min_pairwise_angle_degrees(&directions) > 100.0,
            "methane H–C–H angles collapsed"
        );
        let resultant: Vector3<f32> = directions.iter().sum();
        assert!(resultant.norm() < 0.4, "methane is not tetrahedral");
    }

    /// Unit vectors from `atom` to its bonded neighbours of a given element.
    fn neighbor_units_to(structure: &Structure, atom: usize, element: &str) -> Vec<Vector3<f32>> {
        let center = structure.atoms[atom].position;
        structure
            .bonds
            .iter()
            .filter_map(|bond| {
                let other = if bond.a == atom {
                    bond.b
                } else if bond.b == atom {
                    bond.a
                } else {
                    return None;
                };
                if structure.atoms[other].element != element {
                    return None;
                }
                (structure.atoms[other].position - center).try_normalize(1.0e-4)
            })
            .collect()
    }

    #[test]
    fn cumulene_centres_relax_to_linear() {
        // CO2 (O=C=O): the carbon bears two double bonds, so it is sp/linear —
        // the angle at carbon must be ~180°, not a bent sp2 ~120°.
        let co2 = smiles_to_structure("O=C=O", Some("co2")).unwrap();
        let carbon = (0..co2.atoms.len())
            .find(|&i| co2.atoms[i].element == "C")
            .unwrap();
        let oxygens = neighbor_units_to(&co2, carbon, "O");
        assert_eq!(oxygens.len(), 2);
        let angle = oxygens[0]
            .dot(&oxygens[1])
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees();
        assert!(angle > 170.0, "CO2 came out bent at {angle}°");

        // Allene (H2C=C=CH2): the central carbon is likewise linear.
        let allene = smiles_to_structure("C=C=C", Some("allene")).unwrap();
        let central = (0..allene.atoms.len())
            .find(|&i| {
                allene.atoms[i].element == "C" && neighbor_units_to(&allene, i, "C").len() == 2
            })
            .unwrap();
        let carbons = neighbor_units_to(&allene, central, "C");
        let angle = carbons[0]
            .dot(&carbons[1])
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees();
        assert!(angle > 170.0, "allene centre came out bent at {angle}°");
    }

    #[test]
    fn bracket_radical_keeps_its_pinned_hydrogen_count() {
        // A SMILES carbene [CH2] must build C + exactly 2 H, not fill to methane.
        let structure = smiles_to_structure("[CH2]", None).unwrap();
        assert_eq!(
            structure.atoms.iter().filter(|a| a.element == "C").count(),
            1
        );
        assert_eq!(
            structure.atoms.iter().filter(|a| a.element == "H").count(),
            2
        );
        // A bare radical carbon [C] gets none.
        let bare = smiles_to_structure("[C]", None).unwrap();
        assert_eq!(bare.atoms.len(), 1);
    }

    #[test]
    fn carboxylate_oxygen_keeps_its_charge_and_gets_no_hydrogen() {
        // CC(=O)[O-] — the anionic oxygen must NOT be protonated.
        let structure = smiles_to_structure("CC(=O)[O-]", Some("acetate")).unwrap();
        let charged = structure
            .atoms
            .iter()
            .filter(|a| a.element == "O" && a.charge < 0.0)
            .count();
        assert_eq!(charged, 1);
        // Acetate is C2H3O2⁻: 2 C, 2 O, 3 H.
        let hydrogens = structure.atoms.iter().filter(|a| a.element == "H").count();
        assert_eq!(hydrogens, 3);
    }

    #[test]
    fn smiles_to_structure_handles_ethanol() {
        let structure = smiles_to_structure("CCO", None).unwrap();
        assert_eq!(
            structure.atoms.iter().filter(|a| a.element == "C").count(),
            2
        );
        assert_eq!(
            structure.atoms.iter().filter(|a| a.element == "O").count(),
            1
        );
        // C2H6O: 6 hydrogens.
        assert_eq!(
            structure.atoms.iter().filter(|a| a.element == "H").count(),
            6
        );
        // The two carbons must be sp3-tetrahedral, not flattened by the lift.
        let carbons: Vec<usize> = (0..structure.atoms.len())
            .filter(|&i| structure.atoms[i].element == "C")
            .collect();
        for &carbon in &carbons {
            let directions = neighbor_units(&structure, carbon);
            assert!(
                min_pairwise_angle_degrees(&directions) > 95.0,
                "ethanol carbon {carbon} is flattened"
            );
        }
    }

    #[test]
    fn unsupported_element_falls_back_without_panicking() {
        // Boron is not UFF-parameterised; the build must still return a structure.
        let mut sketch = Sketch::new();
        let b = sketch.add_atom("B", Point2::origin());
        let c = sketch.add_atom("C", Point2::new(1.5, 0.0));
        sketch.add_bond(b, c, BondType::Single);
        let structure = sketch_to_structure(&sketch, "borane-ish");
        assert!(structure.atoms.iter().any(|a| a.element == "B"));
        assert!(structure.atoms.iter().any(|a| a.element == "C"));
    }
}
