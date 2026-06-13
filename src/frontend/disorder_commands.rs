//! Console command `disorder` (alias `pack`): packs molecules into a box,
//! sphere, or cylinder. Headless, so it runs the
//! [`crate::workflows::packing`] engine synchronously (no poll loop) and adds the
//! result as a new entry — exactly the engine the GUI task uses, one source of
//! truth.

use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Result, anyhow, bail};
use nalgebra::{Point3, Vector3};

use crate::frontend::state::AppState;
use crate::io::structure_io;
use crate::workflows::packing::{
    PackLimits, PackRequest, PackSpecies, Region, RegionSense, count_for_concentration_molar,
    count_for_density_g_per_cm3, pack,
};

/// How much of a molecule to pack.
enum Amount {
    Count(usize),
    Density(f32),
    Concentration(f32),
}

/// One `--of` molecule and its amount.
struct ComponentArg {
    entry: String,
    amount: Amount,
}

pub fn disorder_command(state: &mut AppState, args: &[String]) -> Result<String> {
    if args.is_empty() {
        return Ok(USAGE.to_string());
    }

    let mut components: Vec<ComponentArg> = Vec::new();
    let mut box_lengths: Option<[f32; 3]> = None;
    let mut sphere_radius: Option<f32> = None;
    let mut cylinder: Option<(f32, f32)> = None;
    let mut outside = false;
    let mut tolerance = 2.0f32;
    let mut seed = 1u64;
    let mut avoid: Option<String> = None;
    let mut name = "Disordered system".to_string();
    let mut set_cell = true;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let key_raw = arg
            .strip_prefix("--")
            .ok_or_else(|| anyhow!("unexpected argument `{arg}` (expected --key value)"))?;
        let (key, inline) = match key_raw.split_once('=') {
            Some((k, v)) => (k, Some(v.to_string())),
            None => (key_raw, None),
        };
        i += 1;

        // `--outside` / `--no-cell` take no value; everything else does.
        let valueless = matches!(key, "outside" | "no-cell");
        let value = if let Some(v) = inline {
            Some(v)
        } else if valueless {
            None
        } else {
            let v = args
                .get(i)
                .filter(|next| !next.starts_with("--"))
                .cloned()
                .ok_or_else(|| anyhow!("--{key} needs a value"))?;
            i += 1;
            Some(v)
        };

        match key {
            "of" => components.push(ComponentArg {
                entry: value.unwrap(),
                amount: Amount::Count(1),
            }),
            "count" => set_last_amount(
                &mut components,
                key,
                |v| Ok(Amount::Count(parse_u(v)?)),
                &value,
            )?,
            "density" => set_last_amount(
                &mut components,
                key,
                |v| Ok(Amount::Density(parse_f(v)?)),
                &value,
            )?,
            "conc" => set_last_amount(
                &mut components,
                key,
                |v| Ok(Amount::Concentration(parse_f(v)?)),
                &value,
            )?,
            "box" => box_lengths = Some(parse_triple(&value.unwrap())?),
            "sphere" => sphere_radius = Some(parse_f(&value.unwrap())?),
            "cylinder" => cylinder = Some(parse_pair(&value.unwrap())?),
            "outside" => outside = true,
            "tolerance" => tolerance = parse_f(&value.unwrap())?,
            "seed" => seed = parse_u(&value.unwrap())? as u64,
            "avoid" => avoid = Some(value.unwrap()),
            "name" => name = value.unwrap(),
            "no-cell" => set_cell = false,
            other => bail!("unknown option --{other}"),
        }
    }

    if components.is_empty() {
        bail!("specify at least one molecule with --of <entry>");
    }

    // Exactly one region shape.
    let shapes = [
        box_lengths.is_some(),
        sphere_radius.is_some(),
        cylinder.is_some(),
    ]
    .iter()
    .filter(|set| **set)
    .count();
    if shapes == 0 {
        bail!("specify a region: --box <X,Y,Z>, --sphere <R>, or --cylinder <R,L>");
    }
    if shapes > 1 {
        bail!("specify only one region (--box, --sphere, or --cylinder)");
    }
    let is_box = box_lengths.is_some();
    let region = if let Some([x, y, z]) = box_lengths {
        Region::Box {
            min: Point3::origin(),
            max: Point3::new(x, y, z),
        }
    } else if let Some(r) = sphere_radius {
        Region::Sphere {
            center: Point3::new(r, r, r),
            radius: r,
        }
    } else {
        let (r, length) = cylinder.unwrap();
        Region::Cylinder {
            center: Point3::new(r, r, length * 0.5),
            axis: Vector3::new(0.0, 0.0, 1.0),
            radius: r,
            length,
        }
    };
    let sense = if outside {
        RegionSense::Outside
    } else {
        RegionSense::Inside
    };

    // Resolve each molecule and turn its amount into a copy count.
    let mut species = Vec::with_capacity(components.len());
    for component in &components {
        let entry_id = resolve_entry(state, &component.entry)?;
        state.ensure_entry_loaded(entry_id);
        let molecule = state
            .entries
            .entry(entry_id)
            .map(|entry| entry.structure.clone())
            .ok_or_else(|| anyhow!("entry `{}` could not be loaded", component.entry))?;
        if molecule.atoms.is_empty() {
            bail!("molecule `{}` has no atoms to pack", component.entry);
        }
        let count = match component.amount {
            Amount::Count(n) => n,
            Amount::Density(d) => count_for_density_g_per_cm3(&molecule, d, &region),
            Amount::Concentration(c) => count_for_concentration_molar(&molecule, c, &region),
        };
        species.push(PackSpecies { molecule, count });
    }
    if species.iter().all(|s| s.count == 0) {
        bail!("nothing to pack: every molecule resolves to zero copies");
    }

    let fixed = match &avoid {
        Some(token) => {
            let entry_id = resolve_entry(state, token)?;
            state.ensure_entry_loaded(entry_id);
            Some(
                state
                    .entries
                    .entry(entry_id)
                    .map(|entry| entry.structure.clone())
                    .ok_or_else(|| anyhow!("avoid entry `{token}` could not be loaded"))?,
            )
        }
        None => None,
    };

    let output_cell = (set_cell && is_box).then(|| {
        let [x, y, z] = box_lengths.unwrap();
        crate::domain::UnitCell::from_parameters(x, y, z, 90.0, 90.0, 90.0)
    });

    let request = PackRequest {
        species,
        region,
        sense,
        tolerance,
        periodic: false,
        seed,
        fixed,
        output_cell,
        limits: PackLimits::default(),
    };

    let result = pack(request, Arc::new(AtomicBool::new(false)), |_| Ok(()))?;
    let report = result.report.clone();
    let mut structure = result.structure;
    structure.title = name;

    let atom_count = structure.atoms.len();
    let save_path = structure_io::default_structure_save_path(&structure, None);
    let entry_id = state.entries.add_entry(structure, None, save_path);
    state.show_entry(entry_id);

    let placed = report.total_placed();
    let requested = report.total_requested();
    if report.converged {
        Ok(format!(
            "Packed {placed} molecules ({atom_count} atoms) into a disordered system"
        ))
    } else {
        Ok(format!(
            "Packed {placed}/{requested} molecules ({atom_count} atoms); worst overlap {:.2} Å — \
             enlarge the region or lower the density",
            report.max_overlap
        ))
    }
}

