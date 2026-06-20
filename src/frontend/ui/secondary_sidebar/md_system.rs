use super::*;

/// Cations and anions offered in the System Builder. Restricted to monovalent
/// ions, matching the solvation logic's monovalent neutralization arithmetic.
/// Selectable ions as `(token, label)`: `token` is the GROMACS residue name
/// genion expects (`-pname`/`-nname`), `label` is the conventional chemical
/// form shown to the user.
pub(crate) const MD_POSITIVE_IONS: &[(&str, &str)] = &[("NA", "Na+"), ("K", "K+")];

pub(crate) const MD_NEGATIVE_IONS: &[(&str, &str)] = &[("CL", "Cl-")];

/// The display label for an ion token, falling back to the token itself.
pub(crate) fn ion_label(token: &str) -> &str {
    MD_POSITIVE_IONS
        .iter()
        .chain(MD_NEGATIVE_IONS)
        .find(|(value, _)| *value == token)
        .map(|(_, label)| *label)
        .unwrap_or(token)
}

pub(crate) fn render_md_system_task_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    use crate::{
        frontend::state::MdSystemSizingMode,
        workflows::molecular_dynamics::{self, BoxShape as MdBoxShape, WaterModel},
    };

    let atom_count = state.structure().atoms.len();
    let has_cell = state.structure().cell.is_some();
    // A periodic framework (nanosheet or other celled, bonded, non-biopolymer
    // structure) is routed to the bond-derived topology path instead of pdb2gmx.
    // Coverage depends on the selected custom force field, so snapshot the custom
    // atom types (from the cached `.itp` text) before borrowing the prompt mutably.
    let is_framework = molecular_dynamics::is_framework_shape(state.structure());
    let custom_types = state
        .ui
        .pending_md_system
        .as_ref()
        .and_then(|p| p.custom_force_field_text.as_deref())
        .map(crate::engines::gromacs::custom_ff::custom_types)
        .unwrap_or_default();
    let framework_flexible_ok =
        is_framework && molecular_dynamics::supports_flexible(state.structure());
    let framework_coverage = if is_framework {
        molecular_dynamics::framework_coverage(state.structure())
    } else {
        None
    };
    // Elements covered only by the user's force field, and any still uncovered.
    let user_provided_elements = if is_framework {
        molecular_dynamics::user_provided_elements(state.structure(), &custom_types)
    } else {
        Vec::new()
    };
    let unparameterized_elements = if is_framework {
        molecular_dynamics::unparameterized_elements(state.structure(), &custom_types)
    } else {
        Vec::new()
    };
    // Saved custom force fields available to pick, and the structure's own crystal
    // cell parameters for the cell editor's "reset" — both snapshotted up front.
    let available_force_fields = if is_framework {
        crate::backend::force_fields::list_force_fields()
    } else {
        Vec::new()
    };
    let framework_crystal_cell = if is_framework {
        state
            .structure()
            .cell
            .as_ref()
            .map(|c| [c.a, c.b, c.c, c.alpha, c.beta, c.gamma])
    } else {
        None
    };
    // Snapshot the previews out before borrowing the prompt mutably. The box
    // preview is cheap; the solvation preview is cached and only recomputed when
    // its inputs change (see `md_solvation_preview`).
    let preview = state.ui.pending_md_system.as_ref().map(|prompt| {
        crate::workflows::molecular_dynamics::preview(state.structure(), &prompt.config())
    });
    let solvation_preview = md_solvation_preview(state);
    // Computed before the prompt borrow so the target picker can list hosts.
    let hosts = remote_host_options(state);

    if let Some(prompt) = &mut state.ui.pending_md_system {
        // The bond-derived material path (which keeps the crystal cell as the box)
        // is taken only for a framework built with GROMACS; the built-in geometry
        // path re-boxes like any other structure.
        let framework_build =
            is_framework && prompt.engine == crate::frontend::state::MdBuildEngine::Gromacs;
        ui.label(format!("{atom_count} atoms"));
        if framework_build {
            ui.label(
                "Periodic framework: the crystal cell becomes the simulation box, keeping its \
                 shape. Adjust the lattice below — typically only the out-of-plane axis, to open \
                 a vacuum gap or a solvent column.",
            );
        } else {
            if has_cell {
                ui.colored_label(
                    egui::Color32::from_rgb(0xd0, 0x90, 0x30),
                    "This structure already has a cell; building will replace it.",
                );
            }
            ui.label("Wrap the molecule in a periodic box, then optionally solvate.");
        }
        ui.separator();

        // ---- Run name ----------------------------------------------------
        run_name_field(ui, &mut prompt.run_name);
        ui.separator();

        // ---- Build engine ------------------------------------------------
        use crate::frontend::state::MdBuildEngine;
        ui.strong("Build engine");
        ui.horizontal(|ui| {
            ui.label("Engine:");
            egui::ComboBox::from_id_salt("md_build_engine")
                .selected_text(prompt.engine.label())
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    for engine in MdBuildEngine::all() {
                        ui.selectable_value(&mut prompt.engine, *engine, engine.label());
                    }
                });
        });
        match prompt.engine {
            MdBuildEngine::Gromacs if is_framework => {
                use crate::workflows::molecular_dynamics::{Coverage, FrameworkMode};
                let amber = egui::Color32::from_rgb(0xd0, 0x90, 0x30);
                ui.label(
                    "Periodic framework: the topology is generated from the structure's bonds.",
                );
                if !framework_flexible_ok {
                    // No bonded parameters for this chemistry; rigid is the only option.
                    prompt.framework_mode = FrameworkMode::Rigid;
                }
                ui.horizontal(|ui| {
                    ui.label("Model:");
                    ui.radio_value(
                        &mut prompt.framework_mode,
                        FrameworkMode::Rigid,
                        "Rigid (frozen)",
                    );
                    ui.add_enabled_ui(framework_flexible_ok, |ui| {
                        ui.radio_value(
                            &mut prompt.framework_mode,
                            FrameworkMode::Flexible,
                            "Flexible (bonded)",
                        );
                    });
                });
                match prompt.framework_mode {
                    FrameworkMode::Rigid => {
                        ui.label("The sheet is frozen; only the surrounding system moves.");
                    }
                    FrameworkMode::Flexible => {
                        ui.label("The sheet flexes via bonds, angles and dihedrals.");
                    }
                }
                if !framework_flexible_ok {
                    ui.label(
                        "Flexible modeling needs carbon-family bonded parameters; only rigid is available for this material.",
                    );
                }
                // Standard biomolecular force fields don't parameterize these
                // materials, so SilicoLab supplies its own: validated OPLS-AA for
                // carbon, generic UFF otherwise. The flag grades those parameters.
                match framework_coverage {
                    Some(Coverage::Good) => {
                        ui.label(
                            "Parameters: OPLS-AA aromatic carbon (validated for carbon \
                             nanostructures).",
                        );
                    }
                    Some(Coverage::Approximate) => {
                        ui.colored_label(
                            amber,
                            "Parameters: generic UFF — no validated force field exists for this \
                             material, so results are approximate.",
                        );
                    }
                    Some(Coverage::Poor) => {
                        ui.colored_label(
                            amber,
                            "Parameters: generic UFF on transition-metal chemistry it was not \
                             designed for — treat results as qualitative.",
                        );
                    }
                    None => {}
                }

                // ---- Custom force field --------------------------------------
                // Cover elements the built-in tables lack, or override built-in
                // atom types, with a user-supplied GROMACS parameter block.
                ui.separator();
                ui.strong("Custom force field");
                ui.horizontal(|ui| {
                    ui.label("Use:");
                    let current = prompt.custom_force_field.clone();
                    egui::ComboBox::from_id_salt("md_custom_ff")
                        .selected_text(
                            current
                                .clone()
                                .unwrap_or_else(|| "(built-in only)".to_string()),
                        )
                        .show_ui(ui, |ui| {
                            crate::frontend::theme::stabilize_selectable_rows(ui);
                            if ui
                                .selectable_label(current.is_none(), "(built-in only)")
                                .clicked()
                            {
                                actions.push(AppAction::SelectCustomForceField(None));
                            }
                            for name in &available_force_fields {
                                let selected = current.as_deref() == Some(name.as_str());
                                if ui.selectable_label(selected, name).clicked() {
                                    actions.push(AppAction::SelectCustomForceField(Some(
                                        name.clone(),
                                    )));
                                }
                            }
                        });
                });
                if !unparameterized_elements.is_empty() {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        format!(
                            "No parameters for: {}. Add a custom force field that defines an atom \
                             type named after each element.",
                            unparameterized_elements.join(", ")
                        ),
                    );
                }
                if !user_provided_elements.is_empty() {
                    ui.colored_label(
                        amber,
                        format!(
                            "Using your custom force field for: {}.",
                            user_provided_elements.join(", ")
                        ),
                    );
                }
                ui.collapsing("Add / import a force field", |ui| {
                    ui.label(
                        "Paste a GROMACS [ atomtypes ] block (and optional [ bondtypes ] …). Name \
                         each atom type after its element symbol (e.g. Pt), or after a built-in \
                         type (CJ, HJ, Mo, …) to override it. Omit [ defaults ].",
                    );
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut prompt.custom_ff_draft_name);
                    });
                    ui.add(
                        egui::TextEdit::multiline(&mut prompt.custom_ff_draft)
                            .code_editor()
                            .desired_rows(5)
                            .desired_width(f32::INFINITY)
                            .hint_text("[ atomtypes ]\nPt  78  195.08  0.0  A  0.2754  0.33"),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Import file…").clicked() {
                            actions.push(AppAction::ImportCustomForceFieldFile);
                        }
                        if ui.button("Save to library").clicked() {
                            actions.push(AppAction::SaveCustomForceField);
                        }
                    });
                    if let Some(name) = prompt.custom_force_field.clone()
                        && ui.button(format!("Delete \"{name}\"")).clicked()
                    {
                        actions.push(AppAction::DeleteCustomForceField(name));
                    }
                });
            }
            MdBuildEngine::Gromacs => {
                ui.label(
                    "GROMACS assigns the force field and writes a topology the MD run reuses.",
                );
            }
            MdBuildEngine::BuiltIn => {
                ui.colored_label(
                    egui::Color32::from_rgb(0xd0, 0x90, 0x30),
                    "Geometry only: no topology is produced, so the MD run needs a custom topology.",
                );
            }
        }

        // ---- Box / simulation cell ---------------------------------------
        if let (true, Some(cell)) = (framework_build, prompt.framework_cell.as_mut()) {
            // The crystal cell is the box; expose its lattice parameters directly
            // so the shape (e.g. hexagonal) is preserved and editable, rather than
            // forcing one of the generic cuboid box shapes.
            ui.strong("Simulation cell");
            ui.label(format!("Shape: {}", cell_shape_label(cell)));
            egui::Grid::new("md_framework_cell")
                .num_columns(2)
                .show(ui, |ui| {
                    for (label, idx) in [("a (A):", 0), ("b (A):", 1), ("c (A):", 2)] {
                        ui.label(label);
                        md_length_value(ui, &mut cell[idx]);
                        ui.end_row();
                    }
                    for (label, idx) in
                        [("alpha (deg):", 3), ("beta (deg):", 4), ("gamma (deg):", 5)]
                    {
                        ui.label(label);
                        ui.add(
                            egui::DragValue::new(&mut cell[idx])
                                .range(1.0..=179.0)
                                .speed(0.1)
                                .fixed_decimals(1),
                        );
                        ui.end_row();
                    }
                });
            ui.label(
                "Taken from the crystal cell. The in-plane lattice (a, b, gamma) tiles the sheet \
                 across the boundary — usually leave it; widen c for vacuum or solvent.",
            );
            if let Some(crystal) = framework_crystal_cell
                && ui.button("Reset c to crystal cell").clicked()
            {
                cell[2] = crystal[2];
            }
        } else {
            ui.strong("Box");
            ui.label("Box shape:");
            ui.horizontal(|ui| {
                for shape in MdBoxShape::selectable() {
                    ui.radio_value(&mut prompt.shape, *shape, shape.label());
                }
            });

            ui.label("Sizing:");
            ui.horizontal(|ui| {
                ui.radio_value(&mut prompt.mode, MdSystemSizingMode::Padding, "Padding");
                ui.radio_value(&mut prompt.mode, MdSystemSizingMode::Absolute, "Absolute");
            });

            match prompt.mode {
                MdSystemSizingMode::Padding => {
                    egui::Grid::new("md_system_padding")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Padding X (A):");
                            md_length_value(ui, &mut prompt.padding_angstrom[0]);
                            ui.end_row();
                            ui.label("Padding Y (A):");
                            md_length_value(ui, &mut prompt.padding_angstrom[1]);
                            ui.end_row();
                            ui.label("Padding Z (A):");
                            md_length_value(ui, &mut prompt.padding_angstrom[2]);
                            ui.end_row();
                        });
                    ui.label(
                        "Default 10 A (= 1.0 nm) keeps the box above GROMACS' 1.0 nm cutoffs.",
                    );
                }
                MdSystemSizingMode::Absolute => {
                    egui::Grid::new("md_system_absolute")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Edge a (A):");
                            md_length_value(ui, &mut prompt.absolute_angstrom[0]);
                            ui.end_row();
                            ui.label("Edge b (A):");
                            md_length_value(ui, &mut prompt.absolute_angstrom[1]);
                            ui.end_row();
                            ui.label("Edge c (A):");
                            md_length_value(ui, &mut prompt.absolute_angstrom[2]);
                            ui.end_row();
                        });
                }
            }

            if prompt.shape == MdBoxShape::Cubic {
                ui.label("Cubic: the largest edge is applied to all three axes.");
            }
        }

        // ---- Force field -------------------------------------------------
        // Only the GROMACS pdb2gmx path assigns a bundled force field; the
        // built-in build is geometry only, and a framework uses its own
        // bond-derived parameters — both ignore this selection.
        if prompt.engine == MdBuildEngine::Gromacs && !is_framework {
            ui.separator();
            ui.strong("Force field");
            ui.horizontal(|ui| {
                ui.label("Force field:");
                egui::ComboBox::from_id_salt("md_force_field")
                    .selected_text(molecular_dynamics::force_field_title(&prompt.force_field))
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for entry in molecular_dynamics::FORCE_FIELDS {
                            ui.selectable_value(
                                &mut prompt.force_field,
                                entry.token.to_string(),
                                entry.title,
                            );
                        }
                    });
            });
        }

        // ---- Solvent -----------------------------------------------------
        ui.separator();
        ui.strong("Solvent");
        ui.checkbox(&mut prompt.solvate, "Solvate system (add explicit water)");
        if prompt.solvate {
            ui.horizontal(|ui| {
                ui.label("Water model:");
                egui::ComboBox::from_id_salt("md_water_model")
                    .selected_text(prompt.water.label())
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for model in WaterModel::all() {
                            ui.selectable_value(&mut prompt.water, *model, model.label());
                        }
                    });
            });

            // ---- Ions ----------------------------------------------------
            ui.separator();
            ui.strong("Ions");
            ui.checkbox(&mut prompt.neutralize, "Neutralize net charge");
            ui.checkbox(&mut prompt.add_salt, "Add salt bath");
            if prompt.add_salt {
                ui.horizontal(|ui| {
                    ui.label("Concentration (mol/L):");
                    ui.add(
                        egui::DragValue::new(&mut prompt.salt_concentration_molar)
                            .range(0.0..=5.0)
                            .speed(0.01)
                            .fixed_decimals(2),
                    );
                });
            }
            if prompt.neutralize || prompt.add_salt {
                ui.horizontal(|ui| {
                    ui.label("Cation:");
                    md_ion_combo("md_cation", &mut prompt.positive_ion, MD_POSITIVE_IONS, ui);
                    ui.label("Anion:");
                    md_ion_combo("md_anion", &mut prompt.negative_ion, MD_NEGATIVE_IONS, ui);
                });
            }
        }

        // ---- Preview -----------------------------------------------------
        ui.separator();
        if let (true, Some(cell)) = (framework_build, prompt.framework_cell) {
            // The framework box is the (edited) crystal cell; report it directly
            // and check it clears the nonbonded cutoff's minimum image — the most
            // common framework build failure — instead of a generic padded box.
            let [a, b, c, _, _, _] = cell;
            ui.label(format!(
                "Box: {a:.1} x {b:.1} x {c:.1} A ({})",
                cell_shape_label(&cell)
            ));
            let unit_cell = crate::domain::UnitCell::from_parameters(
                cell[0], cell[1], cell[2], cell[3], cell[4], cell[5],
            );
            if let Err(error) = crate::workflows::molecular_dynamics::ensure_periodic_cutoff_fits(
                &unit_cell,
                crate::workflows::molecular_dynamics::DEFAULT_CUTOFF_NM,
            ) {
                ui.colored_label(egui::Color32::LIGHT_RED, error.to_string());
            }
            if prompt.solvate {
                // The real water count comes from gmx solvate at build time; the
                // geometric estimate is computed against a padded box, so it does
                // not apply here.
                ui.label("Solvent water count is determined during the build.");
            }
        } else {
            match preview.flatten() {
                Some(preview) => {
                    let [a, b, c] = preview.edges_angstrom;
                    ui.label(format!("Resulting box: {a:.1} x {b:.1} x {c:.1} A"));
                    if !preview.fits {
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            "Box is smaller than the molecule; build will fail.",
                        );
                    }
                }
                None => {
                    ui.colored_label(egui::Color32::LIGHT_RED, "No atoms to box.");
                }
            }
            if prompt.solvate {
                match &solvation_preview {
                    Some(Ok(est)) => {
                        ui.label(format!(
                            "~ {} waters, +{} {}, +{} {}",
                            est.water,
                            est.cations,
                            ion_label(&prompt.positive_ion),
                            est.anions,
                            ion_label(&prompt.negative_ion)
                        ));
                    }
                    Some(Err(error)) => {
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            format!("Solvation preview unavailable: {error}"),
                        );
                    }
                    None => {}
                }
            }
        }

        // ---- Actions -----------------------------------------------------
        ui.separator();
        // The execution target sits right above the Build button (they're closely
        // related). Only the GROMACS build runs external tools that can go remote;
        // the built-in path is pure-Rust geometry that always runs locally.
        if prompt.engine == MdBuildEngine::Gromacs {
            compute_target_picker(ui, &mut prompt.target, &hosts, actions);
        }
        ui.horizontal(|ui| {
            let build_label = if prompt.solvate {
                "Build & Solvate"
            } else {
                "Build"
            };
            if ui
                .button(format!("{}  {build_label}", egui_phosphor::regular::CUBE))
                .clicked()
            {
                actions.push(AppAction::ConfirmMdSystem);
            }
            if ui
                .button(format!("{}  Cancel", egui_phosphor::regular::X))
                .clicked()
            {
                actions.push(AppAction::CancelMdSystemPrompt);
            }
        });
    } else {
        ui.label("MD system panel is unavailable.");
    }
}

