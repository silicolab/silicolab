use anyhow::{Result, bail};
use nalgebra::{Point3, Rotation3, Unit, Vector3};

use crate::domain::BondType;
use crate::domain::chemistry::element_style;
use crate::engines::forcefield::equilibrium_bond_length;
use crate::workflows::reticular::library::{
    ComponentClass, ComponentTemplate, TemplateAtom, TemplateBond, component_template,
};
use crate::workflows::reticular::recipe::{CoreSlot, FunctionalizationRule};

pub(super) fn validate_component_template(component: &ComponentTemplate) -> Result<()> {
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

pub(super) fn apply_functional_substitutions(
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
