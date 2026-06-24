use super::{AppState, DockArea, DockModel, DockTab, MdRunPrompt, MdStageEdit, StaticView};
use crate::workflows::molecular_dynamics::{MdStage, StageLength};

/// The config-side literal default (`DockLayoutConfig::default`, in the
/// backend layer) must stay in lock-step with `DockModel::default`, since the
/// two are spelled out independently (the backend can't reference the
/// frontend's view tokens or size consts).
#[test]
fn default_dock_matches_config_default() {
    let from_model = DockModel::default().to_config();
    let literal = crate::backend::config::DockLayoutConfig::default();
    assert_eq!(from_model.bottom.tabs, literal.bottom.tabs);
    assert_eq!(from_model.bottom.active, literal.bottom.active);
    assert_eq!(from_model.bottom.collapsed, literal.bottom.collapsed);
    assert_eq!(from_model.right.tabs, literal.right.tabs);
    assert_eq!(from_model.right.active, literal.right.active);
    assert_eq!(from_model.right.collapsed, literal.right.collapsed);
    assert_eq!(from_model.right_width, literal.right_width);
    assert_eq!(from_model.bottom_height, literal.bottom_height);
}

/// A sidebar is no longer pinned to a fixed pixel cap: on a wide window it may
/// grow far past the old limit, bounded only by the opposite sidebar's
/// footprint and the reserved workspace minimum. The two sidebars can never
/// jointly shrink the workspace below that minimum, and a window too narrow to
/// honor the reservation still floors the max at the sidebar's own minimum so
/// `clamp(min, max)` stays valid.
#[test]
fn sidebar_max_width_reserves_workspace_not_a_fixed_cap() {
    use super::{
        LayoutState, SIDEBAR_MIN_WIDTH_PRIMARY, SIDEBAR_MIN_WIDTH_SECONDARY, WORKSPACE_MIN_WIDTH,
        sidebar_max_width,
    };

    // Wide window, opposite sidebar hidden: the sidebar may claim everything
    // past the reserved workspace — well beyond the old fixed 480 px cap.
    let wide = 2560.0;
    let lone_max = sidebar_max_width(wide, 0.0, SIDEBAR_MIN_WIDTH_PRIMARY);
    assert_eq!(lone_max, wide - WORKSPACE_MIN_WIDTH);
    assert!(lone_max > 480.0);

    // A window too narrow to honor the reservation floors the max at the
    // sidebar's own minimum.
    assert_eq!(
        sidebar_max_width(200.0, 0.0, SIDEBAR_MIN_WIDTH_SECONDARY),
        SIDEBAR_MIN_WIDTH_SECONDARY
    );

    // Two visible sidebars at their per-side maxima never overlap: combined
    // with the workspace minimum they always fit the window, for stored widths
    // from narrow to far wider than the window itself.
    for &primary_stored in &[240.0_f32, 600.0, 1800.0] {
        for &secondary_stored in &[240.0_f32, 600.0, 1800.0] {
            let layout = LayoutState {
                primary_sidebar_width: primary_stored,
                dock: DockModel {
                    right_width: secondary_stored,
                    ..DockModel::default()
                },
                ..LayoutState::default()
            };
            let primary_rendered = primary_stored.clamp(
                SIDEBAR_MIN_WIDTH_PRIMARY,
                layout.primary_sidebar_max_width(wide),
            );
            let secondary_rendered = secondary_stored.clamp(
                SIDEBAR_MIN_WIDTH_SECONDARY,
                layout.secondary_sidebar_max_width(wide),
            );
            assert!(
                primary_rendered + secondary_rendered + WORKSPACE_MIN_WIDTH <= wide + 0.5,
                "sidebars overlap at primary={primary_stored} secondary={secondary_stored}"
            );
        }
    }
}

/// A saved layout missing a view, duplicating one, or naming an unknown token
/// is repaired on load so every fixed view is reachable in exactly one area.
#[test]
fn from_config_repairs_incomplete_layout() {
    use crate::backend::config::{DockAreaLayout, DockLayoutConfig};
    let config = DockLayoutConfig {
        bottom: DockAreaLayout {
            // Console duplicated, an unknown token, and Assistant/Output/Monitor
            // missing entirely.
            tabs: vec!["console".into(), "console".into(), "mystery".into()],
            active: Some("console".into()),
            collapsed: false,
        },
        right: DockAreaLayout {
            tabs: vec![],
            active: None,
            collapsed: true,
        },
        right_width: 300.0,
        bottom_height: 200.0,
    };
    let model = DockModel::from_config(&config);
    for view in StaticView::all() {
        let tab = DockTab::Static(*view);
        let holders = DockArea::all()
            .into_iter()
            .filter(|&area| model.area(area).tabs.contains(&tab))
            .count();
        assert_eq!(holders, 1, "{view:?} must appear in exactly one area");
    }
    // Assistant is restored to its home (right) area.
    assert!(
        model
            .right
            .tabs
            .contains(&DockTab::Static(StaticView::Assistant))
    );
}

