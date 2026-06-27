use nalgebra::Vector3;

use super::{build_framework, connected_components};
use crate::domain::{BondType, Structure};
use crate::workflows::reticular::{
    ComponentSource, CoreSlot, FunctionalizationRule, NetworkId, ReticularBuildSpec, StackingMode,
    component_template,
};

#[test]
fn builds_minimal_honeycomb_framework() {
    let structure = build_framework(&ReticularBuildSpec::default()).expect("framework");

    assert!(!structure.atoms.is_empty());
    assert!(structure.cell.is_some());
    assert_eq!(structure.title, "Structure");
    assert_eq!(
        connected_components(&structure),
        ReticularBuildSpec::default().supercell[2] as usize
    );
    assert!(
        structure.bonds.len() >= structure.atoms.len(),
        "expected a connected covalent network, got {} bonds for {} atoms",
        structure.bonds.len(),
        structure.atoms.len()
    );
}

#[test]
fn default_honeycomb_geometry_is_stable() {
    let structure = build_framework(&ReticularBuildSpec::default()).expect("framework");
    let cell = structure.cell.as_ref().expect("cell");

    assert_eq!(structure.atoms.len(), 144);
    assert_eq!(structure.bonds.len(), 168);
    assert_eq!(connected_components(&structure), 2);
    assert!((cell.a - 15.637).abs() < 0.001);
    assert!((cell.b - 15.637).abs() < 0.001);
    assert!((cell.c - 7.2).abs() < 0.0001);
    assert!((cell.alpha - 90.0).abs() < 0.0001);
    assert!((cell.beta - 90.0).abs() < 0.0001);
    assert!((cell.gamma - 60.0).abs() < 0.0001);
}

#[test]
fn preserves_aromatic_bonds_in_built_framework() {
    let structure = build_framework(&ReticularBuildSpec::default()).expect("framework");

    assert!(
        structure
            .bonds
            .iter()
            .any(|bond| bond.bond_type == BondType::Aromatic),
        "expected aromatic bonds from aromatic building blocks to survive framework build"
    );
}

#[test]
fn c3_orientational_stacking_uses_local_component_rotation() {
    let single_layer = build_framework(&ReticularBuildSpec {
        supercell: [1, 1, 1],
        ..ReticularBuildSpec::default()
    })
    .expect("single layer");
    let structure = build_framework(&ReticularBuildSpec {
        stacking: StackingMode::C3Orientational,
        modulate_primary_orientation: true,
        modulate_secondary_orientation: true,
        supercell: [1, 1, 2],
        ..ReticularBuildSpec::default()
    })
    .expect("C3 orientational stack");
    let layer_atom_count = single_layer.atoms.len();

    assert_eq!(structure.atoms.len(), layer_atom_count * 3);
    assert_eq!(connected_components(&structure), 3);
    assert_fractional_positions_are_wrapped(&structure);
}

#[test]
fn c3_orientational_ab_supercells_do_not_rigidly_rotate_layers() {
    let structure = build_framework(&ReticularBuildSpec {
        stacking: StackingMode::C3Orientational,
        modulate_primary_orientation: true,
        modulate_secondary_orientation: true,
        supercell: [3, 3, 3],
        ..ReticularBuildSpec::default()
    })
    .expect("C3 orientational stack");

    assert_eq!(connected_components(&structure), 3);
    assert_fractional_positions_are_wrapped(&structure);
}

#[test]
fn c3_orientational_stacking_can_modulate_only_secondary_core() {
    let single_layer = build_framework(&ReticularBuildSpec {
        supercell: [1, 1, 1],
        ..ReticularBuildSpec::default()
    })
    .expect("single layer");
    let structure = build_framework(&ReticularBuildSpec {
        stacking: StackingMode::C3Orientational,
        modulate_primary_orientation: false,
        modulate_secondary_orientation: true,
        supercell: [1, 1, 2],
        ..ReticularBuildSpec::default()
    })
    .expect("secondary-only C3 orientational stack");

    assert_eq!(structure.atoms.len(), single_layer.atoms.len() * 3);
    assert_eq!(connected_components(&structure), 3);
    assert_fractional_positions_are_wrapped(&structure);
}

