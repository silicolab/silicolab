use anyhow::{Result, bail};
use nalgebra::{Point3, Rotation3, Unit, Vector3};

use crate::{
    domain::chemistry::element_style,
    domain::{Atom, Bond, BondType, Structure, UnitCell},
    engines::forcefield::equilibrium_bond_length,
};

use super::{
    library::{
        ComponentClass, ComponentTemplate, TemplateAtom, TemplateBond, component_template,
        network_template,
    },
    recipe::{CoreSlot, FunctionalizationRule, LinkerDirection, NetworkId, ReticularBuildSpec},
};

pub fn build_framework(spec: &ReticularBuildSpec) -> Result<Structure> {
    let primitive = build_primitive_framework(spec)?;

    Ok(expand_supercell(&primitive, supercell_repeats(spec)))
}

#[cfg(test)]
fn connected_components(structure: &Structure) -> usize {
    let mut visited = vec![false; structure.atoms.len()];
    let mut components = 0;

    for start in 0..structure.atoms.len() {
        if visited[start] {
            continue;
        }

        components += 1;
        let mut stack = vec![start];
        visited[start] = true;

        while let Some(current) = stack.pop() {
            for bond in &structure.bonds {
                let next = if bond.a == current {
                    bond.b
                } else if bond.b == current {
                    bond.a
                } else {
                    continue;
                };

                if !visited[next] {
                    visited[next] = true;
                    stack.push(next);
                }
            }
        }
    }

    components
}

fn build_primitive_framework(spec: &ReticularBuildSpec) -> Result<Structure> {
    let mut primary = component_template(spec.primary, &spec.custom_components);
    let mut secondary = component_template(spec.secondary, &spec.custom_components);
    let mut linkers = spec
        .linkers
        .iter()
        .copied()
        .map(|source| component_template(source, &spec.custom_components))
        .collect::<Vec<_>>();
    let network = network_template(spec.network);

    if spec.functionalization_enabled {
        apply_functional_substitutions(
            &mut primary,
            &mut secondary,
            &mut linkers,
            &spec.functionalizations,
            &spec.custom_components,
        )?;
    }
    validate_component_template(&primary)?;
    validate_component_template(&secondary)?;
    for linker in &linkers {
        validate_component_template(linker)?;
    }

    if primary.connectivity != network.primary_connectivity {
        bail!(
            "{} connectivity mismatch: selected component has {}, network requires {}",
            network.primary_label,
            primary.connectivity,
            network.primary_connectivity
        );
    }

    if let (Some(label), Some(required_connectivity)) =
        (network.secondary_label, network.secondary_connectivity)
        && secondary.connectivity != required_connectivity
    {
        bail!(
            "{} connectivity mismatch: selected component has {}, network requires {}",
            label,
            secondary.connectivity,
            required_connectivity
        );
    }

    let (graph, cell) = match spec.network {
        NetworkId::HoneycombVertexVertex => build_honeycomb_direct_stack(
            &primary,
            &secondary,
            &linkers,
            spec.linker_direction,
            spec.layer_spacing,
            spec,
        )?,
    };

    let mut structure =
        Structure::with_cell_and_bonds(spec.name.clone(), graph.atoms, graph.bonds, cell);
    structure.wrap_atoms_into_cell_preserving_bonds();

    Ok(structure)
}

fn validate_component_template(component: &ComponentTemplate) -> Result<()> {
    match component.class {
        ComponentClass::Core => {}
        ComponentClass::Linker => {
            if component.connectivity != 2 {
                bail!(
                    "{} linker templates must define two substitution sites",
                    component.label
                );
            }
        }
        ComponentClass::FunctionalGroup => {
            if component.atoms.len() != 1 {
                bail!(
                    "{} functional groups must currently define exactly one replacement atom",
                    component.label
                );
            }
        }
    }

    if component.coordination_sites.len() != component.connectivity {
        bail!(
            "{} declares connectivity {} but defines {} coordination sites",
            component.label,
            component.connectivity,
            component.coordination_sites.len()
        );
    }

    for site in &component.coordination_sites {
        if site.binding_atom >= component.atoms.len() {
            bail!(
                "{} coordination site references missing binding atom index {}",
                component.label,
                site.binding_atom
            );
        }
    }

    Ok(())
}