/// A combo box for choosing an ion name from a fixed list.
pub(crate) fn md_ion_combo(
    id: &str,
    value: &mut String,
    options: &[(&str, &str)],
    ui: &mut egui::Ui,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(ion_label(value))
        .show_ui(ui, |ui| {
            crate::frontend::theme::stabilize_selectable_rows(ui);
            for (token, label) in options {
                ui.selectable_value(value, (*token).to_string(), *label);
            }
        });
}

/// Compute — or reuse the cached — solvation count preview for the System
/// Builder panel. Returns `None` when there is no prompt or solvation is off.
/// Recomputing grid-fills the box, so the result is cached and only refreshed
/// when the inputs that affect it change.
pub(crate) fn md_solvation_preview(
    state: &mut AppState,
) -> Option<Result<crate::workflows::molecular_dynamics::SolvationEstimate, String>> {
    let (config, options) = {
        let prompt = state.ui.pending_md_system.as_ref()?;
        let options = prompt.solvation_options()?;
        (prompt.config(), options)
    };
    let key = md_solvation_estimate_key(state.structure(), &config, &options);
    if state.ui.md_solvation_preview_key == key
        && let Some(cached) = &state.ui.md_solvation_preview
    {
        return Some(cached.clone());
    }
    let result = md_compute_solvation_estimate(state.structure(), &config, &options);
    state.ui.md_solvation_preview = Some(result.clone());
    state.ui.md_solvation_preview_key = key;
    Some(result)
}

