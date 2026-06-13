//! The Representation settings page: app-wide *default* visual appearance for
//! newly built or loaded structures.
//!
//! Defined as a block of [`SettingDescriptor`]s appended to the schema-driven
//! [`super::settings_registry`]. The governing rule is **1:1 with a real
//! renderer capability**: every live control here (base style, cartoon geometry,
//! surface style/transparency) writes a field the renderer actually honours via
//! [`AppAction::SetRepresentation`]. Options the renderer cannot yet draw are
//! rendered as inert, clearly-labelled placeholders — never as fake live
//! controls — so a stored value is never silently ignored.
//!
//! Reads come from `state.config.representation`; mutation stays in the
//! dispatcher. The free functions below are plain `fn` pointers (no closures),
//! matching the registry's no-capture discipline.

use eframe::egui::{self, RichText};

use super::settings_registry::{Control, SettingCategory, SettingDescriptor, caption_text};
use crate::{
    backend::representation::{
        BaseStyle, RepresentationEdit, RepresentationGroup, SurfaceStylePref,
    },
    frontend::{actions::AppAction, state::AppState},
};

// --- Group headings -------------------------------------------------------- //

const BASE_GROUP: &str = "Base Representation (Atoms and Bonds)";
const CARTOON_GROUP: &str = "Cartoon / Ribbons";
const SURFACE_GROUP: &str = "Surface";
const SCHEMES_GROUP: &str = "Color Schemes";
const RESTORE_GROUP: &str = "Restore all defaults";

// --- Choice option labels (const, sourced from the enums' own labels) ------ //

const BASE_STYLE_OPTIONS: [&str; 4] = {
    let styles = BaseStyle::all();
    [
        styles[0].label(),
        styles[1].label(),
        styles[2].label(),
        styles[3].label(),
    ]
};

const SURFACE_STYLE_OPTIONS: [&str; 2] = {
    let styles = SurfaceStylePref::all();
    [styles[0].label(), styles[1].label()]
};

// --- Live read / change accessors (free fns → `fn` pointers) --------------- //

fn base_style_read(state: &AppState) -> usize {
    BaseStyle::all()
        .iter()
        .position(|style| *style == state.config.representation.base.default_style)
        .unwrap_or(0)
}

fn base_style_change(index: usize) -> AppAction {
    let style = BaseStyle::all().get(index).copied().unwrap_or_default();
    AppAction::SetRepresentation(RepresentationEdit::DefaultBaseStyle(style))
}

fn surface_style_read(state: &AppState) -> usize {
    SurfaceStylePref::all()
        .iter()
        .position(|style| *style == state.config.representation.surface.style)
        .unwrap_or(0)
}

fn surface_style_change(index: usize) -> AppAction {
    let style = SurfaceStylePref::all()
        .get(index)
        .copied()
        .unwrap_or_default();
    AppAction::SetRepresentation(RepresentationEdit::SurfaceStyle(style))
}

fn helix_width_read(state: &AppState) -> f32 {
    state.config.representation.cartoon.helix_width
}
fn helix_width_change(value: f32) -> AppAction {
    AppAction::SetRepresentation(RepresentationEdit::HelixWidth(value))
}

fn helix_thickness_read(state: &AppState) -> f32 {
    state.config.representation.cartoon.helix_thickness
}
fn helix_thickness_change(value: f32) -> AppAction {
    AppAction::SetRepresentation(RepresentationEdit::HelixThickness(value))
}

fn sheet_width_read(state: &AppState) -> f32 {
    state.config.representation.cartoon.sheet_width
}
fn sheet_width_change(value: f32) -> AppAction {
    AppAction::SetRepresentation(RepresentationEdit::SheetWidth(value))
}

fn sheet_thickness_read(state: &AppState) -> f32 {
    state.config.representation.cartoon.sheet_thickness
}
fn sheet_thickness_change(value: f32) -> AppAction {
    AppAction::SetRepresentation(RepresentationEdit::SheetThickness(value))
}

fn coil_width_read(state: &AppState) -> f32 {
    state.config.representation.cartoon.coil_width
}
fn coil_width_change(value: f32) -> AppAction {
    AppAction::SetRepresentation(RepresentationEdit::CoilWidth(value))
}

fn coil_thickness_read(state: &AppState) -> f32 {
    state.config.representation.cartoon.coil_thickness
}
fn coil_thickness_change(value: f32) -> AppAction {
    AppAction::SetRepresentation(RepresentationEdit::CoilThickness(value))
}

fn smoothing_read(state: &AppState) -> f32 {
    state.config.representation.cartoon.smoothing as f32
}
fn smoothing_change(value: f32) -> AppAction {
    AppAction::SetRepresentation(RepresentationEdit::Smoothing(value.round().max(0.0) as u32))
}

