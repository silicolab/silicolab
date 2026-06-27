use anyhow::Result;
use nalgebra::{Point3, Rotation3, Vector3};

use crate::domain::{Atom, Bond, BondType, UnitCell};
use crate::engines::forcefield::equilibrium_bond_length;
use crate::workflows::reticular::library::{ComponentTemplate, CoordinationSite};
use crate::workflows::reticular::recipe::{LinkerDirection, ReticularBuildSpec};

#[derive(Default)]
pub(super) struct BuildGraph {
    pub(super) atoms: Vec<Atom>,
    pub(super) bonds: Vec<Bond>,
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

pub(super) fn build_honeycomb_direct_stack(
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

fn template_site_direction(component: &ComponentTemplate, site: &CoordinationSite) -> Vector3<f32> {
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
) -> Option<&CoordinationSite> {
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

pub(super) fn add_bond(bonds: &mut Vec<Bond>, a: usize, b: usize, bond_type: BondType) {
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
