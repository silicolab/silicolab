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
    let net_charge = args.charge.unwrap_or_else(|| {
        structure
            .atoms
            .iter()
            .map(|atom| atom.charge)
            .sum::<f32>()
            .round() as i32
    });
    let request = AcpypeRequest {
        structure,
        net_charge,
        charge_method: ChargeMethod::parse(&args.method)?,
        atom_types: AtomTypeSet::parse(&args.atom_types)?,
        max_duration: Duration::from_secs(args.timeout.max(1) * 60),
    };
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
        "typed {entry_name} (entry #{entry_id}) in {:.0} s — {}; saved as force field `{library_name}`",
        outcome.wall_time.as_secs_f64(),
        outcome.summary
    ))
}

pub fn list_command() -> String {
    let names = list_force_fields();
    if names.is_empty() {
        "no saved force fields".to_string()
    } else {
        format!("saved force fields: {}", names.join(", "))
    }
}
