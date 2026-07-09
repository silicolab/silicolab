use eframe::egui::{self, Align2, Color32, FontId, Grid, Pos2, Sense, Stroke, Vec2};

use crate::frontend::block_editor::BuildingBlockSite;
use crate::{
    domain::chemistry::element_style,
    domain::{BondType, Structure},
    workflows::reticular::{
        ComponentSource, CoreSlot, FunctionalizationRule, ReticularBuildSpec, component_label,
        component_template, functional_group_options,
    },
};

use super::geometry::{
    aromatic_component_centers, color32, draw_aromatic_bond, perpendicular_offset, trimmed_segment,
};

const COMPONENT_PREVIEW_MIN_WIDTH: f32 = 180.0;
const COMPONENT_PREVIEW_CANVAS_HEIGHT: f32 = 160.0;
const COMPONENT_PREVIEW_ATOM_RADIUS: f32 = 6.0;
const COMPONENT_PREVIEW_ATOM_STROKE_WIDTH: f32 = 1.0;
const COMPONENT_PREVIEW_PORT_RADIUS: f32 = 8.0;
const COMPONENT_PREVIEW_PORT_STROKE_WIDTH: f32 = 2.0;

pub fn component_preview(
    ui: &mut egui::Ui,
    title: &str,
    component_id: ComponentSource,
    substitutions: &[(usize, ComponentSource)],
    custom_components: &[String],
    preview_width: f32,
) {
    let mut component = component_template(component_id, custom_components);

    for (atom_index, group_id) in substitutions {
        let group = component_template(*group_id, custom_components);
        if let (Some(atom), Some(replacement)) =
            (component.atoms.get_mut(*atom_index), group.atoms.first())
        {
            atom.element = replacement.element.clone();
        }
    }

    let preview_width = preview_width.max(COMPONENT_PREVIEW_MIN_WIDTH);
    ui.set_max_width(preview_width);
    ui.set_width(preview_width);

    ui.vertical(|ui| {
        ui.label(format!("{title}: {}", component.label));
        ui.label(format!("{} connection points", component.connectivity));

        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(preview_width, COMPONENT_PREVIEW_CANVAS_HEIGHT),
            Sense::hover(),
        );
        let pal = crate::frontend::theme::palette(ui);
        let canvas_radius = f32::from(crate::frontend::theme::radius::CONTROL);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, canvas_radius, pal.item_fill);
        painter.rect_stroke(
            rect,
            canvas_radius,
            Stroke::new(1.0_f32, pal.hairline),
            egui::StrokeKind::Inside,
        );

        let max_radius = component
            .atoms
            .iter()
            .map(|atom| atom.position.coords.norm())
            .chain(
                component
                    .coordination_sites
                    .iter()
                    .map(|site| site.coordination_position.norm()),
            )
            .fold(1.0_f32, f32::max);
        let scale = rect.width().min(rect.height()) * 0.42 / max_radius;
        let center = rect.center();
        let atom_positions = component
            .atoms
            .iter()
            .map(|atom| {
                Pos2::new(
                    center.x + atom.position.x * scale,
                    center.y - atom.position.y * scale,
                )
            })
            .collect::<Vec<_>>();
        let aromatic_bonds = component
            .bonds
            .iter()
            .map(|bond| (bond.a, bond.b, bond.bond_type))
            .collect::<Vec<_>>();
        let aromatic_centers = aromatic_component_centers(&aromatic_bonds, &atom_positions);

        for (bond_index, bond) in component.bonds.iter().enumerate() {
            if let (Some(first), Some(second)) =
                (atom_positions.get(bond.a), atom_positions.get(bond.b))
            {
                let start = *first;
                let end = *second;

                match bond.bond_type {
                    BondType::Double => {
                        let offset = 3.0;
                        let perp = perpendicular_offset(start, end, offset);
                        painter.line_segment(
                            [start + perp, end + perp],
                            Stroke::new(1.5_f32, Color32::from_rgb(80, 84, 90)),
                        );
                        painter.line_segment(
                            [start - perp, end - perp],
                            Stroke::new(1.5_f32, Color32::from_rgb(80, 84, 90)),
                        );
                    }
                    BondType::Triple => {
                        let offset = 4.0;
                        let perp = perpendicular_offset(start, end, offset);
                        painter.line_segment(
                            [start, end],
                            Stroke::new(1.5_f32, Color32::from_rgb(80, 84, 90)),
                        );
                        painter.line_segment(
                            [start + perp, end + perp],
                            Stroke::new(1.5_f32, Color32::from_rgb(80, 84, 90)),
                        );
                        painter.line_segment(
                            [start - perp, end - perp],
                            Stroke::new(1.5_f32, Color32::from_rgb(80, 84, 90)),
                        );
                    }
                    BondType::Aromatic => {
                        draw_aromatic_bond(
                            &painter,
                            start,
                            end,
                            aromatic_centers[bond_index],
                            Color32::from_rgb(80, 84, 90),
                            2.0,
                            1.3,
                            5.0,
                        );
                    }
                    _ => {
                        painter.line_segment(
                            [start, end],
                            Stroke::new(2.0_f32, Color32::from_rgb(80, 84, 90)),
                        );
                    }
                }
            }
        }

        let mut hydrogen_count = 0;
        for atom in &component.atoms {
            let pos = Pos2::new(
                center.x + atom.position.x * scale,
                center.y - atom.position.y * scale,
            );
            let style = element_style(&atom.element);

            painter.circle_filled(pos, COMPONENT_PREVIEW_ATOM_RADIUS, color32(style.color));
            painter.circle_stroke(
                pos,
                COMPONENT_PREVIEW_ATOM_RADIUS,
                Stroke::new(
                    COMPONENT_PREVIEW_ATOM_STROKE_WIDTH,
                    Color32::from_rgb(40, 44, 48),
                ),
            );
            if atom.element == "H" {
                hydrogen_count += 1;
                painter.text(
                    pos,
                    Align2::CENTER_CENTER,
                    hydrogen_count.to_string(),
                    FontId::monospace(8.0),
                    Color32::from_rgb(40, 44, 48),
                );
            }
        }

        for site in &component.coordination_sites {
            let pos = Pos2::new(
                center.x + site.coordination_position.x * scale,
                center.y - site.coordination_position.y * scale,
            );
            let port_color = Color32::from_rgb(210, 40, 40);

            painter.circle_stroke(
                pos,
                COMPONENT_PREVIEW_PORT_RADIUS,
                Stroke::new(COMPONENT_PREVIEW_PORT_STROKE_WIDTH, port_color),
            );
            painter.line_segment(
                [pos + Vec2::new(-4.0, 0.0), pos + Vec2::new(4.0, 0.0)],
                Stroke::new(1.4_f32, port_color),
            );
            painter.line_segment(
                [pos + Vec2::new(0.0, -4.0), pos + Vec2::new(0.0, 4.0)],
                Stroke::new(1.4_f32, port_color),
            );

            if let Some(atom) = component.atoms.get(site.binding_atom) {
                let atom_pos = Pos2::new(
                    center.x + atom.position.x * scale,
                    center.y - atom.position.y * scale,
                );
                if let Some((start, end)) = trimmed_segment(
                    atom_pos,
                    pos,
                    COMPONENT_PREVIEW_ATOM_RADIUS + COMPONENT_PREVIEW_ATOM_STROKE_WIDTH * 0.5,
                    COMPONENT_PREVIEW_PORT_RADIUS + COMPONENT_PREVIEW_PORT_STROKE_WIDTH * 0.5,
                ) {
                    painter.line_segment([start, end], Stroke::new(1.0_f32, port_color));
                }
            }
        }
    });
}

