//! The QM task-panel forms: the molecular HF/DFT form (method, basis, dispersion,
//! charge/spin, calculation kind, and advanced options), the transition-state
//! search form, the periodic (crystalline) form, and the memory-estimate readout.
//! Rendered by [`super::render_qm_task_panel`]; split out of `task_panels` to keep
//! that file within the per-file size budget.

use super::*;

/// The molecular QM form: method, basis, dispersion, charge/spin, the calculation
/// kind, the properties toggle, and the advanced options.
pub(crate) fn render_molecular_qm_form(
    ui: &mut egui::Ui,
    prompt: &mut crate::frontend::state::QmPrompt,
    entry_options: &[(u64, String)],
) {
    use crate::engines::qm::{QmDispersion, QmKind, QmMethod, supports_dispersion};

    let presets = QmMethod::presets();

    egui::Grid::new("sidebar_qm_options")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Method:");
            let is_custom = !presets.iter().any(|m| m == &prompt.method);
            egui::ComboBox::from_id_salt("qm_method")
                .selected_text(if is_custom {
                    format!("Custom: {}", prompt.method.label())
                } else {
                    prompt.method.label()
                })
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    for method in &presets {
                        let label = method.label();
                        ui.selectable_value(&mut prompt.method, method.clone(), label);
                    }
                    if ui
                        .selectable_label(is_custom, "Custom functional…")
                        .clicked()
                    {
                        let name = if prompt.custom_functional.trim().is_empty() {
                            "pbe".to_string()
                        } else {
                            prompt.custom_functional.clone()
                        };
                        prompt.custom_functional = name.clone();
                        prompt.method = QmMethod::Dft(name);
                    }
                });
            ui.end_row();
            // Read live, after the dropdown, so the rows below react to a method
            // change in the same frame it is made.
            let method_is_composite = matches!(prompt.method, QmMethod::Composite(_));

            // Free-text functional name, shown when the method is a DFT
            // functional outside the preset list.
            if !presets.iter().any(|m| m == &prompt.method)
                && matches!(prompt.method, QmMethod::Dft(_))
            {
                ui.label("Functional:");
                if ui
                    .text_edit_singleline(&mut prompt.custom_functional)
                    .changed()
                {
                    prompt.method = QmMethod::Dft(prompt.custom_functional.clone());
                }
                ui.end_row();
            }

            ui.label("Basis set:");
            // A composite carries its own implied basis.
            if method_is_composite {
                ui.label("(implied by composite)");
            } else {
                egui::ComboBox::from_id_salt("qm_basis")
                    .selected_text(prompt.basis.clone())
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        // The orbital basis sets hartree accepts, kept in sync via
                        // the engine constant (and its drift-guard test).
                        for basis in crate::engines::qm::QM_BASIS_SETS {
                            ui.selectable_value(&mut prompt.basis, basis.to_string(), *basis);
                        }
                    });
            }
            ui.end_row();

            // Dispersion is a primary level-of-theory choice (the default is
            // B3LYP-D3(BJ)), so it lives here, not under Advanced. Offer only the
            // variants hartree parametrizes for the chosen functional, and drop a
            // stale value the new method can't carry — otherwise the run (and the
            // memory estimate) would bail. Composites (own dispersion) and post-HF
            // (none) report no support, so the row hides and any value is cleared.
            let d3_ok = supports_dispersion(&prompt.method, QmDispersion::D3Bj);
            let d4_ok = supports_dispersion(&prompt.method, QmDispersion::D4);
            match prompt.options.dispersion {
                Some(QmDispersion::D3Bj) if !d3_ok => prompt.options.dispersion = None,
                Some(QmDispersion::D4) if !d4_ok => prompt.options.dispersion = None,
                _ => {}
            }
            if d3_ok || d4_ok {
                ui.label("Dispersion:");
                egui::ComboBox::from_id_salt("qm_disp")
                    .selected_text(match prompt.options.dispersion {
                        None => "none",
                        Some(QmDispersion::D3Bj) => "D3(BJ)",
                        Some(QmDispersion::D4) => "D4",
                    })
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        ui.selectable_value(&mut prompt.options.dispersion, None, "none");
                        if d3_ok {
                            ui.selectable_value(
                                &mut prompt.options.dispersion,
                                Some(QmDispersion::D3Bj),
                                "D3(BJ)",
                            );
                        }
                        if d4_ok {
                            ui.selectable_value(
                                &mut prompt.options.dispersion,
                                Some(QmDispersion::D4),
                                "D4",
                            );
                        }
                    });
                ui.end_row();
            }

            ui.label("Charge:");
            ui.add(egui::DragValue::new(&mut prompt.charge).range(-10..=10));
            ui.end_row();

            ui.label("Multiplicity:").on_hover_text(
                "Spin multiplicity, 2S+1 (1 = singlet, 2 = doublet, 3 = triplet, …).",
            );
            ui.add(egui::DragValue::new(&mut prompt.multiplicity).range(1..=11));
            ui.end_row();
        });

    ui.separator();
    ui.label("Calculation:");
    ui.radio_value(&mut prompt.kind, QmKind::SinglePoint, "Single-point energy");
    ui.radio_value(&mut prompt.kind, QmKind::Optimize, "Geometry optimization");
    ui.radio_value(
        &mut prompt.kind,
        QmKind::Frequencies,
        "Vibrational frequencies",
    );
    ui.radio_value(
        &mut prompt.kind,
        QmKind::TransitionState,
        "Transition-state search",
    );

    if prompt.kind == QmKind::TransitionState {
        render_qm_transition_state_form(ui, prompt, entry_options);
    }

    // The three calculation kinds above are mutually exclusive (radios). The
    // property toggle below is an independent on/off, but we draw it with the
    // same filled-circle indicator so the panel speaks one visual language
    // ("selected = dot"). It is set apart under its own "Options:" header so
    // the shared dot style is not misread as "mutually exclusive with the
    // calculation above". `ui.radio` is purely visual here — the click
    // handler flips the bool; it does not join the calculation-kind group.
    ui.separator();
    ui.label("Options:");
    if ui
        .radio(
            prompt.options.compute_properties,
            "Compute dipole, charges & bond orders",
        )
        .clicked()
    {
        prompt.options.compute_properties = !prompt.options.compute_properties;
    }

    render_qm_advanced(ui, prompt);
}

