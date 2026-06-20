use eframe::egui::{self, Ui};

use crate::{
    domain::Structure,
    frontend::widgets::{combo_box, drag_value, supercell_value},
    workflows::nanosheet::{
        CarbonNitrideNode, NanosheetSpec, SheetFamily, SheetKind, TmdPolytype, presets,
    },
};

pub struct NanosheetBuilderPanel {
    pub original: Structure,
    pub spec: NanosheetSpec,
}

impl NanosheetBuilderPanel {
    pub fn new(current: &Structure) -> Self {
        Self {
            original: current.clone(),
            spec: NanosheetSpec::default(),
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        ui.label("Generate a periodic 2D material from a parametrized lattice family.");
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Name");
            ui.add_sized(
                [260.0, 20.0],
                egui::TextEdit::singleline(&mut self.spec.name),
            );
        });

        // Pick the lattice family first, then a preset within it: this keeps the
        // preset list short and meaningful as more materials are added.
        let mut family = self.spec.kind.family();
        combo_box(
            ui,
            "Type",
            &mut family,
            SheetFamily::all(),
            SheetFamily::label,
        );
        if family != self.spec.kind.family() {
            self.spec.kind = family.default_kind();
        }

        let presets = presets()
            .into_iter()
            .filter(|(_, kind)| kind.family() == family)
            .collect::<Vec<_>>();
        let current_label = presets
            .iter()
            .find(|(_, kind)| *kind == self.spec.kind)
            .map(|(label, _)| *label)
            .unwrap_or("Custom");
        ui.horizontal(|ui| {
            ui.label("Preset");
            egui::ComboBox::from_id_salt("nanosheet-preset")
                .selected_text(current_label)
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    for (label, kind) in &presets {
                        if ui
                            .selectable_label(current_label == *label, *label)
                            .clicked()
                        {
                            self.spec.kind = kind.clone();
                        }
                    }
                });
        });

        ui.separator();
        match &mut self.spec.kind {
            SheetKind::Honeycomb(params) => {
                element_edit(ui, "Sublattice A", &mut params.element_a);
                element_edit(ui, "Sublattice B", &mut params.element_b);
                labeled_drag(ui, "Lattice a (A)", &mut params.lattice_a);
                labeled_drag(ui, "Buckling (A)", &mut params.buckling);
                ui.label("Equal A/B gives graphene-type sheets; distinct A/B gives h-BN-type.");
            }
            SheetKind::Tmd(params) => {
                element_edit(ui, "Metal (M)", &mut params.metal);
                element_edit(ui, "Chalcogen (X)", &mut params.chalcogen);
                labeled_drag(ui, "Lattice a (A)", &mut params.lattice_a);
                labeled_drag(
                    ui,
                    "Chalcogen separation (A)",
                    &mut params.chalcogen_separation,
                );
                combo_box(
                    ui,
                    "Polytype",
                    &mut params.polytype,
                    &[TmdPolytype::H, TmdPolytype::T],
                    TmdPolytype::label,
                );
            }
            SheetKind::CarbonNitride(params) => {
                let previous = params.node;
                combo_box(
                    ui,
                    "Node",
                    &mut params.node,
                    &[CarbonNitrideNode::Triazine, CarbonNitrideNode::Heptazine],
                    CarbonNitrideNode::label,
                );
                if params.node != previous {
                    params.lattice_a = params.node.ideal_lattice_a();
                }
                labeled_drag(ui, "Lattice a (A)", &mut params.lattice_a);
            }
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Interlayer spacing (A)");
            drag_value(ui, &mut self.spec.interlayer_spacing);
        });
        ui.horizontal(|ui| {
            ui.label("Supercell");
            supercell_value(ui, &mut self.spec.supercell[0]);
            ui.label("x");
            supercell_value(ui, &mut self.spec.supercell[1]);
            ui.label("x");
            supercell_value(ui, &mut self.spec.supercell[2]);
        });
        ui.separator();
    }
}

fn element_edit(ui: &mut Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add_sized([80.0, 20.0], egui::TextEdit::singleline(value));
    });
}

fn labeled_drag(ui: &mut Ui, label: &str, value: &mut f32) {
    ui.horizontal(|ui| {
        ui.label(label);
        drag_value(ui, value);
    });
}