const USAGE: &str = "disorder [options]\n  \
    --of <entry>        Molecule to pack (name | \"active\" | id); repeatable\n  \
    --count <n>         Copies of the preceding --of (default 1)\n  \
    --density <g/cm3>   …or auto-count the preceding molecule by mass density\n  \
    --conc <mol/L>      …or by molar concentration\n  \
    --box <X,Y,Z>       Pack into an X×Y×Z Å box\n  \
    --sphere <R>        Pack into a sphere of radius R Å\n  \
    --cylinder <R,L>    Pack into a cylinder (radius R, length L Å)\n  \
    --outside           Pack OUTSIDE the region (sphere/cylinder only)\n  \
    --tolerance <Å>     Min inter-molecule spacing (default 2.0)\n  \
    --seed <n>          RNG seed (default 1)\n  \
    --avoid <entry>     Fixed obstacle structure to pack around\n  \
    --name <text>       Result entry name (default \"Disordered system\")\n  \
    --no-cell           Don't set the region as the result's simulation cell";

/// Resolve an entry token: `"active"`, a numeric id, or a (case-insensitive) name.
fn resolve_entry(state: &AppState, token: &str) -> Result<u64> {
    let token = token.trim();
    if token.eq_ignore_ascii_case("active") {
        return state
            .entries
            .active_entry_id()
            .ok_or_else(|| anyhow!("no active entry to pack"));
    }
    if let Ok(id) = token.parse::<u64>()
        && state.entries.entry(id).is_some()
    {
        return Ok(id);
    }
    if let Some(entry) = state
        .entries
        .records
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(token))
    {
        return Ok(entry.id);
    }
    bail!("no entry matches `{token}` (use a name, an id, or \"active\")")
}

