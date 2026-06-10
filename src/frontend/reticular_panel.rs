use eframe::egui::{self, Ui};

use crate::{
    domain::Structure,
    frontend::widgets::{
        combo_box, component_preview, component_source_combo_box, drag_value,
        functionalization_selector, preview_substitutions_for, supercell_value,
    },
    workflows::reticular::{
        ComponentSource, CoreSlot, LinkerDirection, NetworkId, ReticularBuildSpec,
        core_options_for, linker_options, network_options, network_template,
    },
};

const PREVIEW_TARGET_WIDTH: f32 = 240.0;
const PREVIEW_MIN_WIDTH: f32 = 180.0;

struct PreviewItem {
    title: String,
    component_id: ComponentSource,
    substitutions: Vec<(usize, ComponentSource)>,
}

pub struct ReticularBuilderPanel {
    pub original: Structure,
    pub spec: ReticularBuildSpec,
}

impl ReticularBuilderPanel {
    pub fn new(current: &Structure) -> Self {
        Self {
            original: current.clone(),
            spec: ReticularBuildSpec::default(),
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        ui.label("Build a reticular structure from cores, linkers, and functional groups.");
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Name");
            ui.add_sized(
                [260.0, 20.0],
                egui::TextEdit::singleline(&mut self.spec.name),
            );
        });

        combo_box(
            ui,
            "Network",
            &mut self.spec.network,
            network_options(),
            NetworkId::label,
        );

        let network = network_template(self.spec.network);
        ui.horizontal(|ui| {
            if ui.button("Import Building Block...").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("SLF", &["slf"])
                    .pick_file()
            {
                match std::fs::read_to_string(&path) {
                    Ok(source) => self.spec.custom_components.push(source),
                    Err(error) => {
                        ui.label(format!("Import failed: {error}"));
                    }
                }
            }
            ui.label(format!(
                "{} custom blocks loaded",
                self.spec.custom_components.len()
            ));
        });

        let primary_options =
            core_options_for(network.primary_connectivity, &self.spec.custom_components);
        if !primary_options.contains(&self.spec.primary)
            && let Some(first) = primary_options.first()
        {
            self.spec.primary = *first;
        }
        component_source_combo_box(
            ui,
            network.primary_label,
            &mut self.spec.primary,
            &primary_options,
            &self.spec.custom_components,
        );

        if let (Some(label), Some(connectivity)) =
            (network.secondary_label, network.secondary_connectivity)
        {
            let secondary_options = core_options_for(connectivity, &self.spec.custom_components);
            if !secondary_options.contains(&self.spec.secondary)
                && let Some(first) = secondary_options.first()
            {
                self.spec.secondary = *first;
            }
            component_source_combo_box(
                ui,
                label,
                &mut self.spec.secondary,
                &secondary_options,
                &self.spec.custom_components,
            );
        }
        let linker_options = linker_options(&self.spec.custom_components)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        self.spec
            .linkers
            .retain(|linker| linker_options.contains(linker));
        ui.horizontal(|ui| {
            ui.label("Linkers");
            if ui.button(egui_phosphor::regular::PLUS).clicked()
                && let Some(first) = linker_options.first()
            {
                self.spec.linkers.push(*first);
            }
            if ui.button(egui_phosphor::regular::MINUS).clicked() {
                self.spec.linkers.pop();
            }
            ui.label(format!("{} in chain", self.spec.linkers.len()));
        });
        let mut remove_linker = None;
        for (index, linker) in self.spec.linkers.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                component_source_combo_box(
                    ui,
                    &format!("Linker {}", index + 1),
                    linker,
                    &linker_options,
                    &self.spec.custom_components,
                );
                if ui
                    .button(egui_phosphor::regular::TRASH)
                    .on_hover_text("Remove linker")
                    .clicked()
                {
                    remove_linker = Some(index);
                }
            });
        }
        if let Some(index) = remove_linker {
            self.spec.linkers.remove(index);
        }
        if !self.spec.linkers.is_empty() {
            combo_box(
                ui,
                "Linker direction",
                &mut self.spec.linker_direction,
                linker_direction_options(),
                LinkerDirection::label,
            );
        }
        ui.horizontal(|ui| {
            ui.label("Orientational modulation");
            ui.checkbox(
                &mut self.spec.modulate_primary_orientation,
                network.primary_label,
            );
            if let Some(label) = network.secondary_label {
                ui.checkbox(&mut self.spec.modulate_secondary_orientation, label);
            }
        });
        normalize_stacking_period(&mut self.spec);

        ui.horizontal(|ui| {
            ui.label("Preview supercell");
            supercell_value(ui, &mut self.spec.supercell[0]);
            ui.label("x");
            supercell_value(ui, &mut self.spec.supercell[1]);
            ui.label("x");
            supercell_value(ui, &mut self.spec.supercell[2]);
            normalize_stacking_period(&mut self.spec);
        });
        ui.horizontal(|ui| {
            ui.label("Layer spacing");
            drag_value(ui, &mut self.spec.layer_spacing);
        });

        ui.separator();
        let mut preview_items = vec![PreviewItem {
            title: network.primary_label.to_string(),
            component_id: self.spec.primary,
            substitutions: preview_substitutions_for(CoreSlot::Primary, &self.spec),
        }];
        if let Some(label) = network.secondary_label {
            preview_items.push(PreviewItem {
                title: label.to_string(),
                component_id: self.spec.secondary,
                substitutions: preview_substitutions_for(CoreSlot::Secondary, &self.spec),
            });
        }
        for (index, linker) in self.spec.linkers.iter().copied().enumerate() {
            preview_items.push(PreviewItem {
                title: format!("Linker {}", index + 1),
                component_id: linker,
                substitutions: preview_substitutions_for(CoreSlot::Linker(index), &self.spec),
            });
        }

        let spacing = ui.spacing().item_spacing.x;
        let available_width = ui.available_width().max(PREVIEW_MIN_WIDTH);
        let column_count = preview_column_count(available_width, spacing);
        let item_width = preview_item_width(available_width, column_count, spacing);
        for row in preview_items.chunks(column_count) {
            ui.horizontal_top(|ui| {
                for item in row {
                    ui.allocate_ui_with_layout(
                        egui::vec2(item_width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            component_preview(
                                ui,
                                &item.title,
                                item.component_id,
                                &item.substitutions,
                                &self.spec.custom_components,
                                item_width,
                            );
                        },
                    );
                }
            });
        }

        ui.separator();
        functionalization_selector(ui, &mut self.spec);
        ui.separator();
    }
}

pub fn linker_direction_options() -> &'static [LinkerDirection] {
    &[
        LinkerDirection::PrimaryToSecondary,
        LinkerDirection::SecondaryToPrimary,
    ]
}

fn normalize_stacking_period(spec: &mut ReticularBuildSpec) {
    let period = spec.stacking_period();
    if period <= 1 {
        return;
    }

    let layers = spec.supercell[2].max(1);
    spec.supercell[2] = layers.div_ceil(period) * period;
}

fn preview_column_count(available_width: f32, spacing: f32) -> usize {
    ((available_width + spacing) / (PREVIEW_TARGET_WIDTH + spacing))
        .floor()
        .max(1.0) as usize
}

fn preview_item_width(available_width: f32, column_count: usize, spacing: f32) -> f32 {
    let total_spacing = spacing * column_count.saturating_sub(1) as f32;
    ((available_width - total_spacing) / column_count as f32).max(PREVIEW_MIN_WIDTH)
}
