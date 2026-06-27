use super::super::*;

/// Build the MD system with the engine the panel selected. Returns `true` once
/// the work is launched/finished and the panel may close; `false` leaves the
/// panel open with a reported reason so the user can adjust inputs and retry.
///
/// GROMACS is the default: it runs the full pdb2gmx pipeline on a worker thread
/// and produces a force-field topology a run can reuse. The built-in path is a
/// geometry-only fallback (box + solvation coordinates, no topology) that the
/// user can opt into explicitly.
pub(crate) fn build_md_system(
    state: &mut AppState,
    prompt: &crate::frontend::state::MdSystemPrompt,
) -> bool {
    use crate::frontend::state::MdBuildEngine;
    use crate::workflows::molecular_dynamics::is_framework_shape;
    match prompt.engine {
        MdBuildEngine::Gromacs => {
            // A covalent framework (celled, bonded, non-biopolymer) has no residue
            // template for pdb2gmx; generate its topology directly from the bonds
            // instead. The material build validates parameters and reports any
            // element the built-in tables and the custom force field don't cover.
            if is_framework_shape(state.structure()) {
                start_material_md_build(state, prompt)
            } else {
                start_gromacs_md_build(state, prompt)
            }
        }
        // The built-in geometry build is pure-Rust and always runs locally; it
        // ignores `prompt.prefs.target` (the panel hides the Run-on picker for the
        // built-in engine), so a Remote seed never routes a built-in build off-box.
        MdBuildEngine::BuiltIn => build_md_system_builtin(state, prompt),
    }
}

/// Out-of-plane cell length (A) the framework cell editor is seeded to when the
/// crystal's own gap is thinner. It clears a 1.0 nm cutoff plus the Verlet
/// buffer on both sides of the slab (`2·(1.0+0.1) nm + buffer`), so the default
/// box runs without the user having to widen `c` first.
pub(crate) const FRAMEWORK_C_FLOOR_ANGSTROM: f32 = 25.0;

/// Launch the framework (nanosheet) build: generate the topology from the
/// structure's bonds (rigid or flexible per the prompt), use the user-edited
/// crystal cell as the box, and optionally solvate. Writes `topol.top` and
/// `framework_run.json` into the build run directory for a later MD run.
pub(crate) fn start_material_md_build(
    state: &mut AppState,
    prompt: &crate::frontend::state::MdSystemPrompt,
) -> bool {
    use crate::engines::gromacs::MaterialBuildRequest;

    if state.jobs.engine_running() {
        state.set_message("another external engine job is already running".to_string());
        return false;
    }
    let run_dir = match ensure_active_task_run_dir(
        state,
        TaskKind::BuildMdSystem,
        Some(prompt.run_name.as_str()),
    ) {
        Ok(path) => path,
        Err(error) => {
            state.set_message(format!("failed to create run directory: {error}"));
            complete_active_task(state, TaskKind::BuildMdSystem, TaskStatus::Failed);
            return false;
        }
    };
    // The box is the user-edited crystal cell, preserving its (e.g. hexagonal)
    // shape. Falling back to the structure's own cell keeps non-GUI callers
    // working. When an explicit cell is supplied, build_material_system uses it
    // verbatim instead of opening the out-of-plane axis itself.
    let cell_override = prompt.framework_cell.map(|[a, b, c, alpha, beta, gamma]| {
        crate::domain::UnitCell::from_parameters(a, b, c, alpha, beta, gamma)
    });

    // A remote target relays the build to a deployed worker, which resolves `gmx`
    // on the node; the local arm runs `gmx` here.
    if let Some(host) = resolve_remote_host(state, &prompt.prefs.target) {
        state.ui.pending_optimization = None;
        let job = crate::workflows::gromacs::GromacsJob::BuildMaterial(
            crate::workflows::gromacs::GromacsMaterialRequest {
                structure: state.structure().clone(),
                mode: prompt.framework_mode,
                solvation: prompt.solvation_options(),
                custom_force_field: prompt.custom_force_field_text.clone(),
                cell_override,
                solvent_gap_angstrom: FRAMEWORK_C_FLOOR_ANGSTROM,
                cutoff_nm: crate::workflows::molecular_dynamics::DEFAULT_CUTOFF_NM,
                max_duration: Duration::from_secs(60 * 60),
            },
        );
        relay_gromacs_job(state, host, "GROMACS", job);
        return true;
    }

    let compute = match resolve_md_compute(state, crate::frontend::state::MdEngineChoice::Gromacs) {
        Ok(compute) => compute,
        Err(error) => {
            state.set_message(error.to_string());
            return false;
        }
    };
    if let Some(task_run_id) = state.active_task_run {
        mark_task_status(state, task_run_id, TaskStatus::Running);
        state
            .tasks
            .set_engine_label(task_run_id, Some("GROMACS".to_string()));
        sync_task_manifest(state, task_run_id);
    }

    state.ui.pending_optimization = None;
    let job = crate::frontend::jobs::spawn_material_build_job(MaterialBuildRequest {
        structure: state.structure().clone(),
        mode: prompt.framework_mode,
        working_dir: run_dir,
        compute,
        solvation: prompt.solvation_options(),
        cell_override,
        custom_force_field: prompt.custom_force_field_text.clone(),
        solvent_gap_angstrom: FRAMEWORK_C_FLOOR_ANGSTROM,
        cutoff_nm: crate::workflows::molecular_dynamics::DEFAULT_CUTOFF_NM,
        max_duration: Duration::from_secs(60 * 60),
    });
    state.jobs.set_engine(job);
    state.set_message("Building framework MD system; press Esc to stop".to_string());
    true
}