pub(crate) fn md_compute_solvation_estimate(
    solute: &crate::domain::Structure,
    config: &crate::workflows::molecular_dynamics::MdSystemConfig,
    options: &crate::workflows::molecular_dynamics::SolvationOptions,
) -> Result<crate::workflows::molecular_dynamics::SolvationEstimate, String> {
    use crate::workflows::molecular_dynamics::{build_md_system, estimate};
    // Preview against the box the build would actually produce.
    let (boxed, _) = build_md_system(solute, config).map_err(|e| e.to_string())?;
    estimate(&boxed, options).map_err(|e| e.to_string())
}

pub(crate) fn md_solvation_estimate_key(
    solute: &crate::domain::Structure,
    config: &crate::workflows::molecular_dynamics::MdSystemConfig,
    options: &crate::workflows::molecular_dynamics::SolvationOptions,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    solute.atoms.len().hash(&mut hasher);
    if let Some(edges) = crate::workflows::molecular_dynamics::preview_edges(solute, config) {
        for edge in edges {
            edge.to_bits().hash(&mut hasher);
        }
    }
    options.water.db_token().hash(&mut hasher);
    options.positive_ion.hash(&mut hasher);
    options.negative_ion.hash(&mut hasher);
    options.neutralize.hash(&mut hasher);
    options
        .concentration_molar
        .map(f32::to_bits)
        .hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn md_length_value(ui: &mut egui::Ui, value: &mut f32) {
    ui.add(
        egui::DragValue::new(value)
            .range(0.0..=10_000.0)
            .speed(0.1)
            .fixed_decimals(1),
    );
}

/// A coarse lattice-system label for the framework cell editor readout, so the
/// user can confirm the box matches their material. `cell` is `[a, b, c, α, β,
/// γ]` (lengths in A, angles in degrees).
pub(crate) fn cell_shape_label(cell: &[f32; 6]) -> &'static str {
    let [a, b, c, alpha, beta, gamma] = *cell;
    let ang = |x: f32, target: f32| (x - target).abs() < 0.5;
    let len = |x: f32, y: f32| (x - y).abs() < 0.01 * x.max(y).max(1.0);
    if ang(alpha, 90.0) && ang(beta, 90.0) && (ang(gamma, 120.0) || ang(gamma, 60.0)) {
        "hexagonal"
    } else if ang(alpha, 90.0) && ang(beta, 90.0) && ang(gamma, 90.0) {
        if len(a, b) && len(b, c) {
            "cubic"
        } else if len(a, b) {
            "tetragonal"
        } else {
            "orthorhombic"
        }
    } else {
        "triclinic"
    }
}

