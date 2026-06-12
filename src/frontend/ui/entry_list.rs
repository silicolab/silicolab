use super::*;

pub(crate) fn render_entry_list(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if with_core_button_style(ui, false, |ui| {
                ui.add_sized(
                    [24.0, 24.0],
                    Button::new(
                        RichText::new(egui_phosphor::regular::FILE_PLUS)
                            .size(13.0)
                            .color(core_button_text_color(&pal, false)),
                    )
                    .frame(false),
                )
            })
            .on_hover_text("New Entry")
            .clicked()
            {
                actions.push(AppAction::NewEmptyEntry);
            }
            if with_core_button_style(ui, false, |ui| {
                ui.add_sized(
                    [24.0, 24.0],
                    Button::new(
                        RichText::new(egui_phosphor::regular::FOLDER_PLUS)
                            .size(13.0)
                            .color(core_button_text_color(&pal, false)),
                    )
                    .frame(false),
                )
            })
            .on_hover_text("New Group")
            .clicked()
            {
                state.ui.entry_list.creating_group = !state.ui.entry_list.creating_group;
            }
            if with_core_button_style(ui, false, |ui| {
                ui.add_sized(
                    [24.0, 24.0],
                    Button::new(
                        RichText::new(egui_phosphor::regular::ARROWS_IN_SIMPLE)
                            .size(13.0)
                            .color(core_button_text_color(&pal, false)),
                    )
                    .frame(false),
                )
            })
            .on_hover_text("Collapse All")
            .clicked()
            {
                for group in &state.entries.groups {
                    state
                        .ui
                        .entry_list
                        .collapsed_group_ids
                        .insert(group.id.clone());
                }
            }
        });
    });

    if state.ui.entry_list.creating_group {
        ui.horizontal(|ui| {
            // Reserve the Create button on the right and let the field fill the rest.
            // A plain left-to-right row puts the default-width (280 px) field first,
            // which eats the whole row and pushes Create past the sidebar edge,
            // overflowing and growing the panel on a narrow sidebar.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let create = ui.button("Create").clicked();
                let response = ui.add(
                    egui::TextEdit::singleline(&mut state.ui.entry_list.new_group_name)
                        .desired_width(f32::INFINITY),
                );
                let submit =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if create || submit {
                    actions.push(AppAction::CreateGroup {
                        name: state.ui.entry_list.new_group_name.clone(),
                    });
                }
            });
        });
    }

    ui.separator();

    let search = state.ui.entry_list.search_query.to_lowercase();
    let groups = state.entries.groups.clone();
    let all_group_choices = groups
        .iter()
        .map(|group| (group.id.clone(), group.name.clone()))
        .collect::<Vec<_>>();
    let ungrouped_entries = state
        .entries
        .records
        .iter()
        .filter(|entry| entry.group_id.is_empty())
        .filter(|entry| {
            search.is_empty()
                || entry.name.to_lowercase().contains(&search)
                || entry.id.to_string().contains(&search)
        })
        .map(|entry| (entry.id, entry.name.clone(), entry.group_id.clone()))
        .collect::<Vec<_>>();
    let detail_lines = state
        .entries
        .active_entry()
        .map(|entry| entry_details(&entry.structure, entry.source_path.as_deref()))
        .unwrap_or_default();

    if !detail_lines.is_empty() {
        let detail_height = 28.0 + 20.0 * detail_lines.len() as f32;
        egui::Panel::bottom("entry_list_details")
            .exact_size(detail_height)
            .frame(Frame::default().inner_margin(Margin::same(0)))
            .show_inside(ui, |ui| {
                ui.separator();
                ui.label(RichText::new("Details").strong());
                for line in &detail_lines {
                    ui.label(line);
                }
            });
    }

    let ordered_items: Vec<SelectionItem> = {
        let mut items = Vec::new();
        for group in &groups {
            let has_visible = state.entries.records.iter().any(|e| {
                e.group_id == group.id
                    && (search.is_empty()
                        || e.name.to_lowercase().contains(&search)
                        || e.id.to_string().contains(&search))
            });
            if has_visible || search.is_empty() {
                items.push(SelectionItem::Group(group.id.clone()));
            }
            if !state.ui.entry_list.collapsed_group_ids.contains(&group.id) {
                state
                    .entries
                    .records
                    .iter()
                    .filter(|e| e.group_id == group.id)
                    .filter(|e| {
                        search.is_empty()
                            || e.name.to_lowercase().contains(&search)
                            || e.id.to_string().contains(&search)
                    })
                    .for_each(|e| items.push(SelectionItem::Entry(e.id)));
            }
        }
        items.extend(
            ungrouped_entries
                .iter()
                .map(|(id, _, _)| SelectionItem::Entry(*id)),
        );
        items
    };

    ScrollArea::vertical()
        .max_height(ui.available_height().max(120.0))
        // Wheel/trackpad plus content drag (touch-friendly), but the scroll bar
        // stays a non-interactive position indicator: excluding SCROLL_BAR
        // stops the bar from catching a drag that starts on the adjacent panel
        // resize divider — the bug where dragging the divider scrolled instead
        // of resizing.
        .scroll_source(
            egui::scroll_area::ScrollSource::MOUSE_WHEEL | egui::scroll_area::ScrollSource::DRAG,
        )
        .show(ui, |ui| {
            for group in &groups {
                let group_id = group.id.clone();
                let entries = state
                    .entries
                    .records
                    .iter()
                    .filter(|entry| entry.group_id == group_id)
                    .filter(|entry| {
                        search.is_empty()
                            || entry.name.to_lowercase().contains(&search)
                            || entry.id.to_string().contains(&search)
                    })
                    .map(|entry| (entry.id, entry.name.clone(), entry.group_id.clone()))
                    .collect::<Vec<_>>();
                if entries.is_empty() && search.is_empty() {
                    let collapsed = state.ui.entry_list.collapsed_group_ids.contains(&group.id);
                    if render_group_header(
                        state,
                        ui,
                        actions,
                        &group.id,
                        &group.name,
                        collapsed,
                        &ordered_items,
                    ) && !state
                        .ui
                        .entry_list
                        .collapsed_group_ids
                        .insert(group.id.clone())
                    {
                        state.ui.entry_list.collapsed_group_ids.remove(&group.id);
                    }
                    if !collapsed {
                        let ctx = EntryListCtx {
                            group_choices: &all_group_choices,
                            ordered_items: &ordered_items,
                        };
                        for (entry_id, name, entry_group_id) in &entries {
                            render_entry_list_item(
                                state,
                                ui,
                                actions,
                                *entry_id,
                                name,
                                entry_group_id,
                                &ctx,
                            );
                        }
                    }
                    ui.add_space(2.0);
                    continue;
                }
                if entries.is_empty() {
                    continue;
                }

                let collapsed = state.ui.entry_list.collapsed_group_ids.contains(&group.id);
                if render_group_header(
                    state,
                    ui,
                    actions,
                    &group.id,
                    &group.name,
                    collapsed,
                    &ordered_items,
                ) && !state
                    .ui
                    .entry_list
                    .collapsed_group_ids
                    .insert(group.id.clone())
                {
                    state.ui.entry_list.collapsed_group_ids.remove(&group.id);
                }

                if !collapsed {
                    let ctx = EntryListCtx {
                        group_choices: &all_group_choices,
                        ordered_items: &ordered_items,
                    };
                    for (entry_id, name, entry_group_id) in &entries {
                        render_entry_list_item(
                            state,
                            ui,
                            actions,
                            *entry_id,
                            name,
                            entry_group_id,
                            &ctx,
                        );
                    }
                }
                ui.add_space(2.0);
            }

            if !ungrouped_entries.is_empty() && !groups.is_empty() {
                ui.separator();
            }

            let ctx = EntryListCtx {
                group_choices: &all_group_choices,
                ordered_items: &ordered_items,
            };
            for (entry_id, name, group_id) in &ungrouped_entries {
                render_entry_list_item(state, ui, actions, *entry_id, name, group_id, &ctx);
            }
        });
}

