//! App-wide *default* visual appearance applied to newly built or loaded
//! structures — not edits to whatever is currently in view.
//!
//! This is the persisted half of the Representation settings page. Every field
//! here corresponds **1:1 to a real renderer capability**: the base atom/bond
//! style, the cartoon-ribbon geometry, and the molecular-surface style +
//! transparency. Options the renderer cannot yet honour are deliberately absent
//! from this model and surfaced in the UI as inert placeholders instead — so a
//! stored value is never a value the renderer will silently ignore.
//!
//! Defaults mirror the current hard-coded behaviour
//! ([`crate::frontend::ViewportCartoonState::default`],
//! [`crate::frontend::ViewportSurfaceState::default`]) so turning the feature on
//! changes nothing until the user moves a control. The frontend translates these
//! preferences into the viewport's per-entry visual state when a structure is
//! first shown (base style + cartoon) or its first surface is enabled (surface).

use serde::{Deserialize, Serialize};

/// Default base representation for non-polymer atoms and their bonds. The four
/// styles the renderer can draw as a *base* geometry; each maps 1:1 onto an
/// [`crate::frontend::state::AtomStyle`] frontend-side. (Biopolymer chains keep
/// their Cartoon default and follow [`CartoonPrefs`] instead.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BaseStyle {
    /// Bonds as thin lines, no atom markers (renderer `Wireframe`).
    Wire,
    /// Bonds as cylinders, no atom spheres (renderer `Stick`).
    Stick,
    /// Cylinders plus small atom spheres (renderer `BallAndStick`).
    #[default]
    BallAndStick,
    /// Full van der Waals spheres, no bonds (renderer `Sphere`).
    Sphere,
}

impl BaseStyle {
    pub const fn all() -> [BaseStyle; 4] {
        [
            BaseStyle::Wire,
            BaseStyle::Stick,
            BaseStyle::BallAndStick,
            BaseStyle::Sphere,
        ]
    }

    pub const fn label(self) -> &'static str {
        match self {
            BaseStyle::Wire => "Wire",
            BaseStyle::Stick => "Stick",
            BaseStyle::BallAndStick => "Ball & Stick",
            BaseStyle::Sphere => "Sphere",
        }
    }
}

/// Default molecular-surface render style. Maps 1:1 onto the renderer's
/// [`crate::frontend::SurfaceStyle`] (`Solid` ↔ `Fill`, `Mesh` ↔ `Mesh`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SurfaceStylePref {
    /// A filled, opaque-shaded surface (renderer `Fill`).
    Solid,
    /// A wireframe mesh surface (renderer `Mesh`).
    #[default]
    Mesh,
}

impl SurfaceStylePref {
    pub const fn all() -> [SurfaceStylePref; 2] {
        [SurfaceStylePref::Solid, SurfaceStylePref::Mesh]
    }

    pub const fn label(self) -> &'static str {
        match self {
            SurfaceStylePref::Solid => "Solid",
            SurfaceStylePref::Mesh => "Mesh",
        }
    }
}

/// Base atom/bond default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BasePrefs {
    pub default_style: BaseStyle,
}

/// Cartoon / ribbon geometry defaults — exactly the knobs the cartoon renderer
/// honours ([`crate::frontend::ViewportCartoonState`]). `smoothing` and
/// `profile` are integer counts (spline smoothing passes / cross-section
/// segments); the rest are Ångström widths and thicknesses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CartoonPrefs {
    pub helix_width: f32,
    pub helix_thickness: f32,
    pub sheet_width: f32,
    pub sheet_thickness: f32,
    pub coil_width: f32,
    pub coil_thickness: f32,
    pub smoothing: u32,
    pub profile: u32,
}

impl Default for CartoonPrefs {
    fn default() -> Self {
        // Mirror `ViewportCartoonState::default()` so defaults are unchanged.
        Self {
            helix_width: 2.35,
            helix_thickness: 0.50,
            sheet_width: 3.05,
            sheet_thickness: 0.36,
            coil_width: 0.58,
            coil_thickness: 0.58,
            smoothing: 8,
            profile: 10,
        }
    }
}

/// Molecular-surface defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfacePrefs {
    pub style: SurfaceStylePref,
    /// Surface transparency as a whole percent, 0 (opaque) ..= 100 (invisible).
    /// Stored as a percent (not the renderer's 0.0..=1.0 fraction) because the
    /// UI presents a "%"-suffixed integer; the frontend divides by 100 when
    /// seeding [`crate::frontend::ViewportSurfaceState::transparency`].
    pub transparency_percent: u8,
}

impl Default for SurfacePrefs {
    fn default() -> Self {
        // Mirror `ViewportSurfaceState::default()`: Mesh, transparency 0.8.
        Self {
            style: SurfaceStylePref::Mesh,
            transparency_percent: 80,
        }
    }
}

