use std::ops::RangeInclusive;

use eframe::egui::{self, RichText};

use crate::frontend::{actions::AppAction, state::AppState};

pub(crate) const CAPTION_SIZE: f32 = 12.5;

pub(crate) fn caption_text(text: impl Into<String>, color: egui::Color32) -> RichText {
    RichText::new(text).size(CAPTION_SIZE).color(color)
}

/// Top-level grouping for the Settings panel. `General`, `Representation`,
/// `Engines`, and `Tasks` are populated; `Advanced` carries the meta-settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingCategory {
    #[default]
    General,
    Representation,
    Engines,
    Assistant,
    Tasks,
    Hardware,
    Advanced,
}

impl SettingCategory {
    /// Heading shown above this category's groups, and the label of its entry in
    /// the modal's left rail.
    pub fn label(self) -> &'static str {
        match self {
            SettingCategory::General => "General",
            SettingCategory::Representation => "Representation",
            SettingCategory::Engines => "Engines",
            SettingCategory::Assistant => "Assistant",
            SettingCategory::Tasks => "Tasks",
            SettingCategory::Hardware => "Hardware",
            SettingCategory::Advanced => "Advanced",
        }
    }
}

/// Stable iteration order for categories in the rendered panel and the rail.
pub const CATEGORY_ORDER: [SettingCategory; 7] = [
    SettingCategory::General,
    SettingCategory::Representation,
    SettingCategory::Engines,
    SettingCategory::Assistant,
    SettingCategory::Tasks,
    SettingCategory::Hardware,
    SettingCategory::Advanced,
];

/// How a single setting is edited. Every variant keeps the Elm flow: it reads
/// from [`AppState`] and returns an [`AppAction`] to emit — it never mutates.
pub enum Control {
    /// A boolean checkbox; the descriptor's title is the checkbox label.
    Toggle {
        read: fn(&AppState) -> bool,
        on_change: fn(bool) -> AppAction,
    },
    /// A one-of-N choice rendered as a combo box. `read` returns the index of
    /// the current value within `options`; `on_change` maps a picked index back
    /// to an action.
    Choice {
        read: fn(&AppState) -> usize,
        options: &'static [&'static str],
        on_change: fn(usize) -> AppAction,
    },
    /// A continuous value. `on_change`'s `bool` is `commit`: `false` while the
    /// slider is mid-drag (live preview, do not persist), `true` on release or a
    /// discrete change — preserving the glass-intensity drag/release pattern.
    Slider {
        read: fn(&AppState) -> f32,
        range: RangeInclusive<f32>,
        on_change: fn(f32, bool) -> AppAction,
        /// Whether the slider draws its numeric value box. `false` reads as a
        /// bare track (matches the pre-registry blur-intensity control, which
        /// used `.show_value(false)` so the slider nests cleanly under its
        /// parent toggle).
        show_value: bool,
    },
    /// A free-typed numeric value rendered as an [`egui::DragValue`] with a unit
    /// suffix (`" Å"`, `" %"`, or `""`). Persists on every discrete change — used
    /// by the Representation cartoon/transparency defaults, which are absolute
    /// preferences with no live-preview drag semantics (unlike [`Self::Slider`]).
    Value {
        read: fn(&AppState) -> f32,
        range: RangeInclusive<f32>,
        unit: &'static str,
        speed: f32,
        on_change: fn(f32) -> AppAction,
    },
    /// Escape hatch for editors too complex to express declaratively (e.g. the
    /// engines table, a path picker). Still confined to emitting actions in
    /// practice — the renderer receives `&mut AppState` only to *read* it.
    Custom(fn(&mut AppState, &mut egui::Ui, &mut Vec<AppAction>)),
}

/// A declarative description of one setting.
pub struct SettingDescriptor {
    /// Stable dotted key, e.g. `"appearance.theme"`. Used as a widget id salt
    /// and matched by search.
    pub id: &'static str,
    pub category: SettingCategory,
    /// Section heading the setting renders under, e.g. `"Appearance"`.
    pub group: &'static str,
    pub title: &'static str,
    /// Help text shown beneath the control and matched by search.
    pub description: &'static str,
    /// Extra search terms not present in the title/description.
    pub keywords: &'static [&'static str],
    pub control: Control,
    /// Optional gate: when present and it returns `false`, the control renders
    /// disabled (e.g. blur intensity while transparency is off). `None` =
    /// always enabled. (Availability — whether a setting is registered at all,
    /// e.g. glass support — is decided in [`registry`], not here.)
    pub enabled: Option<fn(&AppState) -> bool>,
    /// When `true`, the control is indented one step, so it reads as nested
    /// under the setting directly above it (the blur slider beneath the
    /// Transparency toggle).
    pub indent: bool,
    /// Whether the current value differs from the default. Together with
    /// [`reset`](Self::reset) it drives the inline "reset to default"
    /// affordance: the button appears only while this returns `false`. `None`
    /// for settings with no meaningful default (path pickers, the engines
    /// table, informational placeholders), which opt out of reset entirely.
    pub is_default: Option<fn(&AppState) -> bool>,
    /// Action that restores this setting's default value, emitted when the
    /// reset affordance is clicked. Paired with [`is_default`](Self::is_default);
    /// both are present or both `None`.
    pub reset: Option<fn() -> AppAction>,
}

impl SettingDescriptor {
    /// Whether this setting matches a (already lower-cased) search query across
    /// its id, title, description, and keywords. Empty query matches everything.
    pub(crate) fn matches(&self, search: &str) -> bool {
        if search.is_empty() {
            return true;
        }
        let hit = |text: &str| text.to_lowercase().contains(search);
        hit(self.id)
            || hit(self.title)
            || hit(self.description)
            || self.keywords.iter().any(|keyword| hit(keyword))
    }
}