/// The transition-state options: the saddle-search algorithm, the climb
/// coordinates, IRC confirmation, and the near-saddle guess route (single guess,
/// reactant→product, or a driven coordinate). Shown only for
/// [`QmKind::TransitionState`](crate::engines::qm::QmKind::TransitionState).
fn render_qm_transition_state_form(
    ui: &mut egui::Ui,
    prompt: &mut crate::frontend::state::QmPrompt,
    entry_options: &[(u64, String)],
) {
    use crate::engines::qm::{QmTsAlgorithm, QmTsCoordinates};
    use crate::frontend::state::{ScanCoordKind, TsRouteKind};

    let ts = &mut prompt.ts;
    ui.separator();
    ui.label("Transition-state search:");

    egui::Grid::new("sidebar_ts_options")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Algorithm:");
            egui::ComboBox::from_id_salt("ts_algorithm")
                .selected_text(ts.algorithm.label())
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    for algorithm in [QmTsAlgorithm::Prfo, QmTsAlgorithm::Dimer] {
                        ui.selectable_value(&mut ts.algorithm, algorithm, algorithm.label());
                    }
                });
            ui.end_row();

            // The climb-coordinate frame applies to P-RFO only; the dimer method
            // discovers its own direction.
            if ts.algorithm == QmTsAlgorithm::Prfo {
                ui.label("Climb coordinates:");
                egui::ComboBox::from_id_salt("ts_coordinates")
                    .selected_text(ts.coordinates.label())
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for coords in [QmTsCoordinates::MassWeighted, QmTsCoordinates::Internal] {
                            ui.selectable_value(&mut ts.coordinates, coords, coords.label());
                        }
                    });
                ui.end_row();
            }
        });

    if ui
        .radio(ts.confirm_irc, "Confirm with an IRC trace from the saddle")
        .clicked()
    {
        ts.confirm_irc = !ts.confirm_irc;
    }

    ui.add_space(4.0);
    ui.label("Initial guess:");
    ui.radio_value(
        &mut ts.route,
        TsRouteKind::Single,
        "From the current geometry",
    );
    ui.radio_value(
        &mut ts.route,
        TsRouteKind::TwoEndpoint,
        "Between a reactant and a product",
    );
    ui.radio_value(
        &mut ts.route,
        TsRouteKind::CoordinateScan,
        "Along a driven coordinate",
    );

    match ts.route {
        TsRouteKind::Single => {
            ui.weak("The current geometry must already sit near the saddle.");
        }
        TsRouteKind::TwoEndpoint => {
            ui.add_space(4.0);
            egui::Grid::new("sidebar_ts_two_endpoint")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Product:");
                    let selected = ts
                        .product_entry
                        .and_then(|id| entry_options.iter().find(|(eid, _)| *eid == id))
                        .map(|(_, name)| name.clone())
                        .unwrap_or_else(|| "(choose an entry)".to_string());
                    egui::ComboBox::from_id_salt("ts_product")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            crate::frontend::theme::stabilize_selectable_rows(ui);
                            for (id, name) in entry_options {
                                ui.selectable_value(&mut ts.product_entry, Some(*id), name.clone());
                            }
                        });
                    ui.end_row();
                });

            if ui
                .radio(ts.use_neb, "Relax a climbing-image NEB band (more robust)")
                .clicked()
            {
                ts.use_neb = !ts.use_neb;
            }
            egui::Grid::new("sidebar_ts_neb")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    if ts.use_neb {
                        ui.label("Band images:");
                        ui.add(egui::DragValue::new(&mut ts.neb_images).range(3..=24));
                        ui.end_row();
                    } else {
                        ui.label("IDPP energy scan:");
                        // 0 = the single geometric IDPP image; ≥3 scans the path
                        // for the energy peak.
                        ui.add(
                            egui::DragValue::new(&mut ts.idpp_scan_points)
                                .range(0..=21)
                                .suffix(" pts"),
                        );
                        ui.end_row();
                    }
                });
            // Atom reordering matters on the NEB route; the IDPP route always maps
            // by connectivity, so this toggle only bites when "use NEB" is on.
            if ui
                .radio(
                    ts.map_atoms,
                    "Reorder product atoms to match the reactant (NEB)",
                )
                .clicked()
            {
                ts.map_atoms = !ts.map_atoms;
            }
        }
        TsRouteKind::CoordinateScan => {
            ui.add_space(4.0);
            egui::Grid::new("sidebar_ts_scan")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Coordinate:");
                    egui::ComboBox::from_id_salt("ts_scan_kind")
                        .selected_text(match ts.scan_kind {
                            ScanCoordKind::Bond => "Bond (Å)",
                            ScanCoordKind::Angle => "Angle (°)",
                            ScanCoordKind::Dihedral => "Dihedral (°)",
                        })
                        .show_ui(ui, |ui| {
                            crate::frontend::theme::stabilize_selectable_rows(ui);
                            ui.selectable_value(&mut ts.scan_kind, ScanCoordKind::Bond, "Bond (Å)");
                            ui.selectable_value(
                                &mut ts.scan_kind,
                                ScanCoordKind::Angle,
                                "Angle (°)",
                            );
                            ui.selectable_value(
                                &mut ts.scan_kind,
                                ScanCoordKind::Dihedral,
                                "Dihedral (°)",
                            );
                        });
                    ui.end_row();

                    // Atom indices (1-based), one DragValue per coordinate slot.
                    ui.label("Atoms (1-based):");
                    ui.horizontal(|ui| {
                        for slot in 0..ts.scan_kind.arity() {
                            ui.add(
                                egui::DragValue::new(&mut ts.scan_atoms[slot]).range(1..=100_000),
                            );
                        }
                    });
                    ui.end_row();

                    let unit = if ts.scan_kind == ScanCoordKind::Bond {
                        " Å"
                    } else {
                        "°"
                    };
                    ui.label("From:");
                    ui.add(
                        egui::DragValue::new(&mut ts.scan_start)
                            .speed(0.05)
                            .suffix(unit),
                    );
                    ui.end_row();
                    ui.label("To:");
                    ui.add(
                        egui::DragValue::new(&mut ts.scan_end)
                            .speed(0.05)
                            .suffix(unit),
                    );
                    ui.end_row();
                    ui.label("Steps:");
                    ui.add(egui::DragValue::new(&mut ts.scan_points).range(3..=51));
                    ui.end_row();
                });
        }
    }
}