fn optional_component_label(
    option: Option<ComponentSource>,
    custom_components: &[String],
) -> String {
    option
        .map(|source| component_label(source, custom_components))
        .unwrap_or_else(|| "None".to_string())
}

pub fn component_source_combo_box(
    ui: &mut egui::Ui,
    label: &str,
    selected: &mut ComponentSource,
    options: &[ComponentSource],
    custom_components: &[String],
) {
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(label)
            .selected_text(component_label(*selected, custom_components))
            .show_ui(ui, |ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
                for option in options {
                    ui.selectable_value(
                        selected,
                        *option,
                        component_label(*option, custom_components),
                    );
                }
            });
    });
}

pub fn functionalization_selector(ui: &mut egui::Ui, spec: &mut ReticularBuildSpec) {
    ui.checkbox(&mut spec.functionalization_enabled, "Functionalization");

    if !spec.functionalization_enabled {
        return;
    }

    sync_functionalization_rules(spec);
    let primary = spec.primary;
    let secondary = spec.secondary;
    let linkers = spec.linkers.clone();

    Grid::new("functionalization_grid")
        .num_columns(3)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            ui.strong("Core");
            ui.strong("H site");
            ui.strong("Replacement");
            ui.end_row();

            for rule in &mut spec.functionalizations {
                ui.label(rule.slot.label());
                ui.label(hydrogen_site_label(
                    rule.slot,
                    rule.atom_index,
                    primary,
                    secondary,
                    &linkers,
                    &spec.custom_components,
                ));
                optional_component_combo_box_with_id(
                    ui,
                    egui::Id::new(("functionalization", rule.slot, rule.atom_index)),
                    &mut rule.group,
                    &functional_group_options(&spec.custom_components),
                    &spec.custom_components,
                );
                ui.end_row();
            }
        });
}