/// The complete, persisted representation defaults. Lives in
/// [`crate::backend::config::AppConfig`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RepresentationPrefs {
    pub base: BasePrefs,
    pub cartoon: CartoonPrefs,
    pub surface: SurfacePrefs,
}

/// One in-place edit to [`RepresentationPrefs`] — a single variant per live
/// field, mirroring the `EditMdRunStage { edit: MdStageEdit }` pattern rather
/// than a separate `AppAction` per setting. Carried by
/// `AppAction::SetRepresentation` and applied in the dispatcher.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RepresentationEdit {
    DefaultBaseStyle(BaseStyle),
    HelixWidth(f32),
    HelixThickness(f32),
    SheetWidth(f32),
    SheetThickness(f32),
    CoilWidth(f32),
    CoilThickness(f32),
    Smoothing(u32),
    Profile(u32),
    SurfaceStyle(SurfaceStylePref),
    SurfaceTransparency(u8),
}

/// The three editable groups, used by "Restore Defaults" on each settings group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepresentationGroup {
    Base,
    Cartoon,
    Surface,
}

impl RepresentationPrefs {
    /// Apply one [`RepresentationEdit`] in place. The only validation is
    /// clamping surface transparency to 0..=100 (its UI is a free-typed
    /// `DragValue`); every other field is constrained by its control's range.
    pub fn apply(&mut self, edit: RepresentationEdit) {
        match edit {
            RepresentationEdit::DefaultBaseStyle(style) => self.base.default_style = style,
            RepresentationEdit::HelixWidth(value) => self.cartoon.helix_width = value,
            RepresentationEdit::HelixThickness(value) => self.cartoon.helix_thickness = value,
            RepresentationEdit::SheetWidth(value) => self.cartoon.sheet_width = value,
            RepresentationEdit::SheetThickness(value) => self.cartoon.sheet_thickness = value,
            RepresentationEdit::CoilWidth(value) => self.cartoon.coil_width = value,
            RepresentationEdit::CoilThickness(value) => self.cartoon.coil_thickness = value,
            RepresentationEdit::Smoothing(value) => self.cartoon.smoothing = value,
            RepresentationEdit::Profile(value) => self.cartoon.profile = value,
            RepresentationEdit::SurfaceStyle(style) => self.surface.style = style,
            RepresentationEdit::SurfaceTransparency(percent) => {
                self.surface.transparency_percent = percent.min(100);
            }
        }
    }

    /// Restore one group to its default, leaving the other groups untouched.
    pub fn reset_group(&mut self, group: RepresentationGroup) {
        let default = RepresentationPrefs::default();
        match group {
            RepresentationGroup::Base => self.base = default.base,
            RepresentationGroup::Cartoon => self.cartoon = default.cartoon,
            RepresentationGroup::Surface => self.surface = default.surface,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_edits_each_field() {
        let mut prefs = RepresentationPrefs::default();

        prefs.apply(RepresentationEdit::DefaultBaseStyle(BaseStyle::Sphere));
        prefs.apply(RepresentationEdit::HelixWidth(4.0));
        prefs.apply(RepresentationEdit::Smoothing(3));
        prefs.apply(RepresentationEdit::SurfaceStyle(SurfaceStylePref::Solid));

        assert_eq!(prefs.base.default_style, BaseStyle::Sphere);
        assert_eq!(prefs.cartoon.helix_width, 4.0);
        assert_eq!(prefs.cartoon.smoothing, 3);
        assert_eq!(prefs.surface.style, SurfaceStylePref::Solid);
    }

    #[test]
    fn apply_clamps_transparency() {
        let mut prefs = RepresentationPrefs::default();
        prefs.apply(RepresentationEdit::SurfaceTransparency(250));
        assert_eq!(prefs.surface.transparency_percent, 100);
    }

    #[test]
    fn reset_group_restores_only_that_group() {
        let mut prefs = RepresentationPrefs::default();
        prefs.apply(RepresentationEdit::DefaultBaseStyle(BaseStyle::Wire));
        prefs.apply(RepresentationEdit::HelixWidth(9.0));
        prefs.apply(RepresentationEdit::SurfaceTransparency(10));

        prefs.reset_group(RepresentationGroup::Cartoon);

        // Cartoon back to default; base and surface edits preserved.
        assert_eq!(prefs.cartoon, CartoonPrefs::default());
        assert_eq!(prefs.base.default_style, BaseStyle::Wire);
        assert_eq!(prefs.surface.transparency_percent, 10);
    }

    #[test]
    fn serde_round_trip() {
        let mut prefs = RepresentationPrefs::default();
        prefs.apply(RepresentationEdit::DefaultBaseStyle(BaseStyle::Stick));
        prefs.apply(RepresentationEdit::CoilThickness(1.25));
        prefs.apply(RepresentationEdit::SurfaceTransparency(42));

        let json = serde_json::to_string(&prefs).expect("serialize");
        let restored: RepresentationPrefs = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(prefs, restored);
    }
}