fn profile_read(state: &AppState) -> f32 {
    state.config.representation.cartoon.profile as f32
}
fn profile_change(value: f32) -> AppAction {
    AppAction::SetRepresentation(RepresentationEdit::Profile(value.round().max(0.0) as u32))
}

fn transparency_read(state: &AppState) -> f32 {
    state.config.representation.surface.transparency_percent as f32
}
fn transparency_change(value: f32) -> AppAction {
    AppAction::SetRepresentation(RepresentationEdit::SurfaceTransparency(
        value.round().clamp(0.0, 100.0) as u8,
    ))
}

// --- Placeholder rendering ------------------------------------------------- //
//
// Inert, clearly-labelled stand-ins for options the renderer cannot yet honour.
// Each group's placeholders are a const `(label, reason)` table drawn by a
// single `Custom` renderer (one descriptor, rather than ~30 capture-free fns),
// so they always read as deliberate "pending" rows, never as broken controls.

/// Standard reason: the feature simply isn't wired in the renderer yet.
const PENDING: &str = "Not yet available — renderer support pending.";
/// A value the renderer hardcodes today, so there is nothing to tune.
const FIXED: &str = "Not yet available — the renderer uses a fixed value here.";
/// Blocked on a GPU capability the current pipeline does not expose.
const CAPABILITY: &str =
    "Not yet available — needs a rendering capability the pipeline does not expose yet.";
/// Blocked because the underlying object/data model does not exist.
const NO_VOLUME: &str =
    "Not yet available — there is no volume import or discrete surface objects.";

const BASE_PLACEHOLDERS: &[(&str, &str)] = &[
    ("Default color scheme", PENDING),
    ("Custom color", PENDING),
    ("Show bond orders", FIXED),
    ("Scale wire width", PENDING),
    ("Minimum / maximum wire width", PENDING),
    ("Wire width", FIXED),
    ("Split / blend coloring", PENDING),
    ("Wire smoothing", PENDING),
    ("Tube / thin-tube radius", PENDING),
    ("Ball size (%)", FIXED),
    ("Stick radius", FIXED),
    ("CPK / sphere scale (%)", FIXED),
];

const CARTOON_PLACEHOLDERS: &[(&str, &str)] = &[
    ("Cartoon style", PENDING),
    ("Color scheme / single color", PENDING),
    ("Quality", PENDING),
    ("Strands drawn as", PENDING),
    ("Simplify while moving", PENDING),
    ("Blend colors", PENDING),
    ("Helix interior", PENDING),
    ("Thin / thick tube width", PENDING),
    ("Ladder width (nucleic acids)", PENDING),
];

const SURFACE_PLACEHOLDERS: &[(&str, &str)] = &[
    ("Front / back split + linked transparency", PENDING),
    ("Color mode / constant color", PENDING),
    ("Paired (positive / negative) colors", PENDING),
    ("Density-map colors", NO_VOLUME),
    ("Mesh width (scaled / constant, min / max)", PENDING),
    ("Angle-dependent transparency", CAPABILITY),
    ("Darken by cavity + intensity", CAPABILITY),
];

fn placeholder_block(ui: &mut egui::Ui, items: &[(&str, &str)]) {
    let pal = crate::frontend::theme::palette(ui);
    for (label, reason) in items {
        ui.add_space(2.0);
        ui.label(RichText::new(*label).color(pal.text_muted));
        ui.label(caption_text(*reason, pal.text_muted));
    }
}

fn render_base_placeholders(
    _state: &mut AppState,
    ui: &mut egui::Ui,
    _actions: &mut Vec<AppAction>,
) {
    placeholder_block(ui, BASE_PLACEHOLDERS);
}

fn render_cartoon_placeholders(
    _state: &mut AppState,
    ui: &mut egui::Ui,
    _actions: &mut Vec<AppAction>,
) {
    placeholder_block(ui, CARTOON_PLACEHOLDERS);
}

fn render_surface_placeholders(
    _state: &mut AppState,
    ui: &mut egui::Ui,
    _actions: &mut Vec<AppAction>,
) {
    placeholder_block(ui, SURFACE_PLACEHOLDERS);
}

// --- Notes, scope text, and reset buttons ---------------------------------- //

fn render_intro(_state: &mut AppState, ui: &mut egui::Ui, _actions: &mut Vec<AppAction>) {
    let pal = crate::frontend::theme::palette(ui);
    ui.label(
        RichText::new(
            "Defaults applied to newly built or loaded structures — not edits to the \
             structure currently in view.",
        )
        .color(pal.text_muted),
    );
}