fn sync_functionalization_rules(spec: &mut ReticularBuildSpec) {
    let existing = spec.functionalizations.clone();
    let mut rules = Vec::new();

    append_functionalization_rules(
        &mut rules,
        &existing,
        CoreSlot::Primary,
        spec.primary,
        &spec.custom_components,
    );
    append_functionalization_rules(
        &mut rules,
        &existing,
        CoreSlot::Secondary,
        spec.secondary,
        &spec.custom_components,
    );
    for (index, linker) in spec.linkers.iter().copied().enumerate() {
        append_functionalization_rules(
            &mut rules,
            &existing,
            CoreSlot::Linker(index),
            linker,
            &spec.custom_components,
        );
    }

    spec.functionalizations = rules;
}

fn append_functionalization_rules(
    rules: &mut Vec<FunctionalizationRule>,
    existing: &[FunctionalizationRule],
    slot: CoreSlot,
    component_id: ComponentSource,
    custom_components: &[String],
) {
    let component = component_template(component_id, custom_components);

    for (atom_index, atom) in component.atoms.iter().enumerate() {
        if atom.element != "H" {
            continue;
        }

        let group = existing
            .iter()
            .find(|rule| rule.slot == slot && rule.atom_index == atom_index)
            .and_then(|rule| rule.group);

        rules.push(FunctionalizationRule {
            slot,
            atom_index,
            group,
        });
    }
}

pub fn preview_substitutions_for(
    slot: CoreSlot,
    spec: &ReticularBuildSpec,
) -> Vec<(usize, ComponentSource)> {
    if !spec.functionalization_enabled {
        return Vec::new();
    }

    spec.functionalizations
        .iter()
        .filter(|rule| rule.slot == slot)
        .filter_map(|rule| rule.group.map(|group| (rule.atom_index, group)))
        .collect()
}

fn hydrogen_site_label(
    slot: CoreSlot,
    atom_index: usize,
    primary: ComponentSource,
    secondary: ComponentSource,
    linkers: &[ComponentSource],
    custom_components: &[String],
) -> String {
    let component_source = match slot {
        CoreSlot::Primary => primary,
        CoreSlot::Secondary => secondary,
        CoreSlot::Linker(index) => linkers.get(index).copied().unwrap_or(primary),
    };
    let component = component_template(component_source, custom_components);
    let mut hydrogen_count = 0;

    for (index, atom) in component.atoms.iter().enumerate() {
        if atom.element == "H" {
            hydrogen_count += 1;
        }
        if index == atom_index {
            return if atom.element == "H" {
                hydrogen_count.to_string()
            } else {
                format!("atom {}", atom_index + 1)
            };
        }
    }

    format!("atom {}", atom_index + 1)
}

fn optional_component_combo_box_with_id(
    ui: &mut egui::Ui,
    id: egui::Id,
    selected: &mut Option<ComponentSource>,
    options: &[Option<ComponentSource>],
    custom_components: &[String],
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(optional_component_label(*selected, custom_components))
        .show_ui(ui, |ui| {
            crate::frontend::theme::stabilize_selectable_rows(ui);
            for option in options {
                ui.selectable_value(
                    selected,
                    *option,
                    optional_component_label(*option, custom_components),
                );
            }
        });
}

pub fn atom_index_combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    selected: &mut usize,
    structure: &Structure,
) {
    if structure.atoms.is_empty() {
        ui.label("none");
        *selected = 0;
        return;
    }

    *selected = (*selected).min(structure.atoms.len() - 1);
    egui::ComboBox::from_id_salt(id)
        .selected_text(atom_choice_label(structure, *selected))
        .show_ui(ui, |ui| {
            crate::frontend::theme::stabilize_selectable_rows(ui);
            for index in 0..structure.atoms.len() {
                ui.selectable_value(selected, index, atom_choice_label(structure, index));
            }
        });
}

fn atom_choice_label(structure: &Structure, index: usize) -> String {
    let atom = &structure.atoms[index];

    format!("{} {}", index + 1, atom.element)
}

pub fn default_substitution_site(structure: &Structure) -> Option<BuildingBlockSite> {
    structure.bonds.iter().find_map(|bond| {
        let first = &structure.atoms[bond.a];
        let second = &structure.atoms[bond.b];

        if first.element == "H" && second.element != "H" {
            Some(BuildingBlockSite {
                leaving_atom: bond.a,
                binding_atom: bond.b,
            })
        } else if second.element == "H" && first.element != "H" {
            Some(BuildingBlockSite {
                leaving_atom: bond.b,
                binding_atom: bond.a,
            })
        } else {
            None
        }
    })
}
