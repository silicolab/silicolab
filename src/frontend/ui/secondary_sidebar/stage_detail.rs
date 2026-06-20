use super::*;

/// The expanded detail view: structural fields that have no inline slot, then the
/// finer parameters split into Standard (shown) and Advanced (collapsed) tiers
/// driven by the [`ParamId`] descriptor table, then per-stage raw passthrough.
pub(crate) fn stage_detail_view(
    ui: &mut egui::Ui,
    index: usize,
    stage: &crate::workflows::molecular_dynamics::MdStage,
    family: crate::workflows::molecular_dynamics::ForceFieldFamily,
    actions: &mut Vec<AppAction>,
) {
    use crate::frontend::state::MdStageEdit;
    use crate::workflows::molecular_dynamics::run::{
        BarostatKind, CouplingGroups, MdParameters, ParamTier,
    };

    egui::Grid::new(("md_stage_detail", index))
        .num_columns(3)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            // Timestep (dynamics only).
            if stage.kind.is_dynamics() {
                ui.label("Timestep (ps)");
                let mut dt = stage.timestep_ps;
                if ui
                    .add(
                        egui::DragValue::new(&mut dt)
                            .range(0.0005..=0.005_f32)
                            .speed(0.0005)
                            .fixed_decimals(4),
                    )
                    .changed()
                {
                    actions.push(AppAction::EditMdRunStage {
                        index,
                        edit: MdStageEdit::Timestep(dt),
                    });
                }
                ui.label("");
                ui.end_row();

                // Coupling groups.
                ui.label("Coupling groups");
                let mut groups = stage.coupling_groups;
                egui::ComboBox::from_id_salt(("md_stage_groups", index))
                    .selected_text(coupling_groups_label(groups))
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for option in [
                            CouplingGroups::WholeSystem,
                            CouplingGroups::SoluteSolvent,
                            CouplingGroups::SoluteLipidSolvent,
                            CouplingGroups::NucleicSolvent,
                        ] {
                            ui.selectable_value(&mut groups, option, coupling_groups_label(option));
                        }
                    });
                if groups != stage.coupling_groups {
                    actions.push(AppAction::EditMdRunStage {
                        index,
                        edit: MdStageEdit::CouplingGroups(groups),
                    });
                }
                ui.label("");
                ui.end_row();
            }

            // Barostat (pressure-coupled stages only).
            if let Some(pressure) = stage.pressure {
                ui.label("Barostat");
                let mut barostat = pressure.barostat;
                egui::ComboBox::from_id_salt(("md_stage_barostat", index))
                    .selected_text(barostat_label(barostat))
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for option in [
                            BarostatKind::StochasticCellRescale,
                            BarostatKind::ParrinelloRahman,
                            BarostatKind::Berendsen,
                        ] {
                            ui.selectable_value(&mut barostat, option, barostat_label(option));
                        }
                    });
                if barostat != pressure.barostat {
                    actions.push(AppAction::EditMdRunStage {
                        index,
                        edit: MdStageEdit::Barostat(barostat),
                    });
                }
                ui.label("");
                ui.end_row();

                ui.label("Barostat τ (ps)");
                let mut tau = pressure.tau_ps;
                if ui
                    .add(
                        egui::DragValue::new(&mut tau)
                            .range(0.5..=20.0_f32)
                            .speed(0.1)
                            .fixed_decimals(1),
                    )
                    .changed()
                {
                    actions.push(AppAction::EditMdRunStage {
                        index,
                        edit: MdStageEdit::BarostatTau(tau),
                    });
                }
                ui.label("");
                ui.end_row();
            }

            // Restraint force constant (restrained stages only).
            if let Some(fc) = stage.restraint.force_constant() {
                ui.label("Restraint k (kJ/mol/nm²)");
                let mut value = fc;
                if ui
                    .add(
                        egui::DragValue::new(&mut value)
                            .range(0.0..=100_000.0_f32)
                            .speed(50.0),
                    )
                    .changed()
                {
                    actions.push(AppAction::EditMdRunStage {
                        index,
                        edit: MdStageEdit::RestraintForceConstant(value),
                    });
                }
                ui.label("");
                ui.end_row();
            }

            // Annealing ramp (annealing stages only).
            if let Some(spec) = &stage.anneal {
                let start = spec
                    .points
                    .first()
                    .copied()
                    .unwrap_or((0.0, stage.temperature_k));
                let end = spec.points.last().copied().unwrap_or(start);
                let (mut start_k, mut end_k, mut duration_ps) = (start.1, end.1, end.0);
                let mut emit = false;
                ui.label("Anneal start (K)");
                emit |= ui
                    .add(
                        egui::DragValue::new(&mut start_k)
                            .range(1.0..=2_000.0_f32)
                            .speed(1.0),
                    )
                    .changed();
                ui.label("");
                ui.end_row();
                ui.label("Anneal end (K)");
                emit |= ui
                    .add(
                        egui::DragValue::new(&mut end_k)
                            .range(1.0..=2_000.0_f32)
                            .speed(1.0),
                    )
                    .changed();
                ui.label("");
                ui.end_row();
                ui.label("Anneal duration (ps)");
                emit |= ui
                    .add(
                        egui::DragValue::new(&mut duration_ps)
                            .range(1.0..=1e6)
                            .speed(10.0),
                    )
                    .changed();
                ui.label("");
                ui.end_row();
                if emit {
                    actions.push(AppAction::EditMdRunStage {
                        index,
                        edit: MdStageEdit::Anneal {
                            start_k,
                            end_k,
                            duration_ps,
                        },
                    });
                }
            }

            // Standard-tier finer parameters (the descriptor table is the source
            // of truth for the inline-vs-detail split: Basic params live inline
            // above, Standard shows here, Advanced collapses below).
            for pid in MdParameters::tiers() {
                if pid.tier() == ParamTier::Standard {
                    param_row(ui, index, stage, *pid, family, actions);
                }
            }
        });

    // Advanced-tier parameters, collapsed by default.
    egui::CollapsingHeader::new("Advanced parameters")
        .id_salt(("md_stage_adv", index))
        .show(ui, |ui| {
            egui::Grid::new(("md_stage_adv_grid", index))
                .num_columns(3)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    for pid in MdParameters::tiers() {
                        if pid.tier() == ParamTier::Advanced {
                            param_row(ui, index, stage, *pid, family, actions);
                        }
                    }
                });
        });

    // Per-stage raw passthrough — identical semantics to `md run --raw`.
    egui::CollapsingHeader::new("Raw .mdp passthrough")
        .id_salt(("md_stage_raw", index))
        .show(ui, |ui| {
            for (line, (key, value)) in stage.raw_passthrough.iter().enumerate() {
                ui.horizontal(|ui| {
                    let mut k = key.clone();
                    let mut v = value.clone();
                    let key_changed = ui
                        .add(
                            egui::TextEdit::singleline(&mut k)
                                .hint_text("key")
                                .desired_width(120.0),
                        )
                        .changed();
                    ui.label("=");
                    let value_changed = ui
                        .add(
                            egui::TextEdit::singleline(&mut v)
                                .hint_text("value")
                                .desired_width(120.0),
                        )
                        .changed();
                    if key_changed || value_changed {
                        actions.push(AppAction::EditMdRunStage {
                            index,
                            edit: MdStageEdit::SetRawLine {
                                line,
                                key: k,
                                value: v,
                            },
                        });
                    }
                    if ui
                        .add(egui::Button::new(egui_phosphor::regular::TRASH).frame(false))
                        .clicked()
                    {
                        actions.push(AppAction::EditMdRunStage {
                            index,
                            edit: MdStageEdit::RemoveRawLine(line),
                        });
                    }
                });
            }
            if ui.button("+ Add line").clicked() {
                actions.push(AppAction::EditMdRunStage {
                    index,
                    edit: MdStageEdit::AddRawLine,
                });
            }
            ui.label(
                RichText::new(
                    "Verbatim key = value lines, merged last; may override any directive.",
                )
                .small()
                .color(egui::Color32::GRAY),
            );
        });
}

