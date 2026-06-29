use super::*;

pub(crate) struct EntryListCtx<'a> {
    pub(crate) group_choices: &'a [(String, String)],
    pub(crate) ordered_items: &'a [SelectionItem],
}

pub(crate) fn render_entry_list_item(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    entry_id: u64,
    name: &str,
    group_id: &str,
    ctx: &EntryListCtx<'_>,
) {
    let is_workspace_active = state.entries.active_entry_id() == Some(entry_id);
    let is_selected = state.ui.entry_list.selected_entry_ids.contains(&entry_id);
    let renaming = state.ui.entry_list.renaming_entry_id == Some(entry_id);

    if renaming {
        let response = ui.add_sized(
            [ui.available_width(), 20.0],
            egui::TextEdit::singleline(&mut state.ui.entry_list.rename_buffer),
        );
        // Commit only on Enter; committing on any lost_focus would rename on an
        // incidental click-away.
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            state.ui.entry_list.renaming_entry_id = None;
        } else if response.lost_focus() {
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                actions.push(AppAction::RenameEntry {
                    entry_id,
                    new_name: state.ui.entry_list.rename_buffer.clone(),
                });
            }
            state.ui.entry_list.renaming_entry_id = None;
        }
    } else {
        let full_width = ui.available_width();
        let (rect, response) =
            ui.allocate_at_least(egui::vec2(full_width, 20.0), Sense::click_and_drag());

        let hovered = response.hovered();
        let pal = crate::frontend::theme::palette(ui);
        let bg_fill = if is_workspace_active {
            pal.blue_overlay(80)
        } else if is_selected {
            pal.blue_overlay(40)
        } else if hovered {
            pal.neutral_overlay(30)
        } else {
            egui::Color32::TRANSPARENT
        };
        let text_color = if is_workspace_active {
            pal.text_strong
        } else if is_selected {
            pal.text_primary
        } else {
            pal.text_muted
        };

        ui.painter().rect_filled(
            rect.shrink2(egui::vec2(4.0, 1.0)),
            f32::from(crate::frontend::theme::radius::CONTROL),
            bg_fill,
        );

        let text_rect = rect.shrink2(egui::vec2(6.0, 0.0));

        // A small chip marks entries produced by a run ("MD" or "QM"). Lay it
        // out first so the name reserves room. The QM chip is clickable: it
        // opens the run's saved output report.
        let chip_label = state.entries.entry(entry_id).and_then(|entry| {
            if entry.origin.is_md_run() {
                Some("MD")
            } else if entry.origin.is_qm_run() {
                Some("QM")
            } else {
                None
            }
        });
        let is_qm = chip_label == Some("QM");
        let chip = chip_label.map(|label| {
            let galley = ui.painter().fonts_mut(|fonts| {
                fonts.layout_no_wrap(
                    label.to_string(),
                    // 11pt floor (was 9) so the MD/QM origin chip stays legible.
                    egui::FontId::proportional(11.0),
                    pal.accent,
                )
            });
            let size = egui::vec2(galley.size().x + 8.0, galley.size().y + 3.0);
            (galley, size)
        });
        let name_reserve = chip.as_ref().map_or(0.0, |(_, size)| size.x + 6.0);

        let mut job = egui::text::LayoutJob::single_section(
            name.to_string(),
            egui::TextFormat {
                font_id: egui::FontId::proportional(13.0),
                color: text_color,
                ..Default::default()
            },
        );
        job.wrap = egui::text::TextWrapping {
            max_width: (text_rect.width() - name_reserve).max(10.0),
            max_rows: 1,
            overflow_character: Some('…'),
            break_anywhere: true,
        };
        let galley = ui.painter().fonts_mut(|f| f.layout_job(job));
        let galley_pos = egui::pos2(
            text_rect.left(),
            text_rect.center().y - galley.size().y / 2.0,
        );
        ui.painter().galley(galley_pos, galley, text_color);

        if let Some((chip_galley, chip_size)) = chip {
            let chip_rect = egui::Rect::from_min_size(
                egui::pos2(
                    text_rect.right() - chip_size.x,
                    text_rect.center().y - chip_size.y / 2.0,
                ),
                chip_size,
            );
            ui.painter().rect_filled(
                chip_rect,
                f32::from(crate::frontend::theme::radius::CHIP),
                pal.blue_overlay(45),
            );
            let chip_pos = egui::pos2(
                chip_rect.center().x - chip_galley.size().x / 2.0,
                chip_rect.center().y - chip_galley.size().y / 2.0,
            );
            ui.painter().galley(chip_pos, chip_galley, pal.accent);

            // The QM chip doubles as a button: it opens the run's saved output
            // report. Registered after the row, so it wins the overlap.
            if is_qm {
                let chip_response = ui
                    .interact(
                        chip_rect,
                        ui.id().with(("qm-output-chip", entry_id)),
                        Sense::click(),
                    )
                    .on_hover_text("View QM output");
                if chip_response.clicked() {
                    actions.push(AppAction::ShowQmOutput(entry_id));
                }
            }
        }

        if response.double_clicked() {
            actions.push(AppAction::ActivateEntry(entry_id));
            state.ui.entry_list.selected_entry_ids.insert(entry_id);
        } else if response.clicked() {
            let shift = ui.input(|i| i.modifiers.shift);
            let ctrl = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);

            let this = SelectionItem::Entry(entry_id);
            if shift {
                let anchor_pos = state
                    .ui
                    .entry_list
                    .selection_anchor
                    .as_ref()
                    .and_then(|a| ctx.ordered_items.iter().position(|item| item == a));
                let current_pos = ctx.ordered_items.iter().position(|item| item == &this);
                if let (Some(a), Some(b)) = (anchor_pos, current_pos) {
                    let (lo, hi) = (a.min(b), a.max(b));
                    state.ui.entry_list.selected_entry_ids.clear();
                    state.ui.entry_list.selected_group_ids.clear();
                    for item in &ctx.ordered_items[lo..=hi] {
                        match item {
                            SelectionItem::Entry(id) => {
                                state.ui.entry_list.selected_entry_ids.insert(*id);
                            }
                            SelectionItem::Group(id) => {
                                state.ui.entry_list.selected_group_ids.insert(id.clone());
                            }
                        }
                    }
                } else {
                    state.ui.entry_list.selected_entry_ids.clear();
                    state.ui.entry_list.selected_group_ids.clear();
                    state.ui.entry_list.selected_entry_ids.insert(entry_id);
                    state.ui.entry_list.selection_anchor = Some(this);
                }
            } else if ctrl {
                if !state.ui.entry_list.selected_entry_ids.remove(&entry_id) {
                    state.ui.entry_list.selected_entry_ids.insert(entry_id);
                }
                state.ui.entry_list.selection_anchor = Some(this);
            } else {
                state.ui.entry_list.selected_entry_ids.clear();
                state.ui.entry_list.selected_group_ids.clear();
                state.ui.entry_list.selected_entry_ids.insert(entry_id);
                state.ui.entry_list.selection_anchor = Some(this);
            }
        }

        let sel_entry_ids: Vec<u64> = state
            .ui
            .entry_list
            .selected_entry_ids
            .iter()
            .copied()
            .collect();
        let sel_group_ids: Vec<String> = state
            .ui
            .entry_list
            .selected_group_ids
            .iter()
            .cloned()
            .collect();
        response.context_menu(|ui| {
            if ui.button("Rename").clicked() {
                state.ui.entry_list.renaming_entry_id = Some(entry_id);
                state.ui.entry_list.rename_buffer = name.to_string();
                ui.close();
            }
            if !group_id.is_empty() && ui.button("Remove from group").clicked() {
                actions.push(AppAction::MoveEntryToGroup {
                    entry_id,
                    group_id: String::new(),
                });
                ui.close();
            }
            if !ctx.group_choices.is_empty() {
                ui.separator();
                ui.label("Move to group");
                for (target_group_id, target_group_name) in ctx.group_choices {
                    if target_group_id == group_id {
                        continue;
                    }
                    if ui.button(target_group_name).clicked() {
                        actions.push(AppAction::MoveEntryToGroup {
                            entry_id,
                            group_id: target_group_id.clone(),
                        });
                        ui.close();
                    }
                }
            }
            ui.separator();
            render_delete_menu_items(
                state,
                ui,
                actions,
                &sel_entry_ids,
                &sel_group_ids,
                Some(entry_id),
                None,
            );
        });
    }
    ui.add_space(2.0);
}