#[test]
fn c3_orientational_stacking_without_modulated_cores_uses_aa_period() {
    let single_layer = build_framework(&ReticularBuildSpec {
        supercell: [1, 1, 1],
        ..ReticularBuildSpec::default()
    })
    .expect("single layer");
    let structure = build_framework(&ReticularBuildSpec {
        stacking: StackingMode::C3Orientational,
        modulate_primary_orientation: false,
        modulate_secondary_orientation: false,
        supercell: [1, 1, 2],
        ..ReticularBuildSpec::default()
    })
    .expect("unmodulated C3 orientational stack");

    assert_eq!(structure.atoms.len(), single_layer.atoms.len() * 2);
    assert_eq!(connected_components(&structure), 2);
    assert_fractional_positions_are_wrapped(&structure);
}

#[test]
fn supported_framework_combinations_form_connected_layers() {
    let cases = [
        (
            NetworkId::HoneycombVertexVertex,
            ComponentSource::BuiltinCore(0),
            ComponentSource::BuiltinCore(0),
            Vec::new(),
        ),
        (
            NetworkId::HoneycombVertexVertex,
            ComponentSource::BuiltinCore(0),
            ComponentSource::BuiltinCore(1),
            Vec::new(),
        ),
        (
            NetworkId::HoneycombVertexVertex,
            ComponentSource::BuiltinCore(1),
            ComponentSource::BuiltinCore(0),
            Vec::new(),
        ),
        (
            NetworkId::HoneycombVertexVertex,
            ComponentSource::BuiltinCore(1),
            ComponentSource::BuiltinCore(1),
            Vec::new(),
        ),
        (
            NetworkId::HoneycombVertexVertex,
            ComponentSource::BuiltinCore(0),
            ComponentSource::BuiltinCore(0),
            vec![ComponentSource::BuiltinLinker(0)],
        ),
    ];

    for (network, primary, secondary, linkers) in cases {
        let spec = ReticularBuildSpec {
            network,
            primary,
            secondary,
            linkers,
            supercell: [1, 1, 1],
            ..ReticularBuildSpec::default()
        };
        let structure = build_framework(&spec).expect("framework");

        assert_eq!(
            connected_components(&structure),
            1,
            "fragmented combination: {:?} {:?} {:?}",
            network,
            primary,
            secondary
        );
        assert!(
            structure.bonds.len() >= structure.atoms.len(),
            "too few bonds for {:?} {:?} {:?}: {} bonds, {} atoms",
            network,
            primary,
            secondary,
            structure.bonds.len(),
            structure.atoms.len()
        );
        let max_bond_length = maximum_periodic_bond_length(&structure);
        assert!(
            max_bond_length < 1.7,
            "unreasonable bond length for {:?} {:?} {:?}: {:.3} A",
            network,
            primary,
            secondary,
            max_bond_length
        );
        if network == NetworkId::HoneycombVertexVertex {
            assert!(
                in_plane_extent(&structure) > 5.0,
                "collapsed vertex-vertex framework for {:?} {:?}",
                primary,
                secondary
            );
        }
    }
}

#[test]
fn custom_psf_components_can_build_frameworks() {
    let spec = ReticularBuildSpec {
        primary: ComponentSource::Custom(0),
        secondary: ComponentSource::BuiltinCore(1),
        linkers: vec![ComponentSource::Custom(1)],
        custom_components: vec![
            include_str!("../building_block/core/t3/benzene.slf").to_string(),
            include_str!("../building_block/linker/trans_ethene.slf").to_string(),
        ],
        supercell: [1, 1, 1],
        ..ReticularBuildSpec::default()
    };
    let structure = build_framework(&spec).expect("framework from custom psf");

    assert_eq!(connected_components(&structure), 1);
    assert!(structure.atoms.len() > 10);
}

#[test]
fn trans_ethene_linker_keeps_sp2_attachment_angles() {
    let linker = component_template(ComponentSource::BuiltinLinker(0), &[]);
    let first_site = &linker.coordination_sites[0];
    let second_site = &linker.coordination_sites[1];
    let double_bond_from_first = linker.atoms[second_site.binding_atom].position
        - linker.atoms[first_site.binding_atom].position;
    let double_bond_from_second = linker.atoms[first_site.binding_atom].position
        - linker.atoms[second_site.binding_atom].position;
    let first_substitution_direction =
        first_site.coordination_position - linker.atoms[first_site.binding_atom].position.coords;
    let second_substitution_direction =
        second_site.coordination_position - linker.atoms[second_site.binding_atom].position.coords;

    assert!(
        (angle(first_substitution_direction, double_bond_from_first) - 120.0).abs() < 5.0,
        "linker entry angle should be sp2-like"
    );
    assert!(
        (angle(second_substitution_direction, double_bond_from_second) - 120.0).abs() < 5.0,
        "linker exit angle should be sp2-like"
    );
}

