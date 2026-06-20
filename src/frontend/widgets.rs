use eframe::egui::{self, Align2, Color32, FontId, Grid, Pos2, Sense, Stroke, Vec2};
use nalgebra::{Point3, Vector3};

use crate::frontend::block_editor::BuildingBlockSite;
use crate::{
    domain::chemistry::element_style,
    domain::{BondType, Structure, UnitCell},
    frontend::AtomSelection,
    workflows::reticular::{
        ComponentSource, CoreSlot, FunctionalizationRule, ReticularBuildSpec, component_label,
        component_template, functional_group_options,
    },
};
const COMPONENT_PREVIEW_MIN_WIDTH: f32 = 180.0;
const COMPONENT_PREVIEW_CANVAS_HEIGHT: f32 = 160.0;
const COMPONENT_PREVIEW_ATOM_RADIUS: f32 = 6.0;
const COMPONENT_PREVIEW_ATOM_STROKE_WIDTH: f32 = 1.0;
const COMPONENT_PREVIEW_PORT_RADIUS: f32 = 8.0;
const COMPONENT_PREVIEW_PORT_STROKE_WIDTH: f32 = 2.0;

pub fn cell_value(ui: &mut egui::Ui, label: &str, value: &mut f32) -> bool {
    ui.label(label);
    let changed = drag_value(ui, value);
    ui.end_row();
    changed
}

pub fn drag_value(ui: &mut egui::Ui, value: &mut f32) -> bool {
    ui.add_sized(
        [104.0, 20.0],
        egui::DragValue::new(value).speed(0.01).max_decimals(6),
    )
    .changed()
}

pub fn charge_value(ui: &mut egui::Ui, value: &mut f32) -> bool {
    ui.add_sized(
        [72.0, 20.0],
        egui::DragValue::new(value).speed(0.01).max_decimals(4),
    )
    .changed()
}

pub fn supercell_value(ui: &mut egui::Ui, value: &mut u32) -> bool {
    ui.add_sized([44.0, 20.0], egui::DragValue::new(value).range(1..=6))
        .changed()
}

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
            Stroke::new(1.0, pal.hairline),
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
                            Stroke::new(1.5, Color32::from_rgb(80, 84, 90)),
                        );
                        painter.line_segment(
                            [start - perp, end - perp],
                            Stroke::new(1.5, Color32::from_rgb(80, 84, 90)),
                        );
                    }
                    BondType::Triple => {
                        let offset = 4.0;
                        let perp = perpendicular_offset(start, end, offset);
                        painter.line_segment(
                            [start, end],
                            Stroke::new(1.5, Color32::from_rgb(80, 84, 90)),
                        );
                        painter.line_segment(
                            [start + perp, end + perp],
                            Stroke::new(1.5, Color32::from_rgb(80, 84, 90)),
                        );
                        painter.line_segment(
                            [start - perp, end - perp],
                            Stroke::new(1.5, Color32::from_rgb(80, 84, 90)),
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
                            Stroke::new(2.0, Color32::from_rgb(80, 84, 90)),
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
                Stroke::new(1.4, port_color),
            );
            painter.line_segment(
                [pos + Vec2::new(0.0, -4.0), pos + Vec2::new(0.0, 4.0)],
                Stroke::new(1.4, port_color),
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
                    painter.line_segment([start, end], Stroke::new(1.0, port_color));
                }
            }
        }
    });
}

pub fn optional_component_label(
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

pub fn combo_box<T>(
    ui: &mut egui::Ui,
    label: &str,
    selected: &mut T,
    options: &[T],
    label_for: fn(T) -> &'static str,
) where
    T: Copy + PartialEq,
{
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(label)
            .selected_text(label_for(*selected))
            .show_ui(ui, |ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
                for option in options {
                    ui.selectable_value(selected, *option, label_for(*option));
                }
            });
    });
}

