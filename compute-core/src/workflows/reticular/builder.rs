mod functionalization;
mod honeycomb;
mod supercell;
#[cfg(test)]
mod tests;

use anyhow::{Result, bail};

use crate::domain::Structure;

use super::library::{component_template, network_template};
use super::recipe::{NetworkId, ReticularBuildSpec};

use functionalization::{apply_functional_substitutions, validate_component_template};
use honeycomb::build_honeycomb_direct_stack;
use supercell::{expand_supercell, supercell_repeats};

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