#[test]
fn linked_framework_does_not_create_linear_sp2_connections() {
    let spec = ReticularBuildSpec {
        linkers: vec![ComponentSource::BuiltinLinker(0)],
        supercell: [1, 1, 1],
        ..ReticularBuildSpec::default()
    };
    let structure = build_framework(&spec).expect("framework");
    let angles = all_periodic_bond_angles(&structure);
    let max_angle = angles.iter().copied().fold(0.0_f32, f32::max);

    assert!(
        max_angle < 165.0,
        "framework contains a nearly linear local bond angle: {max_angle:.1} deg"
    );
    assert!(
        angles.iter().any(|angle| (angle - 120.0).abs() < 8.0),
        "framework should contain sp2-like angles"
    );
}

#[test]
fn chained_linkers_build_connected_framework() {
    let spec = ReticularBuildSpec {
        linkers: vec![
            ComponentSource::BuiltinLinker(0),
            ComponentSource::BuiltinLinker(1),
            ComponentSource::BuiltinLinker(0),
        ],
        supercell: [1, 1, 1],
        ..ReticularBuildSpec::default()
    };
    let structure = build_framework(&spec).expect("framework with chained linkers");

    assert_eq!(connected_components(&structure), 1);
    assert!(
        structure.atoms.len()
            > build_framework(&ReticularBuildSpec {
                linkers: vec![ComponentSource::BuiltinLinker(0)],
                supercell: [1, 1, 1],
                ..ReticularBuildSpec::default()
            })
            .expect("single linker framework")
            .atoms
            .len()
    );
    assert!(
        maximum_periodic_bond_length(&structure) < 1.7,
        "chained linker framework created an unreasonable bond"
    );
}

#[test]
fn pyridinium_combinations_keep_explicit_c_c_c_angles_sp2_like() {
    for (primary, secondary) in [
        (
            ComponentSource::BuiltinCore(1),
            ComponentSource::BuiltinCore(2),
        ),
        (
            ComponentSource::BuiltinCore(2),
            ComponentSource::BuiltinCore(1),
        ),
        (
            ComponentSource::BuiltinCore(0),
            ComponentSource::BuiltinCore(2),
        ),
        (
            ComponentSource::BuiltinCore(2),
            ComponentSource::BuiltinCore(0),
        ),
    ] {
        let structure = build_framework(&ReticularBuildSpec {
            primary,
            secondary,
            supercell: [1, 1, 1],
            ..ReticularBuildSpec::default()
        })
        .expect("pyridinium framework");
        let angles = explicit_c_c_c_angles(&structure);

        assert!(
            !angles.is_empty(),
            "expected C-C-C angles for {:?} {:?}",
            primary,
            secondary
        );
        assert!(
            angles.iter().all(|angle| (angle - 120.0).abs() < 8.0),
            "bad explicit C-C-C angles for {:?} {:?}: {:?}",
            primary,
            secondary,
            angles
        );
    }
}

#[test]
fn functionalization_can_replace_multiple_hydrogen_sites() {
    let core = component_template(ComponentSource::BuiltinCore(0), &[]);
    let hydrogen_targets = core
        .atoms
        .iter()
        .enumerate()
        .filter_map(|(index, atom)| (atom.element == "H").then_some(index))
        .take(2)
        .collect::<Vec<_>>();
    let spec = ReticularBuildSpec {
        functionalization_enabled: true,
        functionalizations: hydrogen_targets
            .iter()
            .map(|atom_index| FunctionalizationRule {
                slot: CoreSlot::Primary,
                atom_index: *atom_index,
                group: Some(ComponentSource::BuiltinFunctionalGroup(0)),
            })
            .collect(),
        supercell: [1, 1, 1],
        ..ReticularBuildSpec::default()
    };

    let structure = build_framework(&spec).expect("framework");
    let fluorine_count = structure
        .atoms
        .iter()
        .filter(|atom| atom.element == "F")
        .count();

    assert!(fluorine_count >= 2);
}