pub(crate) fn coupling_groups_label(
    groups: crate::workflows::molecular_dynamics::run::CouplingGroups,
) -> &'static str {
    use crate::workflows::molecular_dynamics::run::CouplingGroups;
    match groups {
        CouplingGroups::WholeSystem => "Whole system",
        CouplingGroups::SoluteSolvent => "Solute / solvent",
        CouplingGroups::SoluteLipidSolvent => "Solute / lipid / solvent",
        CouplingGroups::NucleicSolvent => "Nucleic / solvent",
    }
}

pub(crate) fn barostat_label(
    barostat: crate::workflows::molecular_dynamics::run::BarostatKind,
) -> &'static str {
    use crate::workflows::molecular_dynamics::run::BarostatKind;
    match barostat {
        BarostatKind::StochasticCellRescale => "Stochastic cell rescale",
        BarostatKind::ParrinelloRahman => "Parrinello–Rahman",
        BarostatKind::Berendsen => "Berendsen (equilibration)",
    }
}

/// Render one tiered parameter row (label · control · default-state) for a
/// [`ParamId`], emitting the matching [`MdStageEdit`] when the user changes it.
/// `None` parameters show a "set" affordance; set ones show a "default" revert.
pub(crate) fn param_row(
    ui: &mut egui::Ui,
    index: usize,
    stage: &crate::workflows::molecular_dynamics::MdStage,
    pid: crate::workflows::molecular_dynamics::run::ParamId,
    family: crate::workflows::molecular_dynamics::ForceFieldFamily,
    actions: &mut Vec<AppAction>,
) {
    use crate::frontend::state::MdStageEdit;
    use crate::workflows::molecular_dynamics::run::{
        ConstraintScope, ParamId, ThermostatKind, family_nonbonded_intent,
    };

    let params = &stage.params;
    let (rc, rv) = family_nonbonded_intent(family);
    let mut push = |edit: MdStageEdit| {
        actions.push(AppAction::EditMdRunStage { index, edit });
    };

    match pid {
        ParamId::CoulombCutoff => {
            if let Some(new) = opt_f32_row(
                ui,
                index,
                pid,
                params.coulomb_cutoff_nm,
                rc,
                0.5..=2.0,
                0.05,
                2,
            ) {
                push(MdStageEdit::CoulombCutoff(new));
            }
        }
        ParamId::VdwCutoff => {
            if let Some(new) =
                opt_f32_row(ui, index, pid, params.vdw_cutoff_nm, rv, 0.5..=2.0, 0.05, 2)
            {
                push(MdStageEdit::VdwCutoff(new));
            }
        }
        ParamId::Thermostat => {
            ui.label(pid.label());
            let mut selected = params.thermostat;
            let label = match selected {
                None => "default",
                Some(ThermostatKind::StochasticVelocityRescale) => "V-rescale",
                Some(ThermostatKind::NoseHoover) => "Nosé–Hoover",
                Some(ThermostatKind::Berendsen) => "Berendsen",
            };
            egui::ComboBox::from_id_salt(("md_param_thermostat", index))
                .selected_text(label)
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    ui.selectable_value(&mut selected, None, "default");
                    ui.selectable_value(
                        &mut selected,
                        Some(ThermostatKind::StochasticVelocityRescale),
                        "V-rescale",
                    );
                    ui.selectable_value(
                        &mut selected,
                        Some(ThermostatKind::NoseHoover),
                        "Nosé–Hoover",
                    );
                    ui.selectable_value(
                        &mut selected,
                        Some(ThermostatKind::Berendsen),
                        "Berendsen",
                    );
                });
            if selected != params.thermostat {
                push(MdStageEdit::Thermostat(selected));
            }
            ui.label("");
            ui.end_row();
        }
        ParamId::ThermostatTau => {
            if let Some(new) = opt_f32_row(
                ui,
                index,
                pid,
                params.thermostat_tau_ps,
                0.1,
                0.05..=10.0,
                0.05,
                2,
            ) {
                push(MdStageEdit::ThermostatTau(new));
            }
        }
        ParamId::Constraints => {
            ui.label(pid.label());
            let mut selected = params.constraints;
            let label = match selected {
                None => "default",
                Some(ConstraintScope::None) => "None (flexible)",
                Some(ConstraintScope::HBonds) => "H-bonds",
                Some(ConstraintScope::AllBonds) => "All bonds",
            };
            egui::ComboBox::from_id_salt(("md_param_constraints", index))
                .selected_text(label)
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    ui.selectable_value(&mut selected, None, "default");
                    ui.selectable_value(
                        &mut selected,
                        Some(ConstraintScope::None),
                        "None (flexible)",
                    );
                    ui.selectable_value(&mut selected, Some(ConstraintScope::HBonds), "H-bonds");
                    ui.selectable_value(
                        &mut selected,
                        Some(ConstraintScope::AllBonds),
                        "All bonds",
                    );
                });
            if selected != params.constraints {
                push(MdStageEdit::Constraints(selected));
            }
            ui.label("");
            ui.end_row();
        }
        ParamId::PmeSpacing => {
            if let Some(new) = opt_f32_row(
                ui,
                index,
                pid,
                params.pme_spacing_nm,
                0.12,
                0.05..=0.3,
                0.01,
                3,
            ) {
                push(MdStageEdit::PmeSpacing(new));
            }
        }
        ParamId::PmeOrder => {
            if let Some(new) = opt_u32_row(ui, index, pid, params.pme_order, 4, 4..=10) {
                push(MdStageEdit::PmeOrder(new));
            }
        }
        ParamId::ConstraintOrder => {
            if let Some(new) = opt_u32_row(ui, index, pid, params.constraint_order, 4, 1..=12) {
                push(MdStageEdit::ConstraintOrder(new));
            }
        }
        ParamId::ConstraintIterations => {
            if let Some(new) = opt_u32_row(ui, index, pid, params.constraint_iterations, 1, 1..=10)
            {
                push(MdStageEdit::ConstraintIterations(new));
            }
        }
        ParamId::DispersionCorrection => {
            if let Some(new) = opt_bool_row(ui, index, pid, params.dispersion_correction) {
                push(MdStageEdit::DispersionCorrection(new));
            }
        }
        ParamId::RemoveComMotion => {
            if let Some(new) = opt_bool_row(ui, index, pid, params.remove_com_motion) {
                push(MdStageEdit::RemoveComMotion(new));
            }
        }
        ParamId::NeighborListSteps => {
            if let Some(new) = opt_u32_row(ui, index, pid, params.neighbor_list_steps, 10, 1..=200)
            {
                push(MdStageEdit::NeighborListSteps(new));
            }
        }
        ParamId::RandomSeed => {
            ui.label(pid.label());
            match params.random_seed {
                Some(mut seed) => {
                    if ui
                        .add(
                            egui::DragValue::new(&mut seed)
                                .range(-1..=i64::MAX)
                                .speed(1.0),
                        )
                        .changed()
                    {
                        push(MdStageEdit::RandomSeed(Some(seed)));
                    }
                    if ui.small_button("default").clicked() {
                        push(MdStageEdit::RandomSeed(None));
                    }
                }
                None => {
                    if ui.button("set").clicked() {
                        push(MdStageEdit::RandomSeed(Some(-1)));
                    }
                    ui.label(RichText::new("default").small().color(egui::Color32::GRAY));
                }
            }
            ui.end_row();
        }
    }
}