/// The periodic (crystalline) QM form: the LDA functional, GTH basis, k-point
/// mesh, grid cutoff, SCF iteration cap, and the optional force/stress outputs.
/// Shown only when the active structure carries a real unit cell.
pub(crate) fn render_periodic_qm_form(
    ui: &mut egui::Ui,
    form: &mut crate::frontend::state::PeriodicQmForm,
) {
    use crate::engines::qm::{PeriodicFunctional, periodic};

    egui::Grid::new("sidebar_periodic_qm_options")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Functional:");
            egui::ComboBox::from_id_salt("periodic_functional")
                .selected_text(form.functional.label())
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    for functional in PeriodicFunctional::all() {
                        ui.selectable_value(&mut form.functional, functional, functional.label());
                    }
                });
            ui.end_row();

            ui.label("Basis (GTH):");
            egui::ComboBox::from_id_salt("periodic_basis")
                .selected_text(form.basis.clone())
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    for basis in periodic::PERIODIC_BASES {
                        ui.selectable_value(&mut form.basis, basis.to_string(), *basis);
                    }
                });
            ui.end_row();

            ui.label("k-point mesh:");
            ui.horizontal(|ui| {
                for division in &mut form.kmesh {
                    ui.add(egui::DragValue::new(division).range(1..=12));
                }
            });
            ui.end_row();

            ui.label("Grid cutoff (Ry):");
            // Floor at 100 Ry: below that a GPW real-space grid is too coarse to
            // give a meaningful energy (hartree's own default is 280 Ry).
            ui.add(
                egui::DragValue::new(&mut form.e_cut_ry)
                    .range(100.0..=1200.0)
                    .speed(10.0),
            );
            ui.end_row();

            ui.label("Max SCF iters:");
            ui.add(egui::DragValue::new(&mut form.max_iter).range(10..=500));
            ui.end_row();
        });

    ui.separator();
    ui.label("Outputs:");
    ui.checkbox(&mut form.forces, "Forces on atoms");
    ui.checkbox(&mut form.stress, "Cell stress tensor");

    ui.add_space(4.0);
    ui.small(
        "Periodic GPW uses GTH pseudopotentials (closed-shell LDA); net charge \
         and spin are not modeled.",
    );
}