/// Launch the GROMACS pdb2gmx → editconf → solvate → genion pipeline as a
/// background engine job, writing its `topol.top` into the build run directory
/// so a later MD run can reuse it. On a setup error (engine missing, run
/// directory) it reports the reason and keeps the panel open.
pub(crate) fn start_gromacs_md_build(
    state: &mut AppState,
    prompt: &crate::frontend::state::MdSystemPrompt,
) -> bool {
    if state.jobs.engine_running() {
        state.set_message("another external engine job is already running".to_string());
        return false;
    }
    let run_dir = match ensure_active_task_run_dir(
        state,
        TaskKind::BuildMdSystem,
        Some(prompt.run_name.as_str()),
    ) {
        Ok(path) => path,
        Err(error) => {
            state.set_message(format!("failed to create run directory: {error}"));
            complete_active_task(state, TaskKind::BuildMdSystem, TaskStatus::Failed);
            return false;
        }
    };
    // Only attach an ion step when solvation is on and the user asked for ions;
    // genion needs the solvent it replaces.
    let ions = if prompt.solvate && (prompt.neutralize || prompt.add_salt) {
        Some(IonOptions {
            neutralize: prompt.neutralize,
            concentration_molar: prompt.add_salt.then_some(prompt.salt_concentration_molar),
            positive_ion: prompt.positive_ion.clone(),
            negative_ion: prompt.negative_ion.clone(),
        })
    } else {
        None
    };

    // A remote target relays the build to a deployed worker, which resolves `gmx`
    // on the node; the local arm runs `gmx` here.
    if let Some(host) = resolve_remote_host(state, &prompt.prefs.target) {
        state.ui.pending_optimization = None;
        let job = crate::workflows::gromacs::GromacsJob::Build(
            crate::workflows::gromacs::GromacsBuildRequest {
                structure: state.structure().clone(),
                force_field: prompt.force_field.clone(),
                water: prompt.water,
                box_config: prompt.config(),
                solvate: prompt.solvate,
                ions,
                max_duration: Duration::from_secs(60 * 60),
            },
        );
        relay_gromacs_job(state, host, "GROMACS", job);
        return true;
    }

    // GROMACS is required for this build; we never silently fall back to a
    // topology-less geometry build.
    let compute = match resolve_md_compute(state, crate::frontend::state::MdEngineChoice::Gromacs) {
        Ok(compute) => compute,
        Err(error) => {
            state.set_message(error.to_string());
            return false;
        }
    };
    if let Some(task_run_id) = state.active_task_run {
        mark_task_status(state, task_run_id, TaskStatus::Running);
        state
            .tasks
            .set_engine_label(task_run_id, Some("GROMACS".to_string()));
        sync_task_manifest(state, task_run_id);
    }

    state.ui.pending_optimization = None;
    let job = spawn_gromacs_build_job(BuildRequest {
        structure: state.structure().clone(),
        working_dir: run_dir,
        compute,
        force_field: prompt.force_field.clone(),
        water: prompt.water,
        box_config: prompt.config(),
        solvate: prompt.solvate,
        ions,
        max_duration: Duration::from_secs(60 * 60),
    });
    state.jobs.set_engine(job);
    state.set_message("GROMACS building MD system; press Esc to stop".to_string());
    true
}