pub fn status_text(structure: &Structure, selection: &AtomSelection) -> String {
    let mut text = format!(
        "{} | {} atoms | {} bonds",
        if structure.title.is_empty() {
            "Untitled structure"
        } else {
            &structure.title
        },
        structure.atoms.len(),
        structure.bonds.len()
    );

    if let Some(cell) = &structure.cell {
        text.push_str(&format!(
            " | cell {:.2} {:.2} {:.2} A, {:.1} {:.1} {:.1} deg",
            cell.a, cell.b, cell.c, cell.alpha, cell.beta, cell.gamma
        ));
    }

    if !selection.is_empty() {
        text.push_str(&format!(" | selected {}", selection.len()));
    }

    text
}

pub fn bond_geometry_summary(structure: &Structure) -> String {
    if structure.bonds.is_empty() {
        return "no bonds detected".to_string();
    }

    let lengths = structure
        .bonds
        .iter()
        .map(|bond| bond_length(structure, bond.a, bond.b))
        .collect::<Vec<_>>();
    let min_length = lengths.iter().copied().fold(f32::INFINITY, f32::min);
    let max_length = lengths.iter().copied().fold(0.0_f32, f32::max);
    let avg_length = lengths.iter().sum::<f32>() / lengths.len() as f32;
    let angles = bond_angles(structure);

    if angles.is_empty() {
        return format!(
            "bond lengths {:.2}-{:.2} A, avg {:.2} A",
            min_length, max_length, avg_length
        );
    }

    let min_angle = angles.iter().copied().fold(f32::INFINITY, f32::min);
    let max_angle = angles.iter().copied().fold(0.0_f32, f32::max);
    let avg_angle = angles.iter().sum::<f32>() / angles.len() as f32;

    format!(
        "bond lengths {:.2}-{:.2} A, avg {:.2} A; angles {:.1}-{:.1} deg, avg {:.1} deg",
        min_length, max_length, avg_length, min_angle, max_angle, avg_angle
    )
}

pub fn bond_length(structure: &Structure, a: usize, b: usize) -> f32 {
    match &structure.cell {
        Some(cell) => periodic_delta(
            cell,
            structure.atoms[a].position,
            structure.atoms[b].position,
        )
        .norm(),
        None => nalgebra::distance(&structure.atoms[a].position, &structure.atoms[b].position),
    }
}

pub fn bond_angles(structure: &Structure) -> Vec<f32> {
    let mut neighbors = vec![Vec::new(); structure.atoms.len()];
    for bond in &structure.bonds {
        neighbors[bond.a].push(bond.b);
        neighbors[bond.b].push(bond.a);
    }

    let mut angles = Vec::new();
    for (center, bonded) in neighbors.iter().enumerate() {
        for i in 0..bonded.len() {
            for j in (i + 1)..bonded.len() {
                let first = delta_from_center(structure, center, bonded[i]);
                let second = delta_from_center(structure, center, bonded[j]);
                let denom = first.norm() * second.norm();
                if denom > 0.0001 {
                    let cosine = (first.dot(&second) / denom).clamp(-1.0, 1.0);
                    angles.push(cosine.acos().to_degrees());
                }
            }
        }
    }

    angles
}

fn delta_from_center(structure: &Structure, center: usize, other: usize) -> Vector3<f32> {
    match &structure.cell {
        Some(cell) => periodic_delta(
            cell,
            structure.atoms[center].position,
            structure.atoms[other].position,
        ),
        None => structure.atoms[other].position - structure.atoms[center].position,
    }
}

pub fn periodic_delta(cell: &UnitCell, first: Point3<f32>, second: Point3<f32>) -> Vector3<f32> {
    let first_frac = cell.cartesian_to_fractional(first);
    let second_frac = cell.cartesian_to_fractional(second);
    let mut delta = second_frac - first_frac;

    delta.x -= delta.x.round();
    delta.y -= delta.y.round();
    delta.z -= delta.z.round();

    cell.vectors[0] * delta.x + cell.vectors[1] * delta.y + cell.vectors[2] * delta.z
}