fn render_base_intro(_state: &mut AppState, ui: &mut egui::Ui, _actions: &mut Vec<AppAction>) {
    let pal = crate::frontend::theme::palette(ui);
    ui.label(
        RichText::new(
            "Applies to ligands, ions, solvent, and other non-polymer atoms; biopolymer \
             chains follow the Cartoon defaults below.",
        )
        .color(pal.text_muted),
    );
    ui.add_space(4.0);
    for line in [
        "Wire — bonds as thin lines, no atom markers.",
        "Stick — bonds as cylinders, no atom spheres.",
        "Ball & Stick — cylinders plus small atom spheres.",
        "Sphere — full van der Waals spheres, no bonds.",
    ] {
        ui.label(caption_text(line, pal.text_muted));
    }
}

fn render_schemes_note(_state: &mut AppState, ui: &mut egui::Ui, _actions: &mut Vec<AppAction>) {
    let pal = crate::frontend::theme::palette(ui);
    ui.label(caption_text(
        "Molecular color schemes and custom colors are deferred — there is no \
         color-scheme backend yet, so no live control is offered here.",
        pal.text_muted,
    ));
}

/// A group-scoped "Restore Defaults" button. The group can't be captured by a
/// `fn` pointer, so each group gets a tiny wrapper feeding this shared helper.
fn group_reset_button(ui: &mut egui::Ui, actions: &mut Vec<AppAction>, group: RepresentationGroup) {
    if ui.button("Restore Defaults").clicked() {
        actions.push(AppAction::ResetRepresentationGroup(group));
    }
}

fn render_base_reset(_state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    group_reset_button(ui, actions, RepresentationGroup::Base);
}

fn render_cartoon_reset(_state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    group_reset_button(ui, actions, RepresentationGroup::Cartoon);
}

fn render_surface_reset(_state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    group_reset_button(ui, actions, RepresentationGroup::Surface);
}

fn render_page_reset(_state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    if ui.button("Restore all representation defaults").clicked() {
        actions.push(AppAction::ResetRepresentationDefaults);
    }
}

// --- Descriptor builders --------------------------------------------------- //

/// A live `DragValue` setting (no inline reset — the spec scopes reset to the
/// group and page buttons only). One uniform builder for the cartoon/surface
/// numeric defaults, hence the wide-but-flat parameter list.
#[allow(clippy::too_many_arguments)]
fn value(
    id: &'static str,
    group: &'static str,
    title: &'static str,
    keywords: &'static [&'static str],
    read: fn(&AppState) -> f32,
    range: std::ops::RangeInclusive<f32>,
    unit: &'static str,
    speed: f32,
    on_change: fn(f32) -> AppAction,
) -> SettingDescriptor {
    SettingDescriptor {
        id,
        category: SettingCategory::Representation,
        group,
        title,
        description: "",
        keywords,
        control: Control::Value {
            read,
            range,
            unit,
            speed,
            on_change,
        },
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
    }
}

/// A `Custom`-rendered note / button / placeholder block (no reset, no search
/// beyond its keywords).
fn custom(
    id: &'static str,
    group: &'static str,
    title: &'static str,
    keywords: &'static [&'static str],
    render: fn(&mut AppState, &mut egui::Ui, &mut Vec<AppAction>),
) -> SettingDescriptor {
    SettingDescriptor {
        id,
        category: SettingCategory::Representation,
        group,
        title,
        description: "",
        keywords,
        control: Control::Custom(render),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
    }
}

