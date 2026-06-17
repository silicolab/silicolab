//! Command-line option parsing for the `md` subcommands: the [`Flags`] scanner
//! and the value parsers that turn tokens into typed protocol/system settings.

use anyhow::{Result, anyhow, bail};

use crate::workflows::molecular_dynamics::{
    WaterModel,
    run::{MdParameters, StageEdits, SystemTypeOverrides},
};

/// Parse a `--water` token into a [`WaterModel`].
pub fn parse_water_model(token: &str) -> Result<WaterModel> {
    match token.trim().to_ascii_lowercase().as_str() {
        "tip4p" => Ok(WaterModel::Tip4p),
        "tip4pew" => Ok(WaterModel::Tip4pEw),
        "tip3p" => Ok(WaterModel::Tip3p),
        "tip5p" => Ok(WaterModel::Tip5p),
        "tip5pe" => Ok(WaterModel::Tip5pEwald),
        "spc" => Ok(WaterModel::Spc),
        "spce" | "spc/e" => Ok(WaterModel::SpcE),
        other => bail!(
            "unknown water model `{other}` (expected tip4p, tip4pew, tip3p, tip5p, tip5pe, spc, or spce)"
        ),
    }
}

/// Pull `--key value` / `--flag` pairs from the argument list.
pub struct Flags {
    values: std::collections::BTreeMap<String, String>,
    flags: std::collections::BTreeSet<String>,
}

impl Flags {
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut values = std::collections::BTreeMap::new();
        let mut flags = std::collections::BTreeSet::new();
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            let Some(key) = arg.strip_prefix("--") else {
                bail!("unexpected argument `{arg}` (expected --key value)");
            };
            if let Some((k, v)) = key.split_once('=') {
                values.insert(k.to_string(), v.to_string());
                i += 1;
            } else if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                values.insert(key.to_string(), args[i + 1].clone());
                i += 2;
            } else {
                flags.insert(key.to_string());
                i += 1;
            }
        }
        Ok(Self { values, flags })
    }

    pub fn flag(&self, key: &str) -> bool {
        self.flags.contains(key)
    }

    pub fn f32(&self, key: &str) -> Result<Option<f32>> {
        self.values
            .get(key)
            .map(|v| {
                v.parse::<f32>()
                    .map_err(|_| anyhow!("invalid number for --{key}: {v}"))
            })
            .transpose()
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
}

/// Parse a time like `200ns`, `500ps`, or a bare number (picoseconds).
pub fn parse_time_ps(value: &str) -> Result<f64> {
    let v = value.trim().to_ascii_lowercase();
    if let Some(ns) = v.strip_suffix("ns") {
        Ok(ns.trim().parse::<f64>()? * 1000.0)
    } else if let Some(ps) = v.strip_suffix("ps") {
        Ok(ps.trim().parse::<f64>()?)
    } else {
        Ok(v.parse::<f64>()?)
    }
}

/// `--x` => `Some(true)`, `--no-x` => `Some(false)`, neither => `None`.
fn tri_state_flag(flags: &Flags, name: &str) -> Option<bool> {
    if flags.flag(name) {
        Some(true)
    } else if flags.flag(&format!("no-{name}")) {
        Some(false)
    } else {
        None
    }
}

/// The system-type overrides expressed on the command line.
pub fn parse_overrides(flags: &Flags) -> SystemTypeOverrides {
    SystemTypeOverrides {
        membrane: tri_state_flag(flags, "membrane"),
        ligand: tri_state_flag(flags, "ligand"),
        nucleic: tri_state_flag(flags, "nucleic"),
    }
}

/// Build per-stage edits from `--set`/`--raw`.
pub fn build_stage_edits(flags: &Flags) -> Result<StageEdits> {
    let mut edits = StageEdits::default();
    if let Some(set) = flags.str("set") {
        parse_set_into(&mut edits.params, set)?;
    }
    if let Some(raw) = flags.str("raw") {
        edits.raw_passthrough = parse_raw_lines(raw)?;
    }
    Ok(edits)
}

/// Parse `--set key=val,key=val` into tiered parameters (Standard/Advanced tiers).
pub fn parse_set_into(params: &mut MdParameters, set: &str) -> Result<()> {
    for pair in set.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| anyhow!("--set entry `{pair}` must be key=value"))?;
        let value = value.trim();
        match key.trim() {
            "coulomb_cutoff" => params.coulomb_cutoff_nm = Some(value.parse()?),
            "vdw_cutoff" => params.vdw_cutoff_nm = Some(value.parse()?),
            "thermostat_tau" => params.thermostat_tau_ps = Some(value.parse()?),
            "pme_spacing" => params.pme_spacing_nm = Some(value.parse()?),
            "pme_order" => params.pme_order = Some(value.parse()?),
            "lincs_order" => params.constraint_order = Some(value.parse()?),
            "lincs_iter" => params.constraint_iterations = Some(value.parse()?),
            "nstlist" => params.neighbor_list_steps = Some(value.parse()?),
            "seed" => params.random_seed = Some(value.parse()?),
            other => bail!(
                "unknown --set key `{other}` (try coulomb_cutoff, vdw_cutoff, thermostat_tau, \
                 pme_spacing, pme_order, lincs_order, lincs_iter, nstlist, seed)"
            ),
        }
    }
    Ok(())
}

/// Parse `--raw "key=val;key2=val2"` into verbatim `.mdp` passthrough lines.
pub fn parse_raw_lines(raw: &str) -> Result<Vec<(String, String)>> {
    let mut lines = Vec::new();
    for pair in raw.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| anyhow!("--raw entry `{pair}` must be key=value"))?;
        lines.push((key.trim().to_string(), value.trim().to_string()));
    }
    Ok(lines)
}