/// Render the on-demand memory estimate from the "Estimate memory" button: a peak
/// figure with its backend and level of theory, colored by whether it fits the
/// safe RAM budget of `location` (this machine, or a selected remote host).
pub(crate) fn render_qm_memory_report(
    ui: &mut egui::Ui,
    report: &crate::engines::qm::QmMemoryReport,
    location: &str,
) {
    let pal = crate::frontend::theme::palette(ui);
    let gib = |bytes: u64| bytes as f64 / 1024.0_f64.powi(3);
    let fits = report.fits();
    let color = if fits {
        pal.status_green
    } else {
        pal.status_amber
    };

    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("≈ {:.2} GiB peak", gib(report.peak_bytes)))
                .strong()
                .color(color),
        );
        ui.label(
            RichText::new(format!(
                "· {} · {}/{}",
                report.backend_label, report.method_label, report.basis_label
            ))
            .color(pal.text_tertiary),
        );
    });
    let verdict = if fits { "fits the" } else { "exceeds the" };
    ui.label(
        RichText::new(format!(
            "{verdict} {:.1} GiB safe budget on {location}",
            gib(report.budget_bytes)
        ))
        .small()
        .color(color),
    );
}

/// Common implicit-solvation solvent names offered in the panel dropdown. hartree
/// accepts more (especially for SMD); the console `qm` command takes any name.
const QM_SOLVENTS: &[&str] = &[
    "water",
    "acetonitrile",
    "methanol",
    "ethanol",
    "dmso",
    "acetone",
    "thf",
    "dmf",
    "toluene",
    "benzene",
    "chloroform",
];