/// The Representation page's descriptors, in render order. Group headers appear
/// in first-appearance order: Base, Cartoon, Surface, Color Schemes, Restore.
pub(super) fn descriptors() -> Vec<SettingDescriptor> {
    vec![
        // Page intro (first descriptor, leads the Base group).
        custom(
            "representation.intro",
            BASE_GROUP,
            "About these defaults",
            &["representation", "appearance", "default", "style"],
            render_intro,
        ),
        // Base group: scope + style summary, the live base-style picker, the
        // placeholder block, then the group reset.
        custom(
            "representation.base.scope",
            BASE_GROUP,
            "Base scope",
            &["ligand", "ion", "solvent", "non-polymer", "base"],
            render_base_intro,
        ),
        SettingDescriptor {
            id: "representation.base.style",
            category: SettingCategory::Representation,
            group: BASE_GROUP,
            title: "Default base style",
            description: "Base representation for ligands, ions, solvent, and other \
                          non-polymer atoms.",
            keywords: &[
                "base",
                "style",
                "wire",
                "stick",
                "ball",
                "sphere",
                "wireframe",
                "vdw",
                "cpk",
                "representation",
                "default",
            ],
            control: Control::Choice {
                read: base_style_read,
                options: &BASE_STYLE_OPTIONS,
                on_change: base_style_change,
            },
            enabled: None,
            indent: false,
            is_default: None,
            reset: None,
        },
        custom(
            "representation.base.pending",
            BASE_GROUP,
            "Base — pending options",
            &["color", "bond order", "wire width", "ball", "stick", "cpk"],
            render_base_placeholders,
        ),
        custom(
            "representation.base.reset",
            BASE_GROUP,
            "Restore base defaults",
            &["reset", "default", "restore", "base"],
            render_base_reset,
        ),
        // Cartoon group: the real `ViewportCartoonState` geometry knobs.
        value(
            "representation.cartoon.helix_width",
            CARTOON_GROUP,
            "Helix width",
            &["cartoon", "ribbon", "helix", "width"],
            helix_width_read,
            0.05..=10.0,
            " Å",
            0.05,
            helix_width_change,
        ),
        value(
            "representation.cartoon.helix_thickness",
            CARTOON_GROUP,
            "Helix thickness",
            &["cartoon", "ribbon", "helix", "thickness"],
            helix_thickness_read,
            0.05..=10.0,
            " Å",
            0.05,
            helix_thickness_change,
        ),
        value(
            "representation.cartoon.sheet_width",
            CARTOON_GROUP,
            "Sheet (strand) width",
            &["cartoon", "ribbon", "sheet", "strand", "width"],
            sheet_width_read,
            0.05..=10.0,
            " Å",
            0.05,
            sheet_width_change,
        ),
        value(
            "representation.cartoon.sheet_thickness",
            CARTOON_GROUP,
            "Sheet (strand) thickness",
            &["cartoon", "ribbon", "sheet", "strand", "thickness"],
            sheet_thickness_read,
            0.05..=10.0,
            " Å",
            0.05,
            sheet_thickness_change,
        ),
        value(
            "representation.cartoon.coil_width",
            CARTOON_GROUP,
            "Coil width",
            &["cartoon", "ribbon", "coil", "loop", "width"],
            coil_width_read,
            0.05..=10.0,
            " Å",
            0.05,
            coil_width_change,
        ),
        value(
            "representation.cartoon.coil_thickness",
            CARTOON_GROUP,
            "Coil thickness",
            &["cartoon", "ribbon", "coil", "loop", "thickness"],
            coil_thickness_read,
            0.05..=10.0,
            " Å",
            0.05,
            coil_thickness_change,
        ),
        value(
            "representation.cartoon.smoothing",
            CARTOON_GROUP,
            "Smoothing",
            &["cartoon", "ribbon", "smoothing", "spline"],
            smoothing_read,
            1.0..=32.0,
            "",
            1.0,
            smoothing_change,
        ),
        value(
            "representation.cartoon.profile",
            CARTOON_GROUP,
            "Profile detail",
            &["cartoon", "ribbon", "profile", "segments", "detail"],
            profile_read,
            6.0..=48.0,
            "",
            1.0,
            profile_change,
        ),
        custom(
            "representation.cartoon.pending",
            CARTOON_GROUP,
            "Cartoon — pending options",
            &["cartoon", "style", "quality", "tube", "ladder", "color"],
            render_cartoon_placeholders,
        ),
        custom(
            "representation.cartoon.reset",
            CARTOON_GROUP,
            "Restore cartoon defaults",
            &["reset", "default", "restore", "cartoon"],
            render_cartoon_reset,
        ),
        // Surface group: live style + transparency, then placeholders + reset.
        SettingDescriptor {
            id: "representation.surface.style",
            category: SettingCategory::Representation,
            group: SURFACE_GROUP,
            title: "Surface style",
            description: "Solid (filled) or mesh molecular surface.",
            keywords: &["surface", "style", "solid", "fill", "mesh"],
            control: Control::Choice {
                read: surface_style_read,
                options: &SURFACE_STYLE_OPTIONS,
                on_change: surface_style_change,
            },
            enabled: None,
            indent: false,
            is_default: None,
            reset: None,
        },
        value(
            "representation.surface.transparency",
            SURFACE_GROUP,
            "Transparency",
            &["surface", "transparency", "opacity", "alpha"],
            transparency_read,
            0.0..=100.0,
            " %",
            1.0,
            transparency_change,
        ),
        custom(
            "representation.surface.pending",
            SURFACE_GROUP,
            "Surface — pending options",
            &["surface", "color", "mesh width", "cavity", "density"],
            render_surface_placeholders,
        ),
        custom(
            "representation.surface.reset",
            SURFACE_GROUP,
            "Restore surface defaults",
            &["reset", "default", "restore", "surface"],
            render_surface_reset,
        ),
        // Color Schemes: a single deferred note, no controls.
        custom(
            "representation.schemes.note",
            SCHEMES_GROUP,
            "Color schemes",
            &["color", "scheme", "palette", "custom color"],
            render_schemes_note,
        ),
        // Page-level reset.
        custom(
            "representation.reset_all",
            RESTORE_GROUP,
            "Restore all representation defaults",
            &["reset", "default", "restore", "all", "representation"],
            render_page_reset,
        ),
    ]
}
