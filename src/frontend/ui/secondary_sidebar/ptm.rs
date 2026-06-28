use super::*;

use crate::domain::modification::{MethylDegree, UblKind};
use crate::frontend::ptm_commands::{LipidKind, lipid_label, methyl_prefix, ubl_label};
use crate::frontend::state::PtmUiKind;
use crate::workflows::glycan::GlycosylationKind;

/// The Modify Protein (PTM) task panel: pick a modification family, an anchor
/// residue, and the family-specific options, then apply it to the active
/// protein. A pure view over `pending_ptm` — every edit is emitted as an
/// [`AppAction`] the dispatcher applies, so the console and panel share one
/// mutation path (the `apply_ptm` seam).
pub(crate) fn render_ptm_task_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    // Snapshot the workspace entries for the UBL override picker before the
    // prompt is borrowed.
    let entries: Vec<(u64, String)> = state
        .entries
        .records
        .iter()
        .map(|record| (record.id, record.name.clone()))
        .collect();

    let Some(prompt) = &state.ui.pending_ptm else {
        ui.label("Open this task to modify a protein.");
        return;
    };

    // --- 1. Modification family --------------------------------------------
    ui.horizontal(|ui| {
        ui.label("Modification:");
        egui::ComboBox::from_id_salt("ptm_family")
            .selected_text(prompt.family.label())
            .show_ui(ui, |ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
                for kind in PtmUiKind::ALL {
                    if ui
                        .selectable_label(prompt.family == kind, kind.label())
                        .clicked()
                    {
                        actions.push(AppAction::SetPtmFamily(kind));
                    }
                }
            });
    });
    ui.separator();

    // --- 2. Anchor residue --------------------------------------------------
    ui.label(RichText::new("Target residue").strong());
    ui.horizontal(|ui| {
        ui.label("Chain:");
        let mut chain = prompt.chain.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut chain)
                    .hint_text("A")
                    .desired_width(36.0),
            )
            .changed()
        {
            actions.push(AppAction::SetPtmChain(chain));
        }
        ui.label("Residue #:");
        let mut res_seq = prompt.res_seq as f64;
        if ui
            .add(
                egui::DragValue::new(&mut res_seq)
                    .speed(1.0)
                    .range(-9999.0..=99_999.0),
            )
            .changed()
        {
            actions.push(AppAction::SetPtmResSeq(res_seq as i32));
        }
    });
    ui.label(
        RichText::new(prompt.family.target_hint())
            .small()
            .color(egui::Color32::GRAY),
    );

    // --- 3. Family-specific controls ---------------------------------------
    render_family_controls(prompt, &entries, ui, actions);
    ui.separator();

    // --- 4. Result name -----------------------------------------------------
    ui.horizontal(|ui| {
        ui.label("Result name:");
        let mut name = prompt.output_name.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut name)
                    .hint_text("(automatic)")
                    .desired_width(180.0),
            )
            .changed()
        {
            actions.push(AppAction::SetPtmName(name));
        }
    });
    ui.separator();

    // --- 5. Apply / Cancel --------------------------------------------------
    ui.horizontal(|ui| {
        if ui
            .button(format!("{}  Apply", egui_phosphor::regular::SPARKLE))
            .clicked()
        {
            actions.push(AppAction::StartPtm);
        }
        if ui
            .button(format!("{}  Cancel", egui_phosphor::regular::X))
            .clicked()
        {
            actions.push(AppAction::CancelPtmPrompt);
        }
    });
}