pub(crate) fn supercell_repeat_value(ui: &mut egui::Ui, value: &mut u32) {
    ui.add_sized(
        [52.0, 20.0],
        egui::DragValue::new(value).range(1..=10).speed(0.1),
    );
}

/// Render the editable run-name field shared by directory-creating task panels.
/// This name is purely human-facing and becomes the run directory's name; the
/// task's durable identity is a separate UUID, so renaming is always safe.
pub(crate) fn run_name_field(ui: &mut egui::Ui, run_name: &mut String) {
    ui.horizontal(|ui| {
        ui.label("Run name:");
        ui.add(
            egui::TextEdit::singleline(run_name)
                .hint_text("auto")
                .desired_width(200.0),
        );
    });
}

/// The per-task execution-target dropdown ("This machine" + each configured
/// remote host) plus an always-visible "Add host…" button that opens the Remote
/// Hosts settings, so users can discover how to configure a host without already
/// knowing where it lives. Mutates `target` in place (the prompt owns it);
/// `actions` carries the open-settings request. `hosts` is `(id, label)` for
/// every configured remote host.
pub(crate) fn compute_target_picker(
    ui: &mut egui::Ui,
    target: &mut crate::backend::config::ComputeTarget,
    hosts: &[(String, String)],
    actions: &mut Vec<AppAction>,
) {
    use crate::backend::config::ComputeTarget;
    let selected = match target {
        ComputeTarget::Local => "This machine".to_string(),
        ComputeTarget::Remote(id) => hosts
            .iter()
            .find(|(host_id, _)| host_id == id)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| "(unconfigured host)".to_string()),
    };
    ui.horizontal(|ui| {
        ui.label("Run on:");
        egui::ComboBox::from_id_salt("compute_target")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
                ui.selectable_value(target, ComputeTarget::Local, "This machine");
                for (id, label) in hosts {
                    ui.selectable_value(target, ComputeTarget::Remote(id.clone()), label);
                }
            });
        if ui
            .button(format!("{}  Add host…", egui_phosphor::regular::PLUS))
            .on_hover_text("Add or manage remote hosts in Settings")
            .clicked()
        {
            actions.push(AppAction::OpenRemoteHostsSettings);
        }
    });
}

/// `(id, label)` for every configured remote host, for the target picker.
pub(crate) fn remote_host_options(state: &AppState) -> Vec<(String, String)> {
    let mut hosts: Vec<(String, String)> = state
        .config
        .remote_hosts
        .values()
        .map(|host| (host.id.clone(), host.label.clone()))
        .collect();
    hosts.sort_by_key(|host| host.1.to_lowercase());
    hosts
}
