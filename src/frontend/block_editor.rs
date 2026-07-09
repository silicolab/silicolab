use std::path::PathBuf;

use anyhow::Result;
use eframe::egui::{self, Grid};

use crate::{
    domain::Structure,
    frontend::widgets::{atom_index_combo, combo_box, default_substitution_site},
    io::formats::slf::{SlfReticular, SlfSubstitutionSite, to_slf},
};

pub struct BuildingBlockEditor {
    pub label: String,
    pub class: BuildingBlockClass,
    pub sites: Vec<BuildingBlockSite>,
    pub pick_target: Option<BlockPickTarget>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BuildingBlockClass {
    Core,
    Linker,
    FunctionalGroup,
}

impl BuildingBlockClass {
    pub fn options() -> &'static [Self] {
        &[Self::Core, Self::Linker, Self::FunctionalGroup]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Core => "Core",
            Self::Linker => "Linker",
            Self::FunctionalGroup => "Functional group",
        }
    }

    pub fn metadata_value(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Linker => "linker",
            Self::FunctionalGroup => "functional_group",
        }
    }
}

#[derive(Clone)]
pub struct BuildingBlockSite {
    pub leaving_atom: usize,
    pub binding_atom: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BlockPickTarget {
    Leaving(usize),
    Binding(usize),
}

impl BuildingBlockEditor {
    pub fn new(structure: &Structure) -> Self {
        let first_site = default_substitution_site(structure).unwrap_or(BuildingBlockSite {
            leaving_atom: 0,
            binding_atom: 0,
        });

        Self {
            label: structure.title.clone(),
            class: BuildingBlockClass::Core,
            sites: vec![first_site],
            pick_target: None,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, structure: &Structure) {
        ui.label(format!(
            "{} atoms | {} bonds",
            structure.atoms.len(),
            structure.bonds.len()
        ));
        ui.horizontal(|ui| {
            ui.label("Label");
            ui.add_sized([320.0, 20.0], egui::TextEdit::singleline(&mut self.label));
        });
        combo_box(
            ui,
            "Class",
            &mut self.class,
            BuildingBlockClass::options(),
            BuildingBlockClass::label,
        );

        ui.separator();
        if let Some(target) = self.pick_target {
            ui.label(match target {
                BlockPickTarget::Leaving(row) => {
                    format!("Picking leaving atom for site {}", row + 1)
                }
                BlockPickTarget::Binding(row) => {
                    format!("Picking binding atom for site {}", row + 1)
                }
            });
        }
        ui.label("Substitution sites");
        Grid::new("building_block_sites")
            .striped(true)
            .num_columns(6)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.strong("#");
                ui.strong("Leaving atom");
                ui.strong("");
                ui.strong("Binding atom");
                ui.strong("");
                ui.strong("");
                ui.end_row();

                let mut remove = None;
                for (index, site) in self.sites.iter_mut().enumerate() {
                    ui.label((index + 1).to_string());
                    atom_index_combo(ui, ("leaving", index), &mut site.leaving_atom, structure);
                    if ui
                        .scope(|ui| {
                            crate::frontend::theme::stabilize_selectable_rows(ui);
                            ui.selectable_label(
                                self.pick_target == Some(BlockPickTarget::Leaving(index)),
                                "Pick",
                            )
                        })
                        .inner
                        .clicked()
                    {
                        self.pick_target = Some(BlockPickTarget::Leaving(index));
                    }
                    atom_index_combo(ui, ("binding", index), &mut site.binding_atom, structure);
                    if ui
                        .scope(|ui| {
                            crate::frontend::theme::stabilize_selectable_rows(ui);
                            ui.selectable_label(
                                self.pick_target == Some(BlockPickTarget::Binding(index)),
                                "Pick",
                            )
                        })
                        .inner
                        .clicked()
                    {
                        self.pick_target = Some(BlockPickTarget::Binding(index));
                    }
                    if ui
                        .button(egui_phosphor::regular::TRASH)
                        .on_hover_text("Remove site")
                        .clicked()
                    {
                        remove = Some(index);
                    }
                    ui.end_row();
                }

                if let Some(index) = remove {
                    self.sites.remove(index);
                    self.pick_target = None;
                }
            });

        if ui
            .button(format!("{}  Add Site", egui_phosphor::regular::PLUS_CIRCLE))
            .clicked()
        {
            self.sites.push(
                default_substitution_site(structure).unwrap_or(BuildingBlockSite {
                    leaving_atom: 0,
                    binding_atom: 0,
                }),
            );
        }
    }

    pub fn apply_picked_atom(&mut self, atom_index: usize) -> bool {
        let Some(target) = self.pick_target.take() else {
            return false;
        };

        match target {
            BlockPickTarget::Leaving(row) => {
                if let Some(site) = self.sites.get_mut(row) {
                    site.leaving_atom = atom_index;
                    true
                } else {
                    false
                }
            }
            BlockPickTarget::Binding(row) => {
                if let Some(site) = self.sites.get_mut(row) {
                    site.binding_atom = atom_index;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn save(&self, structure: &Structure) -> Result<(PathBuf, String)> {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SLF", &["slf"])
            .set_file_name("building_block.slf")
            .save_file()
        else {
            anyhow::bail!("save canceled");
        };
        let reticular = SlfReticular {
            class: self.class.metadata_value().to_string(),
            label: Some(self.label.clone()),
            substitution_sites: self
                .sites
                .iter()
                .filter(|site| {
                    site.leaving_atom < structure.atoms.len()
                        && site.binding_atom < structure.atoms.len()
                })
                .map(|site| SlfSubstitutionSite {
                    leaving_atom: site.leaving_atom,
                    binding_atom: site.binding_atom,
                })
                .collect(),
        };
        let source = to_slf(structure, Some(&reticular));
        std::fs::write(&path, &source)?;

        Ok((path, source))
    }
}