fn set_last_amount(
    components: &mut [ComponentArg],
    key: &str,
    make: impl FnOnce(&str) -> Result<Amount>,
    value: &Option<String>,
) -> Result<()> {
    let value = value
        .as_deref()
        .ok_or_else(|| anyhow!("--{key} needs a value"))?;
    let component = components
        .last_mut()
        .ok_or_else(|| anyhow!("--{key} must follow a --of <entry>"))?;
    component.amount = make(value)?;
    Ok(())
}

fn parse_u(value: &str) -> Result<usize> {
    value
        .trim()
        .parse::<usize>()
        .map_err(|_| anyhow!("invalid integer: {value}"))
}

fn parse_f(value: &str) -> Result<f32> {
    value
        .trim()
        .parse::<f32>()
        .map_err(|_| anyhow!("invalid number: {value}"))
}

fn parse_pair(value: &str) -> Result<(f32, f32)> {
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != 2 {
        bail!("expected two comma-separated numbers, got `{value}`");
    }
    Ok((parse_f(parts[0])?, parse_f(parts[1])?))
}

fn parse_triple(value: &str) -> Result<[f32; 3]> {
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != 3 {
        bail!("expected three comma-separated numbers, got `{value}`");
    }
    Ok([parse_f(parts[0])?, parse_f(parts[1])?, parse_f(parts[2])?])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Structure};

    fn split(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    fn argon() -> Structure {
        Structure::new(
            "argon",
            vec![Atom {
                element: "Ar".to_string(),
                position: Point3::origin(),
                charge: 0.0,
            }],
        )
    }

    fn scratch_with_entry() -> (AppState, u64) {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let save_path = structure_io::default_structure_save_path(&argon(), None);
        let entry_id = state.entries.add_entry(argon(), None, save_path);
        state.show_entry(entry_id);
        (state, entry_id)
    }

    #[test]
    fn packs_active_entry_into_a_box() {
        let (mut state, _) = scratch_with_entry();
        let before = state.entries.records.len();
        let message = disorder_command(
            &mut state,
            &split("--of active --count 8 --box 16,16,16 --tolerance 2.0 --seed 3"),
        )
        .unwrap();
        assert!(message.contains("Packed"), "got: {message}");
        // A new combined entry was added.
        assert_eq!(state.entries.records.len(), before + 1);
        let packed = &state.entries.records.last().unwrap().structure;
        assert_eq!(packed.atoms.len(), 8);
        assert!(packed.cell.is_some(), "box region should stamp a cell");
        // No two argon atoms (different copies) sit below the tolerance.
        for i in 0..packed.atoms.len() {
            for j in (i + 1)..packed.atoms.len() {
                let d = (packed.atoms[i].position - packed.atoms[j].position).norm();
                assert!(d >= 2.0 - 0.25, "copies clash: {d:.3} Å");
            }
        }
    }

    #[test]
    fn requires_a_region() {
        let (mut state, _) = scratch_with_entry();
        let err = disorder_command(&mut state, &split("--of active --count 4")).unwrap_err();
        assert!(err.to_string().contains("region"));
    }

    #[test]
    fn outside_a_box_reports_a_clear_error() {
        let (mut state, _) = scratch_with_entry();
        let err = disorder_command(&mut state, &split("--of active --box 20,20,20 --outside"))
            .unwrap_err();
        assert!(err.to_string().contains("sphere or cylinder"), "got: {err}");
    }

    #[test]
    fn value_flag_does_not_swallow_the_next_flag() {
        let (mut state, _) = scratch_with_entry();
        // `--count` with no value must not consume `--box`.
        let err =
            disorder_command(&mut state, &split("--of active --count --box 10,10,10")).unwrap_err();
        assert!(err.to_string().contains("needs a value"), "got: {err}");
    }

    #[test]
    fn rejects_count_without_a_molecule() {
        let (mut state, _) = scratch_with_entry();
        let err = disorder_command(&mut state, &split("--count 5 --box 10,10,10")).unwrap_err();
        assert!(err.to_string().contains("must follow"));
    }

    #[test]
    fn pack_alias_round_trips_through_the_console() {
        let (mut state, _) = scratch_with_entry();
        let before = state.entries.records.len();
        let message = crate::frontend::console::execute_console_line(
            &mut state,
            "pack --of active --count 4 --box 14,14,14",
        )
        .unwrap();
        assert!(message.contains("Packed"), "got: {message}");
        assert_eq!(state.entries.records.len(), before + 1);
    }

    #[test]
    fn no_cell_flag_suppresses_the_cell() {
        let (mut state, _) = scratch_with_entry();
        disorder_command(
            &mut state,
            &split("--of active --count 4 --box 16,16,16 --no-cell"),
        )
        .unwrap();
        let packed = &state.entries.records.last().unwrap().structure;
        assert!(
            packed.cell.is_none(),
            "--no-cell should leave the cell unset"
        );
    }
}