/// A label · DragValue · revert row for an `Option<f32>` parameter. Returns the
/// new value (`Some(Some(v))` to set, `Some(None)` to revert) when it changes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn opt_f32_row(
    ui: &mut egui::Ui,
    index: usize,
    pid: crate::workflows::molecular_dynamics::run::ParamId,
    value: Option<f32>,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
    speed: f32,
    decimals: usize,
) -> Option<Option<f32>> {
    let _ = index;
    ui.label(pid.label());
    let mut result = None;
    match value {
        Some(mut v) => {
            if ui
                .add(
                    egui::DragValue::new(&mut v)
                        .range(range)
                        .speed(speed as f64)
                        .fixed_decimals(decimals),
                )
                .changed()
            {
                result = Some(Some(v));
            }
            if ui.small_button("default").clicked() {
                result = Some(None);
            }
        }
        None => {
            if ui.button("set").clicked() {
                result = Some(Some(default));
            }
            ui.label(RichText::new("default").small().color(egui::Color32::GRAY));
        }
    }
    ui.end_row();
    result
}

/// As [`opt_f32_row`] for an `Option<u32>` parameter.
pub(crate) fn opt_u32_row(
    ui: &mut egui::Ui,
    index: usize,
    pid: crate::workflows::molecular_dynamics::run::ParamId,
    value: Option<u32>,
    default: u32,
    range: std::ops::RangeInclusive<u32>,
) -> Option<Option<u32>> {
    let _ = index;
    ui.label(pid.label());
    let mut result = None;
    match value {
        Some(mut v) => {
            if ui
                .add(egui::DragValue::new(&mut v).range(range).speed(1.0))
                .changed()
            {
                result = Some(Some(v));
            }
            if ui.small_button("default").clicked() {
                result = Some(None);
            }
        }
        None => {
            if ui.button("set").clicked() {
                result = Some(Some(default));
            }
            ui.label(RichText::new("default").small().color(egui::Color32::GRAY));
        }
    }
    ui.end_row();
    result
}

/// As [`opt_f32_row`] for an `Option<bool>` parameter (default / yes / no combo).
pub(crate) fn opt_bool_row(
    ui: &mut egui::Ui,
    index: usize,
    pid: crate::workflows::molecular_dynamics::run::ParamId,
    value: Option<bool>,
) -> Option<Option<bool>> {
    ui.label(pid.label());
    let mut selected = value;
    let label = match selected {
        None => "default",
        Some(true) => "yes",
        Some(false) => "no",
    };
    egui::ComboBox::from_id_salt(("md_param_bool", pid.label(), index))
        .selected_text(label)
        .show_ui(ui, |ui| {
            crate::frontend::theme::stabilize_selectable_rows(ui);
            ui.selectable_value(&mut selected, None, "default");
            ui.selectable_value(&mut selected, Some(true), "yes");
            ui.selectable_value(&mut selected, Some(false), "no");
        });
    let result = (selected != value).then_some(selected);
    ui.label("");
    ui.end_row();
    result
}