pub(crate) fn render_group_header(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    group_id: &str,
    group_name: &str,
    collapsed: bool,
    ordered_items: &[SelectionItem],
) -> bool {
    let is_selected = state.ui.entry_list.selected_group_ids.contains(group_id);
    let folder_icon = if collapsed {
        egui_phosphor::regular::FOLDER
    } else {
        egui_phosphor::regular::FOLDER_OPEN
    };
    let marker = if collapsed {
        egui_phosphor::regular::CARET_RIGHT
    } else {
        egui_phosphor::regular::CARET_DOWN
    };

    let row_h = 22.0;
    let full_w = ui.available_width();
    let btn_w = 44.0;
    let left_w = (full_w - btn_w).max(0.0);

    let is_renaming = state.ui.entry_list.renaming_group_id.as_deref() == Some(group_id);

    // The whole row is the click target for selection and the collapse toggle.
    // Icons and name are painted directly so nothing overlaps it; only the
    // action buttons are real widgets, registered later so their rects win.
    // While renaming the row is hover-only and the text editor owns clicks.
    let sense = if is_renaming {
        Sense::hover()
    } else {
        Sense::click()
    };
    let (row_rect, row_resp) = ui.allocate_exact_size(egui::vec2(full_w, row_h), sense);
    let right_rect = egui::Rect::from_min_size(
        egui::pos2(row_rect.max.x - btn_w, row_rect.min.y),
        egui::vec2(btn_w, row_h),
    );

    // Background (selection or hover): a rounded, inset, filled highlight.
    let pal = crate::frontend::theme::palette(ui);
    let bg = if is_selected {
        pal.blue_overlay(40)
    } else if row_resp.hovered() {
        pal.neutral_overlay(30)
    } else {
        egui::Color32::TRANSPARENT
    };
    if bg != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(
            row_rect.shrink2(egui::vec2(4.0, 1.0)),
            f32::from(crate::frontend::theme::radius::CONTROL),
            bg,
        );
    }

    // Paint the caret marker and folder icon.
    let icon_color = pal.text_muted;
    let mut x = row_rect.left() + 4.0;
    let marker_galley = ui.painter().layout_no_wrap(
        marker.to_string(),
        egui::FontId::proportional(11.0),
        icon_color,
    );
    let marker_w = marker_galley.size().x;
    ui.painter().galley(
        egui::pos2(x, row_rect.center().y - marker_galley.size().y / 2.0),
        marker_galley,
        icon_color,
    );
    x += marker_w + 4.0;
    let folder_galley = ui.painter().layout_no_wrap(
        folder_icon.to_string(),
        egui::FontId::proportional(14.0),
        icon_color,
    );
    let folder_w = folder_galley.size().x;
    ui.painter().galley(
        egui::pos2(x, row_rect.center().y - folder_galley.size().y / 2.0),
        folder_galley,
        icon_color,
    );
    x += folder_w + 6.0;

    // Name, or the in-place rename editor occupying the name's original slot.
    let mut rename_done = false;
    if is_renaming {
        let edit_rect = egui::Rect::from_min_max(
            egui::pos2(x, row_rect.min.y),
            egui::pos2(row_rect.left() + left_w, row_rect.max.y),
        );
        let mut edit_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(edit_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let resp = edit_ui.add(
            egui::TextEdit::singleline(&mut state.ui.entry_list.rename_group_buffer)
                .desired_width(f32::INFINITY),
        );
        if !state.ui.entry_list.rename_group_focus_requested {
            resp.request_focus();
            state.ui.entry_list.rename_group_focus_requested = true;
        }
        // Commit on Enter; cancel on any other focus loss (e.g. click away).
        if resp.lost_focus() {
            if edit_ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                actions.push(AppAction::RenameGroup {
                    group_id: group_id.to_string(),
                    new_name: state.ui.entry_list.rename_group_buffer.clone(),
                });
            }
            rename_done = true;
        }
    } else {
        let name_color = pal.text_primary;
        let avail = (row_rect.left() + left_w - x).max(0.0);
        let mut job = egui::text::LayoutJob::single_section(
            group_name.to_string(),
            egui::TextFormat {
                font_id: egui::FontId::proportional(13.0),
                color: name_color,
                ..Default::default()
            },
        );
        job.wrap = egui::text::TextWrapping {
            max_width: avail,
            max_rows: 1,
            overflow_character: Some('…'),
            break_anywhere: true,
        };
        let galley = ui.painter().fonts_mut(|f| f.layout_job(job));
        ui.painter().galley(
            egui::pos2(x, row_rect.center().y - galley.size().y / 2.0),
            galley,
            name_color,
        );
    }
    if rename_done {
        state.ui.entry_list.renaming_group_id = None;
        state.ui.entry_list.rename_group_focus_requested = false;
    }

    // Edit / delete buttons in the right area.
    let (btn_pencil, btn_trash) = {
        let mut right_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(right_rect)
                .layout(Layout::right_to_left(Align::Center)),
        );
        let trash = right_ui
            .add(
                egui::Button::new(RichText::new(egui_phosphor::regular::TRASH).size(11.0))
                    .frame(false),
            )
            .clicked();
        let pencil = right_ui
            .add(
                egui::Button::new(
                    RichText::new(egui_phosphor::regular::PENCIL_SIMPLE_LINE).size(11.0),
                )
                .frame(false),
            )
            .clicked();
        (pencil, trash)
    };

    if btn_pencil {
        state.ui.entry_list.renaming_group_id = Some(group_id.to_string());
        state.ui.entry_list.rename_group_buffer = group_name.to_string();
        state.ui.entry_list.rename_group_focus_requested = false;
    }
    if btn_trash {
        actions.push(AppAction::DeleteGroup(group_id.to_string()));
    }

    // Pre-collect selection state for the context-menu closure.
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
    row_resp.context_menu(|ui| {
        if ui.button("Rename").clicked() {
            state.ui.entry_list.renaming_group_id = Some(group_id.to_string());
            state.ui.entry_list.rename_group_buffer = group_name.to_string();
            ui.close();
        }
        ui.separator();
        render_delete_menu_items(
            ui,
            actions,
            &sel_entry_ids,
            &sel_group_ids,
            None,
            Some(group_id),
        );
    });

    // Handle selection on plain left-click (not a button click).
    if !btn_pencil && !btn_trash && row_resp.clicked() {
        let shift = ui.input(|i| i.modifiers.shift);
        let ctrl = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
        let this = SelectionItem::Group(group_id.to_string());

        if shift {
            let anchor_pos = state
                .ui
                .entry_list
                .selection_anchor
                .as_ref()
                .and_then(|a| ordered_items.iter().position(|item| item == a));
            let current_pos = ordered_items.iter().position(|item| item == &this);
            if let (Some(a), Some(b)) = (anchor_pos, current_pos) {
                let (lo, hi) = (a.min(b), a.max(b));
                state.ui.entry_list.selected_entry_ids.clear();
                state.ui.entry_list.selected_group_ids.clear();
                for item in &ordered_items[lo..=hi] {
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
                state
                    .ui
                    .entry_list
                    .selected_group_ids
                    .insert(group_id.to_string());
                state.ui.entry_list.selection_anchor = Some(this);
            }
        } else if ctrl {
            if !state.ui.entry_list.selected_group_ids.remove(group_id) {
                state
                    .ui
                    .entry_list
                    .selected_group_ids
                    .insert(group_id.to_string());
            }
            state.ui.entry_list.selection_anchor = Some(this);
        } else {
            state.ui.entry_list.selected_entry_ids.clear();
            state.ui.entry_list.selected_group_ids.clear();
            state
                .ui
                .entry_list
                .selected_group_ids
                .insert(group_id.to_string());
            state.ui.entry_list.selection_anchor = Some(this);
        }

        return !shift && !ctrl;
    }

    false
}

