//! `forcefield type` / `forcefield list`: GAFF typing of one organic molecule
//! through the acpype engine, saved into the force-field library. Runs
//! synchronously, like `dock`, which suits headless `.sls`/CLI use.

use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::{Context, Result};

use crate::{
    backend::{
        engine_launch::{LaunchTarget, resolve_engine_launch},
        force_fields::{list_force_fields, save_force_field},
    },
    domain::Structure,
    engines::{
        acpype::{AcpypeRequest, AtomTypeSet, ChargeMethod, run_acpype},
        registry::EngineId,
    },
    frontend::{
        console::ForcefieldTypeArgs,
        entry_ref::{entry_structure, resolve_entry_id},
        state::AppState,
    },
};

pub fn type_command(state: &mut AppState, args: ForcefieldTypeArgs) -> Result<String> {
    let entry_id = resolve_entry_id(state, &args.entry)?;
    let structure = entry_structure(state, entry_id, "forcefield type")?;
    let entry_name = state
        .entries
        .entry(entry_id)
        .map(|entry| entry.name.clone())
        .unwrap_or_else(|| format!("entry-{entry_id}"));
    let (request, charge_note) = acpype_request(structure, &args)?;
    let launch = resolve_engine_launch(
        LaunchTarget::Local(&state.config.engine_overrides),
        EngineId::ACPYPE,
    )
    .context("acpype (AmberTools) is needed for force-field typing")?
    .launch;
    let cancel = Arc::new(AtomicBool::new(false));
    let outcome = run_acpype(request, launch, cancel, |_| {})
        .with_context(|| format!("could not type entry #{entry_id} ({entry_name})"))?;

    let library_name = args.name.clone().unwrap_or(entry_name.clone());
    save_force_field(&library_name, &outcome.itp)
        .with_context(|| format!("could not save force field `{library_name}`"))?;
    Ok(format!(
        "typed {entry_name} (entry #{entry_id}) in {:.0} s — {}{charge_note}; saved as force field \
         `{library_name}`",
        outcome.wall_time.as_secs_f64(),
        outcome.summary
    ))
}

/// The acpype request for `args`, and a note when an explicit `--charge`
/// overrides formal charges that sum to something else (e.g. a structure read
/// without them).
fn acpype_request(
    structure: Structure,
    args: &ForcefieldTypeArgs,
) -> Result<(AcpypeRequest, String)> {
    let formal: f32 = structure.atoms.iter().map(|atom| atom.charge).sum();
    let net_charge = args.charge.unwrap_or(formal.round() as i32);
    let note = if (formal - net_charge as f32).abs() > 0.5 {
        format!(
            " (net charge {net_charge:+} from --charge; the formal charges sum to {formal:+.1})"
        )
    } else {
        String::new()
    };
    let request = AcpypeRequest {
        structure,
        net_charge,
        charge_method: ChargeMethod::parse(&args.method)?,
        atom_types: AtomTypeSet::parse(&args.atom_types)?,
        max_duration: Duration::from_secs(args.timeout.max(1) * 60),
    };
    Ok((request, note))
}

pub fn list_command() -> String {
    let names = list_force_fields();
    if names.is_empty() {
        "no saved force fields".to_string()
    } else {
        format!("saved force fields: {}", names.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Bond, BondType};
    use crate::engines::acpype::render_args;
    use nalgebra::Point3;

    fn args(charge: Option<i32>) -> ForcefieldTypeArgs {
        ForcefieldTypeArgs {
            entry: "active".into(),
            charge,
            method: "bcc".into(),
            atom_types: "gaff2".into(),
            name: None,
            timeout: 30,
        }
    }

    fn uncharged_ammonium_fragment() -> Structure {
        let atom = |element: &str, x: f32| Atom {
            element: element.into(),
            position: Point3::new(x, 0.0, 0.0),
            charge: 0.0,
        };
        Structure::with_bonds(
            "methylammonium",
            vec![atom("C", 0.0), atom("N", 1.47)],
            vec![Bond::with_type(0, 1, BondType::Single)],
        )
    }

    #[test]
    fn explicit_charge_reaches_acpype_when_formal_charges_are_missing() {
        let (request, note) =
            acpype_request(uncharged_ammonium_fragment(), &args(Some(1))).unwrap();
        assert_eq!(request.net_charge, 1);
        assert!(render_args(&request).windows(2).any(|w| w == ["-n", "1"]));
        assert!(note.contains("--charge"), "{note}");
    }

    #[test]
    fn net_charge_defaults_to_the_formal_charge_sum() {
        let mut structure = uncharged_ammonium_fragment();
        structure.atoms[1].charge = 1.0;
        let (request, note) = acpype_request(structure, &args(None)).unwrap();
        assert_eq!(request.net_charge, 1);
        assert!(note.is_empty(), "{note}");
    }
}