fn color32(point: Point3<f32>) -> Color32 {
    Color32::from_rgb(
        (point.x.clamp(0.0, 1.0) * 255.0) as u8,
        (point.y.clamp(0.0, 1.0) * 255.0) as u8,
        (point.z.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_aromatic_bond(
    painter: &egui::Painter,
    start: Pos2,
    end: Pos2,
    aromatic_center: Option<Pos2>,
    color: Color32,
    main_width: f32,
    accent_width: f32,
    offset: f32,
) {
    painter.line_segment([start, end], Stroke::new(main_width, color));
    let perp = inward_perpendicular_offset(start, end, offset, aromatic_center);
    draw_dashed_segment(
        painter,
        start + perp,
        end + perp,
        6.0,
        4.0,
        Stroke::new(accent_width, color),
    );
}

fn draw_dashed_segment(
    painter: &egui::Painter,
    start: Pos2,
    end: Pos2,
    dash_length: f32,
    gap_length: f32,
    stroke: Stroke,
) {
    let segment = end - start;
    let length = segment.length();
    if length <= f32::EPSILON {
        return;
    }

    let direction = segment / length;
    let mut cursor = 0.0;
    while cursor < length {
        let dash_end = (cursor + dash_length).min(length);
        painter.line_segment(
            [start + direction * cursor, start + direction * dash_end],
            stroke,
        );
        cursor += dash_length + gap_length;
    }
}

fn perpendicular_offset(start: Pos2, end: Pos2, offset: f32) -> Vec2 {
    let delta = end - start;
    if delta.length_sq() <= f32::EPSILON {
        Vec2::ZERO
    } else {
        Vec2::new(-delta.y, delta.x).normalized() * offset
    }
}

fn trimmed_segment(start: Pos2, end: Pos2, start_trim: f32, end_trim: f32) -> Option<(Pos2, Pos2)> {
    let delta = end - start;
    let length = delta.length();
    if length <= start_trim + end_trim || length <= f32::EPSILON {
        return None;
    }

    let direction = delta / length;
    Some((start + direction * start_trim, end - direction * end_trim))
}

fn inward_perpendicular_offset(
    start: Pos2,
    end: Pos2,
    offset: f32,
    aromatic_center: Option<Pos2>,
) -> Vec2 {
    let perp = perpendicular_offset(start, end, offset);
    let Some(center) = aromatic_center else {
        return perp;
    };

    let midpoint = start + (end - start) * 0.5;
    let center_delta = center - midpoint;
    if perp.dot(center_delta) >= 0.0 {
        perp
    } else {
        -perp
    }
}

fn aromatic_component_centers(
    bonds: &[(usize, usize, crate::domain::BondType)],
    atom_positions: &[Pos2],
) -> Vec<Option<Pos2>> {
    let mut aromatic_neighbors = vec![Vec::new(); atom_positions.len()];
    for &(a, b, _) in bonds
        .iter()
        .filter(|(_, _, bond_type)| *bond_type == BondType::Aromatic)
    {
        aromatic_neighbors[a].push(b);
        aromatic_neighbors[b].push(a);
    }

    let mut component_for_atom = vec![None; atom_positions.len()];
    let mut component_centers = Vec::new();

    for atom_index in 0..atom_positions.len() {
        if aromatic_neighbors[atom_index].is_empty() || component_for_atom[atom_index].is_some() {
            continue;
        }

        let mut stack = vec![atom_index];
        let mut component_atoms = Vec::new();
        component_for_atom[atom_index] = Some(component_centers.len());

        while let Some(current) = stack.pop() {
            component_atoms.push(current);
            for &neighbor in &aromatic_neighbors[current] {
                if component_for_atom[neighbor].is_none() {
                    component_for_atom[neighbor] = Some(component_centers.len());
                    stack.push(neighbor);
                }
            }
        }

        let sum = component_atoms.iter().fold(Vec2::ZERO, |acc, &index| {
            acc + atom_positions[index].to_vec2()
        });
        let center = sum / component_atoms.len() as f32;
        component_centers.push(Pos2::new(center.x, center.y));
    }

    bonds
        .iter()
        .map(|&(a, _, bond_type)| {
            if bond_type != BondType::Aromatic {
                None
            } else {
                component_for_atom[a].map(|component_index| component_centers[component_index])
            }
        })
        .collect()
}