fn apply_functional_substitution(
    template: &mut ComponentTemplate,
    _secondary: &mut ComponentTemplate,
    rule: FunctionalizationRule,
    custom_components: &[String],
) -> Result<()> {
    let Some(group_id) = rule.group else {
        return Ok(());
    };
    let functional_group = component_template(group_id, custom_components);
    if functional_group.class != ComponentClass::FunctionalGroup {
        bail!("selected component is not a functional group");
    }

    if rule.atom_index >= template.atoms.len() {
        bail!("functional substitution target atom does not exist");
    }
    if template.atoms[rule.atom_index].element != "H" {
        bail!("functional substitution target must be hydrogen");
    }

    let h_position = template.atoms[rule.atom_index].position;

    let framework_attachment = find_framework_attachment(template, rule.atom_index);
    let Some((framework_atom_idx, _)) = framework_attachment else {
        bail!("hydrogen atom has no parent atom in framework");
    };
    let framework_atom_pos = template.atoms[framework_atom_idx].position;

    // Calculate the direction from framework atom to H (this is where the functional group should point)
    let target_direction = (h_position - framework_atom_pos).normalize();

    template.atoms.remove(rule.atom_index);

    let old_index = rule.atom_index;
    let mut new_bonds = Vec::new();
    for bond in &template.bonds {
        let mut a = bond.a;
        let mut b = bond.b;

        if a == old_index || b == old_index {
            continue;
        }

        // Atoms after the removed H shift down by one.
        if a > old_index {
            a -= 1;
        }
        if b > old_index {
            b -= 1;
        }
        new_bonds.push(TemplateBond {
            a,
            b,
            bond_type: bond.bond_type,
        });
    }
    template.bonds = new_bonds;

    for site in &mut template.coordination_sites {
        if site.binding_atom > old_index {
            site.binding_atom -= 1;
        }
    }

    let Some(fg_site) = functional_group.coordination_sites.first() else {
        bail!(
            "{} functional group must define one valid substitution site",
            functional_group.label
        );
    };

    let fg_binding_atom = fg_site.binding_atom;
    let fg_binding_pos = functional_group.atoms[fg_binding_atom].position;

    // coordination_position is the position of the leaving atom (Du).
    // Du marks the direction *into* the framework (the direction the functional group
    // approaches the core from).  The hydrogen on the core points *outward*, so the
    // direction we must align with is the opposite of target_direction.
    let fg_leaving_pos = fg_site.coordination_position;
    let fg_attachment_dir = (fg_leaving_pos - fg_binding_pos.coords).normalize();
    let align_direction = -target_direction; // Du points inward, H points outward

    // Determine whether the attachment site on the core is conjugated (sp2/sp).
    // Conjugated sites require coplanarity; saturated sites do not.
    let core_is_conjugated = is_conjugated_attachment(template, framework_atom_idx);

    // Determine whether the functional group binding atom is conjugated.
    let fg_is_conjugated = is_conjugated_functional_group(&functional_group, fg_binding_atom);

    // Compute the desired bond angle based on hybridisation of both fragments.
    // This value is currently used as a diagnostic / future force-field hint;
    // the actual angular geometry is already encoded in target_direction.
    let _desired_angle_deg = ideal_bond_angle(
        template,
        framework_atom_idx,
        &functional_group,
        fg_binding_atom,
    );

    // Calculate rotation to align functional group with target direction.
    // For conjugated systems we enforce coplanarity so that pi orbitals overlap.
    let rotation = if core_is_conjugated && fg_is_conjugated {
        if let Some(core_normal) = estimate_core_plane_normal(template, framework_atom_idx) {
            compute_constrained_rotation(
                fg_attachment_dir,
                align_direction,
                core_normal,
                &functional_group.atoms,
                fg_binding_atom,
            )
        } else {
            Rotation3::rotation_between(&fg_attachment_dir, &align_direction)
                .unwrap_or_else(Rotation3::identity)
        }
    } else {
        Rotation3::rotation_between(&fg_attachment_dir, &align_direction)
            .unwrap_or_else(Rotation3::identity)
    };

    // Compute the actual attachment position for the functional-group binding atom.
    // The binding atom is placed outward from the framework atom (along the original
    // H direction), not inward where Du points.
    let bond_length = equilibrium_bond_length(
        &template.atoms[framework_atom_idx].element,
        &functional_group.atoms[fg_binding_atom].element,
        BondType::Single,
    )?;
    let actual_fg_pos = framework_atom_pos + target_direction * bond_length;

    let placed_fg_positions = place_functional_group_avoiding_clashes(
        template,
        framework_atom_idx,
        &functional_group,
        fg_binding_atom,
        fg_binding_pos,
        actual_fg_pos,
        rotation,
        target_direction,
        core_is_conjugated && fg_is_conjugated,
    );

    let fg_start_idx = template.atoms.len();

    for (i, atom) in functional_group.atoms.iter().enumerate() {
        template.atoms.push(TemplateAtom {
            element: atom.element.clone(),
            position: placed_fg_positions[i],
        });
    }

    let fg_binding_new_idx = fg_start_idx + fg_binding_atom;
    template.bonds.push(TemplateBond {
        a: framework_atom_idx,
        b: fg_binding_new_idx,
        bond_type: crate::domain::BondType::Single,
    });

    for bond in &functional_group.bonds {
        let new_a = fg_start_idx + bond.a;
        let new_b = fg_start_idx + bond.b;
        template.bonds.push(TemplateBond {
            a: new_a,
            b: new_b,
            bond_type: bond.bond_type,
        });
    }

    template
        .coordination_sites
        .retain(|s| s.binding_atom != old_index);
    for site in &mut template.coordination_sites {
        if site.binding_atom > old_index {
            site.binding_atom -= 1;
        }
    }

    Ok(())
}