#[test]
fn insert_tab_dedups_across_areas_and_focuses() {
    let mut dock = DockModel::default();
    // Assistant lives in the right area by default; moving it to the bottom must
    // remove it from the right (a tab lives in exactly one place), make it
    // active in the bottom, and reveal the bottom.
    let assistant = DockTab::Static(StaticView::Assistant);
    dock.insert_tab(DockArea::Bottom, assistant, Some(0));
    assert!(!dock.right.tabs.contains(&assistant));
    assert_eq!(dock.bottom.tabs.first(), Some(&assistant));
    assert_eq!(dock.bottom.active, Some(assistant));
    assert!(!dock.bottom.collapsed);
}

#[test]
fn move_tab_reorders_within_area_with_index_adjustment() {
    // Bottom default order: Console, TaskMonitor, Output.
    let mut dock = DockModel::default();
    let console = DockTab::Static(StaticView::Console);
    // Move Console (index 0) toward the end (index 2): after removing it the
    // list is [TaskMonitor, Output] and the requested index adjusts to 2.
    dock.move_tab(console, DockArea::Bottom, Some(2));
    assert_eq!(
        dock.bottom.tabs,
        vec![
            DockTab::Static(StaticView::TaskMonitor),
            console,
            DockTab::Static(StaticView::Output),
        ]
    );
}

#[test]
fn remove_tab_repoints_active_to_last() {
    let mut dock = DockModel::default();
    // Console is active in the bottom; removing it repoints active to the new
    // last remaining tab.
    dock.remove_tab(DockTab::Static(StaticView::Console));
    assert_eq!(dock.bottom.active, dock.bottom.tabs.last().copied());
    assert!(dock.bottom.active.is_some());
}

#[test]
fn add_task_is_sticky_to_the_area_holding_tasks() {
    let mut dock = DockModel::default();
    // First task homes to the right sidebar.
    dock.add_task(1);
    assert_eq!(dock.area_of(DockTab::Task(1)), Some(DockArea::Right));
    // Drag it to the bottom; a second task now homes alongside it (sticky).
    dock.move_tab(DockTab::Task(1), DockArea::Bottom, None);
    dock.add_task(2);
    assert_eq!(dock.area_of(DockTab::Task(2)), Some(DockArea::Bottom));
}

#[test]
fn clear_task_tabs_keeps_fixed_views() {
    let mut dock = DockModel::default();
    dock.add_task(7); // -> right, active
    dock.clear_task_tabs();
    assert!(dock.area_of(DockTab::Task(7)).is_none());
    // The fixed Assistant view remains and is the right area's active tab again.
    assert!(
        dock.right
            .tabs
            .contains(&DockTab::Static(StaticView::Assistant))
    );
    assert_eq!(
        dock.right.active,
        Some(DockTab::Static(StaticView::Assistant))
    );
}

#[test]
fn is_visible_combines_emptiness_and_collapse() {
    let mut dock = DockModel::default();
    assert!(dock.is_visible(DockArea::Bottom)); // has tabs, not collapsed
    assert!(dock.is_visible(DockArea::Right)); // has Assistant, not collapsed by default
    dock.right.collapsed = true;
    assert!(!dock.is_visible(DockArea::Right)); // explicitly collapsed -> hidden
    dock.bottom.tabs.clear();
    dock.bottom.active = None;
    assert!(!dock.is_visible(DockArea::Bottom)); // empty -> hidden
}

#[test]
fn is_collapsed_only_for_a_hidden_non_empty_area() {
    // `is_collapsed` backs the in-window reveal handle: it must fire exactly
    // when the user hid a panel that still holds tabs (the "I collapsed it
    // and now it's gone" case), and never for an empty area (nothing to
    // reveal) or a shown one.
    let mut dock = DockModel::default();
    assert!(!dock.is_collapsed(DockArea::Right)); // shown by default
    assert!(!dock.is_collapsed(DockArea::Bottom));
    dock.right.collapsed = true;
    assert!(dock.is_collapsed(DockArea::Right)); // non-empty + collapsed
    // An empty area is hidden too, but has nothing to reveal:
    dock.bottom.tabs.clear();
    dock.bottom.active = None;
    dock.bottom.collapsed = true;
    assert!(!dock.is_collapsed(DockArea::Bottom));
}

