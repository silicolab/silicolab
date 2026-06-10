use eframe::egui::{self, Grid};

use crate::{
    domain::chemistry::normalized_symbol,
    domain::{Atom, BondType, Structure, UnitCell},
    frontend::widgets::{cell_value, charge_value, drag_value},
};

pub struct StructureEditor {
    pub original: Structure,
    pub draft: Structure,
    pub selected_atom: Option<usize>,
    pub coordinate_mode: CoordinateMode,
    pub selected_bond: Option<usize>,
    pub add_bond_a: usize,
    pub add_bond_b: usize,
    pub add_bond_type: BondType,
    undo_stack: Vec<Structure>,
    redo_stack: Vec<Structure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateMode {
    Fractional,
    Cartesian,
}

impl StructureEditor {
    pub fn new(structure: &Structure) -> Self {
        Self {
            original: structure.clone(),
            draft: structure.clone(),
            selected_atom: None,
            coordinate_mode: if structure.cell.is_some() {
                CoordinateMode::Fractional
            } else {
                CoordinateMode::Cartesian
            },
            selected_bond: None,
            add_bond_a: 0,
            add_bond_b: if structure.atoms.len() > 1 { 1 } else { 0 },
            add_bond_type: BondType::Single,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut any_changed = false;
        let mut history_navigation = false;
        let before = self.draft.clone();

        if let Some(old_cell) = self.draft.cell.clone() {
            ui.heading("Lattice");
            let fractions = self
                .draft
                .atoms
                .iter()
                .map(|atom| old_cell.cartesian_to_fractional(atom.position))
                .collect::<Vec<_>>();
            let mut a = old_cell.a;
            let mut b = old_cell.b;
            let mut c = old_cell.c;
            let mut alpha = old_cell.alpha;
            let mut beta = old_cell.beta;
            let mut gamma = old_cell.gamma;
            let mut changed = false;

            Grid::new("cell_grid")
                .num_columns(2)
                .spacing([18.0, 6.0])
                .show(ui, |ui| {
                    changed |= cell_value(ui, "a", &mut a);
                    changed |= cell_value(ui, "b", &mut b);
                    changed |= cell_value(ui, "c", &mut c);
                    changed |= cell_value(ui, "alpha", &mut alpha);
                    changed |= cell_value(ui, "beta", &mut beta);
                    changed |= cell_value(ui, "gamma", &mut gamma);
                });

            if changed {
                let new_cell = UnitCell::from_parameters(a, b, c, alpha, beta, gamma);

                for (atom, frac) in self.draft.atoms.iter_mut().zip(fractions) {
                    atom.position = new_cell.fractional_to_cartesian(frac.x, frac.y, frac.z);
                }

                self.draft.cell = Some(new_cell);
                any_changed = true;
            }

            ui.separator();
        }

        ui.horizontal(|ui| {
            if self.draft.cell.is_some() {
                ui.label("Coordinate mode:");
                ui.radio_value(
                    &mut self.coordinate_mode,
                    CoordinateMode::Fractional,
                    "Fractional",
                );
                ui.radio_value(
                    &mut self.coordinate_mode,
                    CoordinateMode::Cartesian,
                    "Cartesian",
                );
                if ui
                    .button(format!("{}  Wrap into cell", egui_phosphor::regular::CUBE))
                    .clicked()
                {
                    self.draft.wrap_atoms_into_cell();
                    any_changed = true;
                }
            }

            if ui
                .button(format!("{}  Add Atom", egui_phosphor::regular::PLUS_CIRCLE))
                .clicked()
            {
                self.add_atom();
                any_changed = true;
            }

            let delete_enabled = self.selected_atom.is_some();
            if ui
                .add_enabled(
                    delete_enabled,
                    egui::Button::new(format!("{}  Delete", egui_phosphor::regular::TRASH)),
                )
                .clicked()
            {
                self.delete_selected_atom();
                any_changed = true;
            }

            if ui
                .add_enabled(
                    !self.undo_stack.is_empty(),
                    egui::Button::new(format!("{}  Undo", egui_phosphor::regular::ARROW_U_UP_LEFT)),
                )
                .clicked()
            {
                self.undo();
                any_changed = true;
                history_navigation = true;
            }

            if ui
                .add_enabled(
                    !self.redo_stack.is_empty(),
                    egui::Button::new(format!(
                        "{}  Redo",
                        egui_phosphor::regular::ARROW_U_UP_RIGHT
                    )),
                )
                .clicked()
            {
                self.redo();
                any_changed = true;
                history_navigation = true;
            }

            if ui
                .button(format!(
                    "{}  Recompute Bonds",
                    egui_phosphor::regular::ARROWS_CLOCKWISE
                ))
                .clicked()
            {
                self.draft.recompute_bonds();
                any_changed = true;
            }
        });

        ui.heading("Atoms");
        let coordinate_cell = self
            .uses_fractional_coordinates()
            .then(|| self.draft.cell.clone())
            .flatten();
        Grid::new("atoms_grid")
            .striped(true)
            .num_columns(6)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                ui.strong("#");
                ui.strong("Element");
                ui.strong("Charge");
                ui.strong(if self.uses_fractional_coordinates() {
                    "fract x"
                } else {
                    "x"
                });
                ui.strong(if self.uses_fractional_coordinates() {
                    "fract y"
                } else {
                    "y"
                });
                ui.strong(if self.uses_fractional_coordinates() {
                    "fract z"
                } else {
                    "z"
                });
                ui.end_row();

                for index in 0..self.draft.atoms.len() {
                    let selected = self.selected_atom == Some(index);
                    if ui
                        .selectable_label(selected, (index + 1).to_string())
                        .clicked()
                    {
                        self.selected_atom = Some(index);
                    }

                    let atom = &mut self.draft.atoms[index];
                    let mut element = atom.element.clone();
                    if ui
                        .add_sized([64.0, 20.0], egui::TextEdit::singleline(&mut element))
                        .changed()
                    {
                        atom.element = normalized_symbol(&element);
                        any_changed = true;
                    }
                    any_changed |= charge_value(ui, &mut atom.charge);

                    if let Some(cell) = &coordinate_cell {
                        let mut frac = cell.cartesian_to_fractional(atom.position);
                        let changed = drag_value(ui, &mut frac.x)
                            | drag_value(ui, &mut frac.y)
                            | drag_value(ui, &mut frac.z);
                        if changed {
                            atom.position = cell.fractional_to_cartesian(frac.x, frac.y, frac.z);
                            any_changed = true;
                        }
                    } else {
                        any_changed |= drag_value(ui, &mut atom.position.x);
                        any_changed |= drag_value(ui, &mut atom.position.y);
                        any_changed |= drag_value(ui, &mut atom.position.z);
                    }

                    ui.end_row();
                }
            });