/// Find the atom in the framework that was bonded to the given H atom
fn find_framework_attachment(
    template: &ComponentTemplate,
    h_index: usize,
) -> Option<(usize, usize)> {
    for (i, bond) in template.bonds.iter().enumerate() {
        if bond.b == h_index {
            return Some((bond.a, i));
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn place_functional_group_avoiding_clashes(
    template: &ComponentTemplate,
    framework_atom_idx: usize,
    functional_group: &ComponentTemplate,
    fg_binding_atom: usize,
    fg_binding_pos: Point3<f32>,
    actual_fg_pos: Point3<f32>,
    base_rotation: Rotation3<f32>,
    bond_axis: Vector3<f32>,
    preserve_conjugation: bool,
) -> Vec<Point3<f32>> {
    let initial_positions = transformed_functional_group_positions(
        functional_group,
        fg_binding_atom,
        fg_binding_pos,
        actual_fg_pos,
        base_rotation,
        bond_axis,
        0.0,
    );
    let initial_score = functional_group_clash_score(
        template,
        framework_atom_idx,
        functional_group,
        fg_binding_atom,
        &initial_positions,
    );

    if initial_score <= f32::EPSILON {
        return initial_positions;
    }

    torsion_candidates()
        .iter()
        .map(|torsion| {
            let positions = transformed_functional_group_positions(
                functional_group,
                fg_binding_atom,
                fg_binding_pos,
                actual_fg_pos,
                base_rotation,
                bond_axis,
                torsion.to_radians(),
            );
            let clash_score = functional_group_clash_score(
                template,
                framework_atom_idx,
                functional_group,
                fg_binding_atom,
                &positions,
            );
            let conjugation_penalty = if preserve_conjugation {
                torsion.abs() * 0.0001
            } else {
                0.0
            };

            (clash_score + conjugation_penalty, positions)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, positions)| positions)
        .unwrap_or(initial_positions)
}

fn transformed_functional_group_positions(
    functional_group: &ComponentTemplate,
    fg_binding_atom: usize,
    fg_binding_pos: Point3<f32>,
    actual_fg_pos: Point3<f32>,
    base_rotation: Rotation3<f32>,
    bond_axis: Vector3<f32>,
    torsion_radians: f32,
) -> Vec<Point3<f32>> {
    let axis = Unit::new_normalize(bond_axis);
    let torsion = Rotation3::from_axis_angle(&axis, torsion_radians);

    functional_group
        .atoms
        .iter()
        .enumerate()
        .map(|(index, atom)| {
            if index == fg_binding_atom {
                actual_fg_pos
            } else {
                let relative_pos = atom.position - fg_binding_pos;
                actual_fg_pos + torsion * (base_rotation * relative_pos)
            }
        })
        .collect()
}

fn functional_group_clash_score(
    template: &ComponentTemplate,
    framework_atom_idx: usize,
    functional_group: &ComponentTemplate,
    fg_binding_atom: usize,
    positions: &[Point3<f32>],
) -> f32 {
    let mut score = 0.0;

    for (fg_index, fg_atom) in functional_group.atoms.iter().enumerate() {
        for (template_index, template_atom) in template.atoms.iter().enumerate() {
            if fg_index == fg_binding_atom && template_index == framework_atom_idx {
                continue;
            }

            let distance = nalgebra::distance(&positions[fg_index], &template_atom.position);
            let min_distance =
                steric_radius(&fg_atom.element) + steric_radius(&template_atom.element);

            if distance < min_distance {
                let overlap = min_distance - distance;
                score += overlap * overlap;
            }
        }
    }

    score
}

fn torsion_candidates() -> &'static [f32] {
    &[
        0.0, 30.0, -30.0, 60.0, -60.0, 90.0, -90.0, 120.0, -120.0, 150.0, -150.0, 180.0,
    ]
}

fn steric_radius(element: &str) -> f32 {
    element_style(element).covalent_radius * 1.15
}

fn apply_functional_substitutions(
    primary: &mut ComponentTemplate,
    secondary: &mut ComponentTemplate,
    linkers: &mut [ComponentTemplate],
    rules: &[FunctionalizationRule],
    custom_components: &[String],
) -> Result<()> {
    // Sort rules by atom_index in descending order to avoid index shifting issues
    // when multiple substitutions are applied to the same component
    let mut primary_rules: Vec<_> = rules
        .iter()
        .filter(|r| r.slot == CoreSlot::Primary)
        .copied()
        .collect();
    let mut secondary_rules: Vec<_> = rules
        .iter()
        .filter(|r| r.slot == CoreSlot::Secondary)
        .copied()
        .collect();
    let mut linker_rules: Vec<_> = rules
        .iter()
        .filter(|r| matches!(r.slot, CoreSlot::Linker(_)))
        .copied()
        .collect();

    primary_rules.sort_by_key(|r| std::cmp::Reverse(r.atom_index));
    secondary_rules.sort_by_key(|r| std::cmp::Reverse(r.atom_index));
    linker_rules.sort_by_key(|r| std::cmp::Reverse(r.atom_index));

    for rule in primary_rules {
        apply_functional_substitution(primary, secondary, rule, custom_components)?;
    }
    for rule in secondary_rules {
        apply_functional_substitution(secondary, primary, rule, custom_components)?;
    }
    for rule in linker_rules {
        let CoreSlot::Linker(index) = rule.slot else {
            continue;
        };
        if let Some(linker) = linkers.get_mut(index) {
            apply_functional_substitution(linker, secondary, rule, custom_components)?;
        }
    }

    Ok(())
}

/// Infer whether an atom in the core is part of a conjugated (sp2 or sp) system.
/// We look at the bond types connected to the atom: aromatic or double bonds
/// indicate conjugation.
fn is_conjugated_attachment(template: &ComponentTemplate, atom_idx: usize) -> bool {
    for bond in &template.bonds {
        let other = if bond.a == atom_idx {
            bond.b
        } else if bond.b == atom_idx {
            bond.a
        } else {
            continue;
        };
        match bond.bond_type {
            BondType::Double | BondType::Triple | BondType::Aromatic => return true,
            _ => {}
        }
        // Also check if the neighbor itself has multiple bonds (allylic / benzylic)
        for nb in &template.bonds {
            let nb_other = if nb.a == other {
                nb.b
            } else if nb.b == other {
                nb.a
            } else {
                continue;
            };
            if nb_other == atom_idx {
                continue;
            }
            match nb.bond_type {
                BondType::Double | BondType::Triple | BondType::Aromatic => return true,
                _ => {}
            }
        }
    }
    false
}

/// Infer whether the binding atom of a functional group is conjugated.
fn is_conjugated_functional_group(fg: &ComponentTemplate, binding_atom: usize) -> bool {
    for bond in &fg.bonds {
        if bond.a == binding_atom || bond.b == binding_atom {
            match bond.bond_type {
                BondType::Double | BondType::Triple | BondType::Aromatic => return true,
                _ => {}
            }
        }
    }
    false
}

/// Estimate the ideal C–C(=X) bond angle (in degrees) from the hybridisation of
/// the two atoms that will be bonded.
///
/// Rules (simplified, sufficient for common organic fragments):
/// - sp2–sp2 : 120° (e.g. phenyl–CHO)
/// - sp2–sp3 : 120° (e.g. alkene–CH3, the substituent points away in the plane)
/// - sp3–sp2 : ~120° as well (tetrahedral carbon attached to sp2 carbon)
/// - sp3–sp3 : 109.5°
/// - sp  –*   : 180°
fn ideal_bond_angle(
    core: &ComponentTemplate,
    core_atom: usize,
    fg: &ComponentTemplate,
    fg_atom: usize,
) -> f32 {
    let core_hybrid = estimate_hybridisation(core, core_atom);
    let fg_hybrid = estimate_hybridisation(fg, fg_atom);

    match (core_hybrid, fg_hybrid) {
        (Hybridisation::Sp, _) | (_, Hybridisation::Sp) => 180.0,
        (Hybridisation::Sp2, Hybridisation::Sp2) => 120.0,
        (Hybridisation::Sp2, Hybridisation::Sp3) => 120.0,
        (Hybridisation::Sp3, Hybridisation::Sp2) => 120.0,
        (Hybridisation::Sp3, Hybridisation::Sp3) => 109.47,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hybridisation {
    Sp,
    Sp2,
    Sp3,
}

/// Estimate hybridisation from bond types and number of attached atoms.
fn estimate_hybridisation(template: &ComponentTemplate, atom_idx: usize) -> Hybridisation {
    let mut bond_order_sum = 0.0_f32;
    let mut neighbor_count = 0;

    for bond in &template.bonds {
        let _other = if bond.a == atom_idx {
            bond.b
        } else if bond.b == atom_idx {
            bond.a
        } else {
            continue;
        };
        neighbor_count += 1;
        bond_order_sum += match bond.bond_type {
            BondType::Single => 1.0,
            BondType::Double => 2.0,
            BondType::Triple => 3.0,
            BondType::Aromatic => 1.5,
        };
    }

    // Use bond-order sum as primary heuristic
    if bond_order_sum >= 2.5 {
        Hybridisation::Sp
    } else if (1.5..2.5).contains(&bond_order_sum) {
        Hybridisation::Sp2
    } else {
        // For bond-order ~1, use neighbor count
        match neighbor_count {
            0..=2 => Hybridisation::Sp, // e.g. terminal atoms, but usually shouldn't happen
            3 => Hybridisation::Sp2,
            _ => Hybridisation::Sp3,
        }
    }
}

/// Estimate the normal vector of the local plane around `center_atom` in the template.
/// Returns `Some(normal)` if the atom and its neighbors appear planar (all lie roughly
/// in the same plane), or `None` for non-planar environments.
fn estimate_core_plane_normal(
    template: &ComponentTemplate,
    center_atom: usize,
) -> Option<Vector3<f32>> {
    let neighbor_positions: Vec<Point3<f32>> = template
        .bonds
        .iter()
        .filter_map(|bond| {
            if bond.a == center_atom {
                Some(template.atoms[bond.b].position)
            } else if bond.b == center_atom {
                Some(template.atoms[bond.a].position)
            } else {
                None
            }
        })
        .collect();

    if neighbor_positions.len() < 3 {
        return None;
    }

    // Compute normal from first two neighbor vectors (sufficient for planar cores like benzene)
    let v1 = neighbor_positions[1] - neighbor_positions[0];
    let v2 = neighbor_positions[2] - neighbor_positions[0];
    let normal = v1.cross(&v2);
    let norm = normal.norm();
    if norm < 1e-6 {
        return None;
    }
    let normal = normal / norm;

    // Verify planarity: all neighbors should have similar normal direction
    for i in 1..neighbor_positions.len().saturating_sub(1) {
        let vi = neighbor_positions[i + 1] - neighbor_positions[0];
        let cross = v1.cross(&vi);
        let cross_norm = cross.norm();
        if cross_norm > 1e-6 {
            let local_normal = cross / cross_norm;
            if local_normal.dot(&normal).abs() < 0.9 {
                return None;
            }
        }
    }

    Some(normal)
}

/// Compute a rotation that aligns `source_dir` to `target_dir` while also keeping
/// the functional group's plane coplanar with the core's plane.
///
/// The algorithm:
/// 1. Find the functional group's in-plane reference vector (a vector from the binding
///    atom to another atom in the group, projected perpendicular to `source_dir`).
/// 2. This reference vector should end up perpendicular to `core_normal` after rotation.
/// 3. Build an orthonormal basis for source space and target space, then construct
///    the rotation matrix directly.
fn compute_constrained_rotation(
    source_dir: Vector3<f32>,
    target_dir: Vector3<f32>,
    core_normal: Vector3<f32>,
    fg_atoms: &[TemplateAtom],
    fg_binding_atom: usize,
) -> Rotation3<f32> {
    // Step 1: Find a reference vector in the functional group's plane.
    // We look for another atom in the group (preferably the heaviest / most distinct)
    // to define an in-plane direction.
    let mut ref_vec = Vector3::zeros();
    for (i, atom) in fg_atoms.iter().enumerate() {
        if i == fg_binding_atom {
            continue;
        }
        let delta: Vector3<f32> = atom.position - fg_atoms[fg_binding_atom].position;
        let perp = delta - source_dir * source_dir.dot(&delta);
        if perp.norm() > ref_vec.norm() {
            ref_vec = perp;
        }
    }

    if ref_vec.norm() < 1e-6 {
        // Functional group is effectively linear (e.g. cyano) – single-axis alignment is enough.
        return Rotation3::rotation_between(&source_dir, &target_dir)
            .unwrap_or_else(Rotation3::identity);
    }
    ref_vec = ref_vec.normalize();

    // Step 2: Build source orthonormal basis {source_dir, ref_vec, source_normal}
    let source_normal = source_dir.cross(&ref_vec).normalize();

    // Step 3: Build target orthonormal basis.
    // target_dir is fixed. We need a target_ref that is perpendicular to both target_dir
    // and core_normal (i.e. lies in the core plane and is perpendicular to target_dir).
    let target_ref = core_normal.cross(&target_dir);
    let target_ref_norm = target_ref.norm();
    let (target_ref, target_normal) = if target_ref_norm > 1e-6 {
        let tr = target_ref / target_ref_norm;
        let tn = target_dir.cross(&tr).normalize();
        (tr, tn)
    } else {
        // target_dir is parallel to core_normal – planar constraint is degenerate.
        return Rotation3::rotation_between(&source_dir, &target_dir)
            .unwrap_or_else(Rotation3::identity);
    };

    // Step 4: Construct rotation matrix from source basis to target basis.
    // R * [source_dir, ref_vec, source_normal] = [target_dir, target_ref, target_normal]
    // => R = target_basis * source_basis^T
    let source_basis = nalgebra::Matrix3::from_columns(&[source_dir, ref_vec, source_normal]);
    let target_basis = nalgebra::Matrix3::from_columns(&[target_dir, target_ref, target_normal]);
    let rotation_matrix = target_basis * source_basis.transpose();

    Rotation3::from_matrix(&rotation_matrix)
}

#[derive(Default)]
struct BuildGraph {
    atoms: Vec<Atom>,
    bonds: Vec<Bond>,
}

struct BuildContext<'a> {
    graph: &'a mut BuildGraph,
    edge_linkers: &'a mut Vec<EdgeLinker>,
}

struct ComponentInstance {
    coordination_sites: Vec<PlacedCoordinationSite>,
}

struct EdgeLinker {
    chain: Vec<LinkedComponentInstance>,
}

struct LinkedComponentInstance {
    instance: ComponentInstance,
    incoming_site: usize,
    outgoing_site: usize,
}

#[derive(Clone)]
struct PlacedCoordinationSite {
    binding_atom: usize,
    binding_position: Point3<f32>,
    coordination_position: Point3<f32>,
}

fn build_honeycomb_direct_stack(
    vertex_a: &ComponentTemplate,
    vertex_b: &ComponentTemplate,
    linkers: &[ComponentTemplate],
    linker_direction: LinkerDirection,
    layer_spacing: f32,
    spec: &ReticularBuildSpec,
) -> Result<(BuildGraph, UnitCell)> {
    let period = spec.stacking_period();
    let mut stack = BuildGraph::default();
    let mut stack_cell = None;

    for layer in 0..period {
        let (layer_graph, layer_cell) = build_honeycomb_direct_layer(
            vertex_a,
            vertex_b,
            linkers,
            linker_direction,
            layer_spacing,
            spec.primary_layer_azimuth_degrees(layer),
            spec.secondary_layer_azimuth_degrees(layer),
        )?;
        let z_offset = layer_cell.vectors[2] * (layer as f32 + 0.5);

        append_layer_graph(&mut stack, &layer_graph, z_offset);
        stack_cell = Some(layer_cell);
    }

    let layer_cell = stack_cell.expect("stacking period must include at least one layer");
    let cell = UnitCell::from_vectors([
        layer_cell.vectors[0],
        layer_cell.vectors[1],
        layer_cell.vectors[2] * period as f32,
    ]);

    Ok((stack, cell))
}

fn append_layer_graph(stack: &mut BuildGraph, layer: &BuildGraph, offset: Vector3<f32>) {
    let atom_offset = stack.atoms.len();

    stack.atoms.extend(layer.atoms.iter().map(|atom| Atom {
        element: atom.element.clone(),
        position: atom.position + offset,
        charge: atom.charge,
    }));

    stack.bonds.extend(
        layer.bonds.iter().map(|bond| {
            Bond::with_type(atom_offset + bond.a, atom_offset + bond.b, bond.bond_type)
        }),
    );
}

fn build_honeycomb_direct_layer(
    vertex_a: &ComponentTemplate,
    vertex_b: &ComponentTemplate,
    linkers: &[ComponentTemplate],
    linker_direction: LinkerDirection,
    layer_spacing: f32,
    primary_azimuth_degrees: f32,
    secondary_azimuth_degrees: f32,
) -> Result<(BuildGraph, UnitCell)> {
    let directions = [radial(90.0), radial(210.0), radial(330.0)];
    let mut graph = BuildGraph::default();

    let vertex_a_instance = append_component(
        &mut graph,
        vertex_a,
        Point3::origin(),
        primary_azimuth_degrees,
    );
    let vertex_b_angle = component_alignment_angle(vertex_b, &vertex_a_instance, &directions)
        .unwrap_or(180.0)
        + secondary_azimuth_degrees;
    let mut edge_linkers = Vec::new();
    let mut ctx = BuildContext {
        graph: &mut graph,
        edge_linkers: &mut edge_linkers,
    };
    let origins = if !linkers.is_empty() {
        origins_matching_linked_sites(
            &mut ctx,
            linkers,
            vertex_b,
            vertex_b_angle,
            &vertex_a_instance,
            &directions,
            linker_direction,
        )?
    } else {
        origins_matching_binding_atoms_to_coordination_sites(
            vertex_b,
            vertex_b_angle,
            &graph,
            &vertex_a_instance,
            &directions,
        )?
    };
    if origins.len() < 3 {
        let c_vec = Vector3::new(0.0, 0.0, layer_spacing.max(1.0));
        return Ok((
            graph,
            UnitCell::from_vectors([radial(0.0), radial(120.0), c_vec]),
        ));
    }
    let vertex_b_instance = append_component(&mut graph, vertex_b, origins[0], vertex_b_angle);
    let a_vec = origins[1] - origins[0];
    let b_vec = origins[2] - origins[0];
    let c_vec = Vector3::new(0.0, 0.0, layer_spacing.max(1.0));
    let cell = UnitCell::from_vectors([a_vec, b_vec, c_vec]);

    for (index, direction) in directions.iter().copied().enumerate() {
        let Some(site_a) = coordination_site_facing(&vertex_a_instance, direction) else {
            continue;
        };

        if let Some(edge_linker) = edge_linkers.get(index) {
            if let Some(first_linker) = edge_linker.chain.first()
                && let Some(first_site) = first_linker
                    .instance
                    .coordination_sites
                    .get(first_linker.incoming_site)
            {
                add_coordination_bond(&mut graph, site_a.binding_atom, first_site.binding_atom);
            }
            for pair in edge_linker.chain.windows(2) {
                if let [previous, next] = pair
                    && let (Some(previous_site), Some(next_site)) = (
                        previous
                            .instance
                            .coordination_sites
                            .get(previous.outgoing_site),
                        next.instance.coordination_sites.get(next.incoming_site),
                    )
                {
                    add_coordination_bond(
                        &mut graph,
                        previous_site.binding_atom,
                        next_site.binding_atom,
                    );
                }
            }
            if let Some(last_linker) = edge_linker.chain.last()
                && let Some(last_site) = last_linker
                    .instance
                    .coordination_sites
                    .get(last_linker.outgoing_site)
                && let Some(site_b) =
                    coordination_site_facing(&vertex_b_instance, -site_direction(last_site))
            {
                add_coordination_bond(&mut graph, last_site.binding_atom, site_b.binding_atom);
            }
        } else if let Some(site_b) =
            coordination_site_facing(&vertex_b_instance, -site_direction(site_a))
        {
            add_coordination_bond(&mut graph, site_a.binding_atom, site_b.binding_atom);
        } else {
            continue;
        }
    }

    Ok((graph, cell))
}

fn append_component(
    graph: &mut BuildGraph,
    component: &ComponentTemplate,
    origin: Point3<f32>,
    angle_degrees: f32,
) -> ComponentInstance {
    let rotation = Rotation3::from_axis_angle(&Vector3::z_axis(), angle_degrees.to_radians());
    let offset = graph.atoms.len();

    for atom in &component.atoms {
        let position = origin + rotation * atom.position.coords;

        graph.atoms.push(Atom {
            element: atom.element.to_string(),
            position,
            charge: 0.0,
        });
    }

    for bond in &component.bonds {
        add_bond(
            &mut graph.bonds,
            offset + bond.a,
            offset + bond.b,
            bond.bond_type,
        );
    }

    let coordination_sites = component
        .coordination_sites
        .iter()
        .map(|site| PlacedCoordinationSite {
            binding_atom: offset + site.binding_atom,
            binding_position: origin
                + rotation * component.atoms[site.binding_atom].position.coords,
            coordination_position: origin + rotation * site.coordination_position,
        })
        .collect();

    ComponentInstance { coordination_sites }
}

fn component_alignment_angle(
    component: &ComponentTemplate,
    target_instance: &ComponentInstance,
    directions: &[Vector3<f32>],
) -> Option<f32> {
    let first_target = directions
        .iter()
        .find_map(|direction| coordination_site_facing(target_instance, *direction))?;
    let first_target_direction = -site_direction(first_target);
    let local_directions = component
        .coordination_sites
        .iter()
        .map(|site| template_site_direction(component, site))
        .collect::<Vec<_>>();

    local_directions
        .iter()
        .filter_map(|local_direction| {
            let candidate_angle = signed_z_angle(*local_direction, first_target_direction)?;
            let rotation = Rotation3::from_axis_angle(&Vector3::z_axis(), candidate_angle);
            let score = directions
                .iter()
                .filter_map(|direction| coordination_site_facing(target_instance, *direction))
                .map(|target_site| {
                    let target_direction = -site_direction(target_site);
                    local_directions
                        .iter()
                        .map(|local_direction| {
                            (rotation * *local_direction)
                                .try_normalize(0.0001)
                                .unwrap_or_else(Vector3::zeros)
                                .dot(&target_direction)
                        })
                        .fold(f32::NEG_INFINITY, f32::max)
                })
                .sum::<f32>();

            Some((score, candidate_angle.to_degrees()))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, angle)| angle)
}

fn origins_matching_linked_sites(
    ctx: &mut BuildContext,
    linkers: &[ComponentTemplate],
    component: &ComponentTemplate,
    angle_degrees: f32,
    target_instance: &ComponentInstance,
    directions: &[Vector3<f32>],
    linker_direction: LinkerDirection,
) -> Result<Vec<Point3<f32>>> {
    let rotation = Rotation3::from_axis_angle(&Vector3::z_axis(), angle_degrees.to_radians());
    let mut origins = Vec::new();
    let (incoming_linker_site, outgoing_linker_site) = match linker_direction {
        LinkerDirection::PrimaryToSecondary => (0, 1),
        LinkerDirection::SecondaryToPrimary => (1, 0),
    };

    for direction in directions {
        let Some(target_site) = coordination_site_facing(target_instance, *direction) else {
            continue;
        };
        let primary_outward = site_direction(target_site);
        let mut previous_binding_atom = target_site.binding_atom;
        let mut previous_outward = primary_outward;
        let mut chain = Vec::new();

        for linker in linkers {
            let Some(linker_template_site) = linker.coordination_sites.get(incoming_linker_site)
            else {
                continue;
            };
            let target_binding_position = ctx.graph.atoms[previous_binding_atom].position
                + previous_outward
                    * equilibrium_bond_length(
                        &ctx.graph.atoms[previous_binding_atom].element,
                        &linker.atoms[linker_template_site.binding_atom].element,
                        BondType::Single,
                    )?;
            let linker_instance = append_component_by_site(
                ctx.graph,
                linker,
                incoming_linker_site,
                target_binding_position,
                -previous_outward,
            );
            let Some(outgoing_site) = linker_instance.coordination_sites.get(outgoing_linker_site)
            else {
                continue;
            };
            previous_binding_atom = outgoing_site.binding_atom;
            previous_outward = site_direction(outgoing_site);
            chain.push(LinkedComponentInstance {
                instance: linker_instance,
                incoming_site: incoming_linker_site,
                outgoing_site: outgoing_linker_site,
            });
        }

        if chain.len() != linkers.len() {
            continue;
        }
        let Some(source_site) =
            template_coordination_site_facing(component, angle_degrees, -previous_outward)
        else {
            continue;
        };
        let binding_position = rotation * component.atoms[source_site.binding_atom].position.coords;
        let target_position = ctx.graph.atoms[previous_binding_atom].position
            + previous_outward
                * equilibrium_bond_length(
                    &ctx.graph.atoms[previous_binding_atom].element,
                    &component.atoms[source_site.binding_atom].element,
                    BondType::Single,
                )?;

        origins.push(target_position - binding_position);
        ctx.edge_linkers.push(EdgeLinker { chain });
    }

    Ok(origins)
}

fn append_component_by_site(
    graph: &mut BuildGraph,
    component: &ComponentTemplate,
    site_index: usize,
    target_binding_position: Point3<f32>,
    target_site_direction: Vector3<f32>,
) -> ComponentInstance {
    let Some(site) = component.coordination_sites.get(site_index) else {
        return append_component(graph, component, target_binding_position, 0.0);
    };
    let local_delta =
        site.coordination_position - component.atoms[site.binding_atom].position.coords;
    let target_delta = target_site_direction
        .try_normalize(0.0001)
        .unwrap_or_else(Vector3::x);
    let angle_degrees = target_delta.y.atan2(target_delta.x).to_degrees()
        - local_delta.y.atan2(local_delta.x).to_degrees();
    let rotation = Rotation3::from_axis_angle(&Vector3::z_axis(), angle_degrees.to_radians());
    let origin =
        target_binding_position - rotation * component.atoms[site.binding_atom].position.coords;

    append_component(graph, component, origin, angle_degrees)
}

fn origins_matching_binding_atoms_to_coordination_sites(
    component: &ComponentTemplate,
    angle_degrees: f32,
    graph: &BuildGraph,
    target_instance: &ComponentInstance,
    directions: &[Vector3<f32>],
) -> Result<Vec<Point3<f32>>> {
    let rotation = Rotation3::from_axis_angle(&Vector3::z_axis(), angle_degrees.to_radians());
    let mut origins = Vec::new();

    for direction in directions {
        let Some(target_site) = coordination_site_facing(target_instance, *direction) else {
            continue;
        };
        let target_outward = site_direction(target_site);
        let Some(source_site) =
            template_coordination_site_facing(component, angle_degrees, -target_outward)
        else {
            continue;
        };
        let binding_position = rotation * component.atoms[source_site.binding_atom].position.coords;
        let target_position = graph.atoms[target_site.binding_atom].position
            + target_outward
                * equilibrium_bond_length(
                    &graph.atoms[target_site.binding_atom].element,
                    &component.atoms[source_site.binding_atom].element,
                    BondType::Single,
                )?;
        origins.push(target_position - binding_position);
    }

    Ok(origins)
}

fn site_direction(site: &PlacedCoordinationSite) -> Vector3<f32> {
    (site.coordination_position - site.binding_position)
        .try_normalize(0.0001)
        .unwrap_or_else(Vector3::x)
}

fn template_site_direction(
    component: &ComponentTemplate,
    site: &super::library::CoordinationSite,
) -> Vector3<f32> {
    (site.coordination_position - component.atoms[site.binding_atom].position.coords)
        .try_normalize(0.0001)
        .unwrap_or_else(Vector3::x)
}

fn signed_z_angle(from: Vector3<f32>, to: Vector3<f32>) -> Option<f32> {
    let from = from.try_normalize(0.0001)?;
    let to = to.try_normalize(0.0001)?;

    Some((from.x * to.y - from.y * to.x).atan2(from.x * to.x + from.y * to.y))
}

fn coordination_site_facing(
    instance: &ComponentInstance,
    direction: Vector3<f32>,
) -> Option<&PlacedCoordinationSite> {
    let direction = direction.try_normalize(0.0001)?;

    instance.coordination_sites.iter().max_by(|a, b| {
        let a_score = site_direction(a).dot(&direction);
        let b_score = site_direction(b).dot(&direction);
        a_score.total_cmp(&b_score)
    })
}

fn template_coordination_site_facing(
    component: &ComponentTemplate,
    angle_degrees: f32,
    direction: Vector3<f32>,
) -> Option<&super::library::CoordinationSite> {
    let rotation = Rotation3::from_axis_angle(&Vector3::z_axis(), angle_degrees.to_radians());
    let direction = direction.try_normalize(0.0001)?;

    component.coordination_sites.iter().max_by(|a, b| {
        let a_score = (rotation * template_site_direction(component, a))
            .try_normalize(0.0001)
            .unwrap_or_else(Vector3::zeros)
            .dot(&direction);
        let b_score = (rotation * template_site_direction(component, b))
            .try_normalize(0.0001)
            .unwrap_or_else(Vector3::zeros)
            .dot(&direction);
        a_score.total_cmp(&b_score)
    })
}

fn add_coordination_bond(graph: &mut BuildGraph, first_atom: usize, second_atom: usize) {
    add_bond(&mut graph.bonds, first_atom, second_atom, BondType::Single);
}

fn add_bond(bonds: &mut Vec<Bond>, a: usize, b: usize, bond_type: BondType) {
    if a == b {
        return;
    }
    let (a, b) = if a < b { (a, b) } else { (b, a) };

    if !bonds.iter().any(|bond| bond.a == a && bond.b == b) {
        bonds.push(Bond::with_type(a, b, bond_type));
    }
}

fn radial(angle_degrees: f32) -> Vector3<f32> {
    let angle = angle_degrees.to_radians();

    Vector3::new(angle.cos(), angle.sin(), 0.0)
}

fn supercell_repeats(spec: &ReticularBuildSpec) -> [u32; 3] {
    let mut repeats = spec.supercell;
    let period = spec.stacking_period();
    if period > 1 {
        repeats[2] = repeats[2].max(1).div_ceil(period);
    }
    repeats
}

fn expand_supercell(structure: &Structure, supercell: [u32; 3]) -> Structure {
    let Some(cell) = &structure.cell else {
        return structure.clone();
    };
    let nx = supercell[0].max(1);
    let ny = supercell[1].max(1);
    let nz = supercell[2].max(1);
    let source_atom_count = structure.atoms.len();
    let expanded_cell = UnitCell::from_parameters(
        cell.a * nx as f32,
        cell.b * ny as f32,
        cell.c * nz as f32,
        cell.alpha,
        cell.beta,
        cell.gamma,
    );
    let mut atoms = Vec::new();
    let mut bonds = Vec::new();

    for ix in 0..nx {
        for iy in 0..ny {
            for iz in 0..nz {
                for atom in &structure.atoms {
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
                for bond in &structure.bonds {
                    let shift = bond_cell_shift(structure, cell, bond);
                    let jx = (ix as i32 + shift.x).rem_euclid(nx as i32) as u32;
                    let jy = (iy as i32 + shift.y).rem_euclid(ny as i32) as u32;
                    let jz = (iz as i32 + shift.z).rem_euclid(nz as i32) as u32;
                    let a = expanded_index(ix, iy, iz, bond.a, ny, nz, source_atom_count);
                    let b = expanded_index(jx, jy, jz, bond.b, ny, nz, source_atom_count);

                    add_bond(&mut bonds, a, b, bond.bond_type);
                }
            }
        }
    }

    Structure::with_cell_and_bonds(structure.title.clone(), atoms, bonds, expanded_cell)
}

fn bond_cell_shift(structure: &Structure, cell: &UnitCell, bond: &Bond) -> CellShift {
    let first = cell.cartesian_to_fractional(structure.atoms[bond.a].position);
    let second = cell.cartesian_to_fractional(structure.atoms[bond.b].position);
    let delta = second - first;

    CellShift {
        x: -delta.x.round() as i32,
        y: -delta.y.round() as i32,
        z: -delta.z.round() as i32,
    }
}

struct CellShift {
    x: i32,
    y: i32,
    z: i32,
}

fn expanded_index(
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
    use nalgebra::Vector3;

    use super::{build_framework, connected_components};
    use crate::domain::{BondType, Structure};
    use crate::workflows::reticular::{
        ComponentSource, CoreSlot, FunctionalizationRule, NetworkId, ReticularBuildSpec,
        StackingMode, component_template,
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
                include_str!("building_block/core/t3/benzene.slf").to_string(),
                include_str!("building_block/linker/trans_ethene.slf").to_string(),
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
        let first_substitution_direction = first_site.coordination_position
            - linker.atoms[first_site.binding_atom].position.coords;
        let second_substitution_direction = second_site.coordination_position
            - linker.atoms[second_site.binding_atom].position.coords;

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
}