fn in_plane_extent(structure: &Structure) -> f32 {
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;

    for atom in &structure.atoms {
        min_x = min_x.min(atom.position.x);
        max_x = max_x.max(atom.position.x);
        min_y = min_y.min(atom.position.y);
        max_y = max_y.max(atom.position.y);
    }

    (max_x - min_x).max(max_y - min_y)
}

fn maximum_periodic_bond_length(structure: &Structure) -> f32 {
    let Some(cell) = structure.cell.as_ref() else {
        return structure
            .bonds
            .iter()
            .map(|bond| {
                nalgebra::distance(
                    &structure.atoms[bond.a].position,
                    &structure.atoms[bond.b].position,
                )
            })
            .fold(0.0_f32, f32::max);
    };

    structure
        .bonds
        .iter()
        .map(|bond| {
            periodic_delta(
                cell,
                structure.atoms[bond.a].position,
                structure.atoms[bond.b].position,
            )
            .norm()
        })
        .fold(0.0_f32, f32::max)
}

fn periodic_delta(
    cell: &crate::domain::UnitCell,
    first: nalgebra::Point3<f32>,
    second: nalgebra::Point3<f32>,
) -> Vector3<f32> {
    let first_frac = cell.cartesian_to_fractional(first);
    let second_frac = cell.cartesian_to_fractional(second);
    let mut delta = second_frac - first_frac;

    delta.x -= delta.x.round();
    delta.y -= delta.y.round();
    delta.z -= delta.z.round();

    cell.vectors[0] * delta.x + cell.vectors[1] * delta.y + cell.vectors[2] * delta.z
}

fn explicit_c_c_c_angles(structure: &Structure) -> Vec<f32> {
    let mut neighbors = vec![Vec::new(); structure.atoms.len()];
    for bond in &structure.bonds {
        neighbors[bond.a].push(bond.b);
        neighbors[bond.b].push(bond.a);
    }

    let mut angles = Vec::new();
    for (center, bonded) in neighbors.iter().enumerate() {
        if structure.atoms[center].element != "C" {
            continue;
        }

        let carbon_neighbors = bonded
            .iter()
            .copied()
            .filter(|index| structure.atoms[*index].element == "C")
            .collect::<Vec<_>>();

        for i in 0..carbon_neighbors.len() {
            for j in (i + 1)..carbon_neighbors.len() {
                angles.push(angle(
                    periodic_delta(
                        structure.cell.as_ref().expect("cell"),
                        structure.atoms[center].position,
                        structure.atoms[carbon_neighbors[i]].position,
                    ),
                    periodic_delta(
                        structure.cell.as_ref().expect("cell"),
                        structure.atoms[center].position,
                        structure.atoms[carbon_neighbors[j]].position,
                    ),
                ));
            }
        }
    }

    angles.sort_by(|a, b| a.total_cmp(b));
    angles
}

fn all_periodic_bond_angles(structure: &Structure) -> Vec<f32> {
    let mut neighbors = vec![Vec::new(); structure.atoms.len()];
    for bond in &structure.bonds {
        neighbors[bond.a].push(bond.b);
        neighbors[bond.b].push(bond.a);
    }

    let mut angles = Vec::new();
    for (center, bonded) in neighbors.iter().enumerate() {
        if bonded.len() < 2 {
            continue;
        }

        for i in 0..bonded.len() {
            for j in (i + 1)..bonded.len() {
                angles.push(angle(
                    periodic_delta(
                        structure.cell.as_ref().expect("cell"),
                        structure.atoms[center].position,
                        structure.atoms[bonded[i]].position,
                    ),
                    periodic_delta(
                        structure.cell.as_ref().expect("cell"),
                        structure.atoms[center].position,
                        structure.atoms[bonded[j]].position,
                    ),
                ));
            }
        }
    }

    angles
}

fn angle(first: Vector3<f32>, second: Vector3<f32>) -> f32 {
    (first.dot(&second) / (first.norm() * second.norm()))
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

fn assert_fractional_positions_are_wrapped(structure: &Structure) {
    let cell = structure.cell.as_ref().expect("cell");

    for atom in &structure.atoms {
        let frac = cell.cartesian_to_fractional(atom.position);
        assert!(
            (-0.0001..=1.0001).contains(&frac.x)
                && (-0.0001..=1.0001).contains(&frac.y)
                && (-0.0001..=1.0001).contains(&frac.z),
            "atom outside unit cell: {:?}",
            frac
        );
    }
}