pub(crate) fn render_delete_menu_items(
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    sel_entry_ids: &[u64],
    sel_group_ids: &[String],
    focused_entry_id: Option<u64>,
    focused_group_id: Option<&str>,
) {
    let n_entries = sel_entry_ids.len();
    let n_groups = sel_group_ids.len();

    if n_entries == 0 && n_groups == 0 {
        if let Some(eid) = focused_entry_id {
            if ui.button("Delete Entry").clicked() {
                actions.push(AppAction::DeleteEntry(eid));
                ui.close();
            }
        } else if let Some(gid) = focused_group_id {
            if ui.button("Ungroup").clicked() {
                actions.push(AppAction::DeleteGroup(gid.to_string()));
                ui.close();
            }
            if ui.button("Delete Group and All Entries").clicked() {
                actions.push(AppAction::DeleteGroupWithEntries(gid.to_string()));
                ui.close();
            }
        }
        return;
    }

    if n_entries > 0 {
        let lbl = if n_entries == 1 {
            "Delete 1 Entry".to_string()
        } else {
            format!("Delete {} Entries", n_entries)
        };
        if ui.button(lbl).clicked() {
            actions.push(AppAction::DeleteEntries(sel_entry_ids.to_vec()));
            ui.close();
        }
    }
    if n_groups > 0 {
        let lbl = if n_groups == 1 {
            "Ungroup 1 Group".to_string()
        } else {
            format!("Ungroup {} Groups", n_groups)
        };
        if ui.button(lbl).clicked() {
            for gid in sel_group_ids {
                actions.push(AppAction::DeleteGroup(gid.clone()));
            }
            ui.close();
        }
        let lbl2 = if n_groups == 1 {
            "Delete 1 Group and Its Entries".to_string()
        } else {
            format!("Delete {} Groups and Their Entries", n_groups)
        };
        if ui.button(lbl2).clicked() {
            for gid in sel_group_ids {
                actions.push(AppAction::DeleteGroupWithEntries(gid.clone()));
            }
            ui.close();
        }
    }
}