/// Returns `true` on success; on failure it reports the reason and leaves the
/// panel open so the user can adjust inputs and retry.
pub(crate) fn build_md_system_builtin(
    state: &mut AppState,
    prompt: &crate::frontend::state::MdSystemPrompt,
) -> bool {
    // The pre-box solute carries any residue metadata system-type detection reads.
    let solute = state.structure().clone();
    let config = prompt.config();
    let result = crate::workflows::molecular_dynamics::build_md_system(state.structure(), &config);
    let (boxed, report) = match result {
        Ok(value) => value,
        Err(error) => {
            state.set_message(format!("MD system build failed: {error}"));
            return false;
        }
    };

    // Optionally fill the freshly built box with water and ions — geometry only,
    // no force field (an engine parameterizes the system later). On failure keep
    // the panel open so the user can adjust the box or solvation settings.
    let solvated = match prompt.solvation_options() {
        Some(options) => match crate::workflows::molecular_dynamics::solvate(&boxed, &options) {
            Ok(out) => Some(out),
            Err(error) => {
                state.set_message(format!("MD system solvation failed: {error}"));
                return false;
            }
        },
        None => None,
    };

    let run_dir = match ensure_active_task_run_dir(
        state,
        TaskKind::BuildMdSystem,
        Some(prompt.run_name.as_str()),
    ) {
        Ok(path) => path,
        Err(error) => {
            state.set_message(format!("failed to create run directory: {error}"));
            complete_active_task(state, TaskKind::BuildMdSystem, TaskStatus::Failed);
            return false;
        }
    };
    if let Some(task_run_id) = state.active_task_run {
        mark_task_status(state, task_run_id, TaskStatus::Running);
    }
    state.cancel_transient_jobs();
    state.ui.pending_optimization = None;
    state.ui.editor = None;
    state.ui.selection.clear();

    // The entry structure is the solvated system when solvation ran, else the
    // bare box.
    let (final_structure, solvation_note) = match solvated {
        Some((structure, report)) => (
            structure,
            format!(
                "; solvated +{} water, +{} {}, +{} {}",
                report.water_added,
                report.cations_added,
                prompt.positive_ion,
                report.anions_added,
                prompt.negative_ion,
            ),
        ),
        None => (boxed, String::new()),
    };
    let final_atom_count = final_structure.atoms.len();
    let save_path = structure_io::default_structure_save_path(&final_structure, None);
    let entry_id = add_and_show_entry(state, final_structure, None, save_path);
    if let Some(task_run_id) = state.active_task_run {
        record_task_result_entry(state, task_run_id, entry_id);
    }
    // A built-in build is geometry-only — no force field is applied — so the
    // context records the generic family (a later run uses the captured
    // engine-neutral topology / cutoff path, not a biomolecular nonbonded block).
    let water_token = prompt.solvate.then(|| prompt.water.db_token().to_string());
    write_md_system_context(
        &run_dir,
        &solute,
        final_atom_count,
        "builtin",
        water_token.as_deref(),
        false,
        0.0,
        false,
        Vec::new(),
    );

    let [a, b, c] = report.edges_angstrom;
    let replaced = if report.replaced_existing_cell {
        " (replaced existing cell)"
    } else {
        ""
    };
    state.set_message(format!(
        "Built MD system: {a:.1} x {b:.1} x {c:.1} A box, {} atoms{replaced}{solvation_note}",
        state.structure().atoms.len()
    ));
    complete_active_task(state, TaskKind::BuildMdSystem, TaskStatus::Completed);
    true
}