        ui.separator();
        ui.heading("Bonds");

        if self.draft.bonds.is_empty() {
            ui.label("No bonds defined");
        } else {
            Grid::new("bonds_grid")
                .striped(true)
                .num_columns(5)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    ui.strong("#");
                    ui.strong("Atoms");
                    ui.strong("Type");
                    ui.strong("Change Type");
                    ui.strong("Delete");
                    ui.end_row();

                    let mut bond_to_remove = None;
                    let mut bond_type_change = None;

                    for index in 0..self.draft.bonds.len() {
                        let bond = &self.draft.bonds[index];
                        let selected = self.selected_bond == Some(index);

                        if ui
                            .selectable_label(selected, (index + 1).to_string())
                            .clicked()
                        {
                            self.selected_bond = Some(index);
                        }

                        let elem_a = self
                            .draft
                            .atoms
                            .get(bond.a)
                            .map(|a| a.element.as_str())
                            .unwrap_or("?");
                        let elem_b = self
                            .draft
                            .atoms
                            .get(bond.b)
                            .map(|a| a.element.as_str())
                            .unwrap_or("?");
                        ui.label(format!(
                            "{}({}) - {}({})",
                            bond.a + 1,
                            elem_a,
                            bond.b + 1,
                            elem_b
                        ));

                        ui.label(bond.bond_type.label());

                        egui::ComboBox::from_id_salt(format!("bond_type_{index}"))
                            .selected_text(bond.bond_type.label())
                            .show_ui(ui, |ui| {
                                for bt in BondType::all() {
                                    let label = bt.label();
                                    if ui.selectable_label(bond.bond_type == *bt, label).clicked() {
                                        bond_type_change = Some((index, *bt));
                                    }
                                }
                            });

                        if ui
                            .add(egui::Button::new(egui_phosphor::regular::TRASH).frame(false))
                            .on_hover_text("Delete this bond")
                            .clicked()
                        {
                            bond_to_remove = Some(index);
                        }

                        ui.end_row();
                    }

                    if let Some((idx, new_type)) = bond_type_change {
                        self.draft.set_bond_type(idx, new_type);
                        any_changed = true;
                    }

                    if let Some(idx) = bond_to_remove {
                        self.draft.remove_bond(idx);
                        if self.selected_bond == Some(idx) {
                            self.selected_bond = None;
                        } else if let Some(sel) = self.selected_bond
                            && sel > idx
                        {
                            self.selected_bond = Some(sel - 1);
                        }
                        any_changed = true;
                    }
                });
        }

        ui.separator();
        ui.heading("Add Bond");
        let atom_count = self.draft.atoms.len();
        if atom_count < 2 {
            ui.label("Need at least 2 atoms to add a bond");
        } else {
            ui.horizontal(|ui| {
                ui.label("Atom A:");
                ui.add(egui::DragValue::new(&mut self.add_bond_a).range(0..=atom_count - 1));
                ui.label("Atom B:");
                ui.add(egui::DragValue::new(&mut self.add_bond_b).range(0..=atom_count - 1));
                egui::ComboBox::from_id_salt("add_bond_type")
                    .selected_text(self.add_bond_type.label())
                    .show_ui(ui, |ui| {
                        for bt in BondType::all() {
                            let label = bt.label();
                            if ui
                                .selectable_label(self.add_bond_type == *bt, label)
                                .clicked()
                            {
                                self.add_bond_type = *bt;
                            }
                        }
                    });
                if ui.button("Add").clicked() {
                    self.draft
                        .add_bond(self.add_bond_a, self.add_bond_b, self.add_bond_type);
                    any_changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.style_mut().visuals.override_text_color = Some(egui::Color32::GRAY);
                if self.add_bond_a < atom_count && self.add_bond_b < atom_count {
                    let elem_a = &self.draft.atoms[self.add_bond_a].element;
                    let elem_b = &self.draft.atoms[self.add_bond_b].element;
                    ui.label(format!(
                        "Preview: {}({}) - {}({})",
                        self.add_bond_a + 1,
                        elem_a,
                        self.add_bond_b + 1,
                        elem_b
                    ));
                }
            });
        }

        if any_changed && !history_navigation {
            self.push_undo(before);
            self.redo_stack.clear();
        }
        any_changed
    }

    fn uses_fractional_coordinates(&self) -> bool {
        self.draft.cell.is_some() && self.coordinate_mode == CoordinateMode::Fractional
    }

    fn add_atom(&mut self) {
        let position = if let Some(cell) = &self.draft.cell {
            cell.fractional_to_cartesian(0.0, 0.0, 0.0)
        } else {
            self.draft.center()
        };

        self.draft.atoms.push(Atom {
            element: "C".to_string(),
            position,
            charge: 0.0,
        });
        self.selected_atom = Some(self.draft.atoms.len() - 1);
    }

    fn delete_selected_atom(&mut self) {
        let Some(index) = self.selected_atom else {
            return;
        };

        if index < self.draft.atoms.len() {
            self.draft.remove_bonds_for_atom(index);
            self.draft.atoms.remove(index);
            self.draft.adjust_bond_indices_after_removal(index);
            self.selected_atom = if self.draft.atoms.is_empty() {
                None
            } else {
                Some(index.min(self.draft.atoms.len() - 1))
            };
            self.selected_bond = None;
        }
    }

    fn push_undo(&mut self, structure: Structure) {
        self.undo_stack.push(structure);
        self.redo_stack.clear();
    }

    fn undo(&mut self) {
        let Some(previous) = self.undo_stack.pop() else {
            return;
        };

        self.redo_stack.push(self.draft.clone());
        self.draft = previous;
        self.selected_atom = self
            .selected_atom
            .filter(|index| *index < self.draft.atoms.len());
        self.selected_bond = self
            .selected_bond
            .filter(|index| *index < self.draft.bonds.len());
    }

    fn redo(&mut self) {
        let Some(next) = self.redo_stack.pop() else {
            return;
        };

        self.undo_stack.push(self.draft.clone());
        self.draft = next;
        self.selected_atom = self
            .selected_atom
            .filter(|index| *index < self.draft.atoms.len());
        self.selected_bond = self
            .selected_bond
            .filter(|index| *index < self.draft.bonds.len());
    }
}