#[test]
fn empty_startup_does_not_create_initial_entry() {
    let state = AppState::scratch(Default::default(), Vec::new());

    assert!(!state.has_active_entry());
    assert_eq!(state.entries.records.len(), 0);
    assert_eq!(state.entries.tabs.len(), 0);
    assert_eq!(state.current_entry_label(), "Scratch");
}

fn prompt_with_one_produce_stage() -> MdRunPrompt {
    MdRunPrompt {
        stages: vec![MdStage::produce(300.0)],
        ..Default::default()
    }
}

#[test]
fn edit_stage_sets_and_reverts_tiered_parameter() {
    let mut prompt = prompt_with_one_produce_stage();
    // Setting and clearing an Advanced-tier parameter round-trips through the
    // Option model (set -> Some, revert -> None).
    prompt.edit_stage(0, MdStageEdit::PmeOrder(Some(6)));
    assert_eq!(prompt.stages[0].params.pme_order, Some(6));
    prompt.edit_stage(0, MdStageEdit::PmeOrder(None));
    assert_eq!(prompt.stages[0].params.pme_order, None);
}

#[test]
fn edit_stage_inline_fields_mutate_in_place() {
    let mut prompt = prompt_with_one_produce_stage();
    prompt.edit_stage(0, MdStageEdit::Temperature(287.0));
    prompt.edit_stage(0, MdStageEdit::Length(StageLength::Steps(1234)));
    prompt.edit_stage(0, MdStageEdit::PressureBar(1.5));
    assert_eq!(prompt.stages[0].temperature_k, 287.0);
    assert_eq!(prompt.stages[0].length, StageLength::Steps(1234));
    assert_eq!(prompt.stages[0].pressure.unwrap().ref_bar, 1.5);
}

#[test]
fn edit_stage_raw_lines_add_set_and_remove() {
    let mut prompt = prompt_with_one_produce_stage();
    prompt.edit_stage(0, MdStageEdit::AddRawLine);
    assert_eq!(prompt.stages[0].raw_passthrough.len(), 1);
    prompt.edit_stage(
        0,
        MdStageEdit::SetRawLine {
            line: 0,
            key: "nstcomm".to_string(),
            value: "50".to_string(),
        },
    );
    assert_eq!(
        prompt.stages[0].raw_passthrough[0],
        ("nstcomm".to_string(), "50".to_string())
    );
    prompt.edit_stage(0, MdStageEdit::RemoveRawLine(0));
    assert!(prompt.stages[0].raw_passthrough.is_empty());
}

#[test]
fn edit_stage_ignores_out_of_range_index() {
    let mut prompt = prompt_with_one_produce_stage();
    // Must not panic on a stale index (e.g. a removed stage).
    prompt.edit_stage(9, MdStageEdit::Temperature(123.0));
    assert_eq!(prompt.stages[0].temperature_k, 300.0);
}

#[test]
fn toggle_stage_expanded_opens_one_at_a_time() {
    let mut prompt = prompt_with_one_produce_stage();
    assert_eq!(prompt.expanded_stage, None);
    prompt.toggle_stage_expanded(0);
    assert_eq!(prompt.expanded_stage, Some(0));
    prompt.toggle_stage_expanded(0);
    assert_eq!(prompt.expanded_stage, None);
}

#[test]
fn inline_and_detail_edits_reach_the_realized_mdp() {
    use crate::engines::gromacs::input::render_mdp;
    use crate::engines::gromacs::stage_specs_from_md_stages;
    use crate::workflows::molecular_dynamics::ForceFieldFamily;

    // The merge of an inline (temperature) and a detail (PME order) edit must
    // resolve into the realized stage exactly as the run will see it.
    let mut prompt = prompt_with_one_produce_stage();
    prompt.edit_stage(0, MdStageEdit::Temperature(310.0));
    prompt.edit_stage(0, MdStageEdit::PmeOrder(Some(6)));

    let specs = stage_specs_from_md_stages(&prompt.stages, ForceFieldFamily::Amber, None);
    let mdp = render_mdp(&specs[0].settings);
    assert!(
        mdp.lines()
            .any(|line| line.starts_with("ref-t") && line.trim_end().ends_with("= 310")),
        "edited temperature should reach ref-t:\n{mdp}"
    );
    assert!(
        mdp.lines()
            .any(|line| line.starts_with("pme-order") && line.trim_end().ends_with("= 6")),
        "edited PME order should reach the mdp:\n{mdp}"
    );
}