/// The "Advanced" collapsing section of the QM panel: solvation, SCF backend,
/// relativity, smearing, FOD, and thermochemistry knobs. Options that do not
/// apply to the chosen method are hidden rather than shown disabled.
fn render_qm_advanced(ui: &mut egui::Ui, prompt: &mut crate::frontend::state::QmPrompt) {
    use crate::engines::qm::{CpcmDielectric, QmMethod, QmScfBackend, QmSolvation};

    let method_is_post_hf = matches!(
        prompt.method,
        QmMethod::Mp2 | QmMethod::Ccsd | QmMethod::CcsdT
    );
    let method_is_mp2 = matches!(prompt.method, QmMethod::Mp2);

    egui::CollapsingHeader::new("Advanced")
        .default_open(false)
        .show(ui, |ui| {
            // Solvation: model + solvent name, reassembled each frame.
            let (mut model, mut name) = match &prompt.options.solvation {
                None => (0u8, "water".to_string()),
                Some(QmSolvation::Cpcm(CpcmDielectric::Named(n))) => (1, n.clone()),
                Some(QmSolvation::Cpcm(CpcmDielectric::Epsilon(_))) => (1, "water".to_string()),
                Some(QmSolvation::Smd(n)) => (2, n.clone()),
                Some(QmSolvation::Alpb(n)) => (3, n.clone()),
                Some(QmSolvation::Gbsa(n)) => (4, n.clone()),
            };
            ui.horizontal(|ui| {
                ui.label("Solvation:");
                egui::ComboBox::from_id_salt("qm_solv_model")
                    .selected_text(match model {
                        1 => "C-PCM",
                        2 => "SMD",
                        3 => "ALPB",
                        4 => "GBSA",
                        _ => "none",
                    })
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for (value, label) in [
                            (0, "none"),
                            (1, "C-PCM"),
                            (2, "SMD"),
                            (3, "ALPB"),
                            (4, "GBSA"),
                        ] {
                            ui.selectable_value(&mut model, value as u8, label);
                        }
                    });
                if model != 0 {
                    egui::ComboBox::from_id_salt("qm_solv_name")
                        .selected_text(name.clone())
                        .show_ui(ui, |ui| {
                            crate::frontend::theme::stabilize_selectable_rows(ui);
                            for s in QM_SOLVENTS {
                                ui.selectable_value(&mut name, s.to_string(), *s);
                            }
                        });
                }
            });
            prompt.options.solvation = match model {
                1 => Some(QmSolvation::Cpcm(CpcmDielectric::Named(name))),
                2 => Some(QmSolvation::Smd(name)),
                3 => Some(QmSolvation::Alpb(name)),
                4 => Some(QmSolvation::Gbsa(name)),
                _ => None,
            };

            // SCF backend.
            ui.horizontal(|ui| {
                ui.label("SCF backend:");
                egui::ComboBox::from_id_salt("qm_backend")
                    .selected_text(prompt.options.scf_backend.label())
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for backend in [
                            QmScfBackend::InCore,
                            QmScfBackend::Direct,
                            QmScfBackend::RiJk,
                            QmScfBackend::Cosx,
                        ] {
                            ui.selectable_value(
                                &mut prompt.options.scf_backend,
                                backend,
                                backend.label(),
                            );
                        }
                    });
            });

            // DFT grid level override.
            let mut grid_override = prompt.options.grid_level.is_some();
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut grid_override, "Override DFT grid")
                    .changed()
                {
                    prompt.options.grid_level = grid_override.then_some(3);
                }
                if let Some(level) = &mut prompt.options.grid_level {
                    ui.add(egui::DragValue::new(level).range(0..=4));
                }
            });

            // Fermi smearing.
            let mut smear_on = prompt.options.smearing_temperature_k.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut smear_on, "Fermi smearing (K)").changed() {
                    prompt.options.smearing_temperature_k = smear_on.then_some(1000.0);
                }
                if let Some(temp) = &mut prompt.options.smearing_temperature_k {
                    ui.add(egui::DragValue::new(temp).range(1.0..=50_000.0).speed(50.0));
                }
            });

            // Method-specific toggles.
            if method_is_mp2 {
                ui.checkbox(&mut prompt.options.ri_mp2, "RI-MP2 (density-fit MP2)");
            }
            if method_is_post_hf {
                ui.checkbox(
                    &mut prompt.options.all_electron,
                    "All-electron (no frozen core)",
                );
            }
            ui.checkbox(&mut prompt.options.x2c, "X2C scalar relativity");
            ui.checkbox(&mut prompt.options.fod, "FOD multireference diagnostic");
            ui.checkbox(
                &mut prompt.options.single_point_hessian,
                "Single-point Hessian (approx. frequencies)",
            );

            // Thermochemistry parameters (used by frequency runs).
            egui::Grid::new("qm_thermo")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Symmetry number σ:");
                    ui.add(egui::DragValue::new(&mut prompt.options.symmetry_number).range(1..=48));
                    ui.end_row();
                    ui.label("quasi-RRHO ω₀ (cm⁻¹):");
                    ui.add(
                        egui::DragValue::new(&mut prompt.options.qrrho_w0_cm1)
                            .range(1.0..=1000.0)
                            .speed(1.0),
                    );
                    ui.end_row();
                });
        });
}
