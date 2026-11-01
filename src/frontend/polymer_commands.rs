//! `polymer build`: grow a linear chain from repeat-unit SMILES as a new entry.

use anyhow::{Context, Result};

use crate::{
    frontend::{console::PolymerBuildArgs, state::AppState},
    io::structure_io::default_structure_save_path,
    workflows::polymer::{PolymerSpec, build_polymer},
};

pub fn build_command(state: &mut AppState, args: PolymerBuildArgs) -> Result<String> {
    let title = args
        .name
        .clone()
        .unwrap_or_else(|| default_title(&args.units, args.dp));
    let spec = PolymerSpec {
        units: args.units.clone(),
        degree: args.dp,
        sequence: args.sequence.clone(),
        residue_name: args.residue.clone(),
        seed: args.seed,
        relax: !args.no_relax,
        title: title.clone(),
    };
    let result = build_polymer(&spec)
        .with_context(|| format!("could not build polymer from {}", args.units.join(" ")))?;
    let report = result.report;
    let save_path = default_structure_save_path(&result.structure, None);
    let entry_id = state.entries.add_entry(result.structure, None, save_path);
    state.show_entry(entry_id);

    let mut summary = format!(
        "built polymer {title} as entry #{entry_id} (dp {}, {} atoms, Rg {:.1} Å, end-to-end \
         {:.1} Å)",
        report.degree, report.atom_count, report.radius_of_gyration, report.end_to_end
    );
    if report.clashes > 0 {
        summary.push_str(&format!(
            "; {} close contacts remain — relax with `md simulate` or rebuild with another --seed",
            report.clashes
        ));
    }
    if report.relax_skipped {
        summary.push_str("; UFF could not relax some junctions (an element it lacks)");
    }
    Ok(summary)
}

fn default_title(units: &[String], degree: usize) -> String {
    match units {
        [unit] => format!("{unit} x{degree}"),
        _ => format!("copolymer x{degree}"),
    }
}