/// Render the controls unique to the selected family (degree, lipid, ubl, or the
/// acetyl N-terminus toggle); the other families add no extra controls.
fn render_family_controls(
    prompt: &crate::frontend::state::PendingPtm,
    entries: &[(u64, String)],
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    match prompt.family {
        PtmUiKind::Phosphorylate => {}
        PtmUiKind::Acetylate => {
            let mut n_terminal = prompt.n_terminal;
            if ui
                .checkbox(&mut n_terminal, "Cap the chain N-terminus (not Lys NZ)")
                .changed()
            {
                actions.push(AppAction::SetPtmNTerminal(n_terminal));
            }
        }
        PtmUiKind::Methylate => {
            ui.horizontal(|ui| {
                ui.label("Degree:");
                egui::ComboBox::from_id_salt("ptm_degree")
                    .selected_text(methyl_prefix(prompt.degree))
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for degree in [MethylDegree::Mono, MethylDegree::Di, MethylDegree::Tri] {
                            if ui
                                .selectable_label(prompt.degree == degree, methyl_prefix(degree))
                                .clicked()
                            {
                                actions.push(AppAction::SetPtmDegree(degree));
                            }
                        }
                    });
            });
        }
        PtmUiKind::Lipidate => {
            ui.horizontal(|ui| {
                ui.label("Lipid:");
                egui::ComboBox::from_id_salt("ptm_lipid")
                    .selected_text(lipid_label(prompt.lipid))
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for kind in [
                            LipidKind::Palmitoyl,
                            LipidKind::Myristoyl,
                            LipidKind::Farnesyl,
                            LipidKind::GeranylGeranyl,
                        ] {
                            if ui
                                .selectable_label(prompt.lipid == kind, lipid_label(kind))
                                .clicked()
                            {
                                actions.push(AppAction::SetPtmLipid(kind));
                            }
                        }
                    });
            });
        }
        PtmUiKind::Ubiquitinate => {
            ui.horizontal(|ui| {
                ui.label("Modifier:");
                egui::ComboBox::from_id_salt("ptm_ubl")
                    .selected_text(ubl_label(prompt.ubl))
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for kind in [UblKind::Ubiquitin, UblKind::Sumo, UblKind::Nedd8] {
                            if ui
                                .selectable_label(prompt.ubl == kind, ubl_label(kind))
                                .clicked()
                            {
                                actions.push(AppAction::SetPtmUbl(kind));
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Template:");
                let current = prompt
                    .ubl_override
                    .and_then(|id| entries.iter().find(|(entry, _)| *entry == id))
                    .map(|(_, label)| label.clone())
                    .unwrap_or_else(|| "Bundled".to_string());
                egui::ComboBox::from_id_salt("ptm_ubl_override")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        if ui
                            .selectable_label(prompt.ubl_override.is_none(), "Bundled")
                            .clicked()
                        {
                            actions.push(AppAction::SetPtmUblOverride(None));
                        }
                        for (id, label) in entries {
                            if ui
                                .selectable_label(prompt.ubl_override == Some(*id), label)
                                .clicked()
                            {
                                actions.push(AppAction::SetPtmUblOverride(Some(*id)));
                            }
                        }
                    });
            });
        }
        PtmUiKind::Glycosylate => {
            ui.horizontal(|ui| {
                ui.label("Glycan (IUPAC):");
                let mut iupac = prompt.glycan_iupac.clone();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut iupac)
                            .hint_text("Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc")
                            .desired_width(260.0),
                    )
                    .changed()
                {
                    actions.push(AppAction::SetPtmGlycanIupac(iupac));
                }
            });
            ui.horizontal(|ui| {
                ui.label("Linkage:");
                egui::ComboBox::from_id_salt("ptm_glyco_kind")
                    .selected_text(glyco_kind_label(prompt.glyco_kind))
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for kind in [GlycosylationKind::NLinked, GlycosylationKind::OLinked] {
                            if ui
                                .selectable_label(prompt.glyco_kind == kind, glyco_kind_label(kind))
                                .clicked()
                            {
                                actions.push(AppAction::SetPtmGlycoKind(kind));
                            }
                        }
                    });
            });
        }
    }
}

/// Display label for a glycosylation linkage in the panel selector.
fn glyco_kind_label(kind: GlycosylationKind) -> &'static str {
    match kind {
        GlycosylationKind::NLinked => "N-linked",
        GlycosylationKind::OLinked => "O-linked",
    }
}
