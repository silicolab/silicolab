//! 2D molecule sketcher — transient editor state and rendering.
//!
//! [`SketcherState`] is the in-progress drawing held in `UiState::sketcher`. It
//! is *transient UI state* in the sense of `ARCHITECTURE.md`: the live canvas is
//! mutated directly each frame (tools, drags, undo/redo) and only the committed
//! result crosses the `AppAction → dispatch` boundary, exactly as the structure
//! editor's live preview does. See [`crate::frontend::dispatcher`] for the
//! commit path (`CommitSketch` → a new workspace entry).
//!
//! The module is split into:
//!   * this file — state, tools, and the mutation primitives,
//!   * [`placement`] — ring-template fusion (pure model-space geometry),
//!   * [`input`] — pointer gestures → primitive calls,
//!   * [`canvas`] — the painter (atoms, bonds, labels, previews),
//!   * [`palette`] — the tool/element/bond/fragment palettes,
//!   * [`view`] — the window shell that ties them together.

mod canvas;
mod input;
mod palette;
mod placement;
mod view;

use std::collections::BTreeSet;

use eframe::egui::{Pos2, Rect, Vec2};
use nalgebra::Point2;

use crate::domain::{
    BondType,
    chemistry::normalized_symbol,
    sketch::{RingTemplate, Sketch},
};

pub use view::render_sketcher_window;

/// Elements always shown in the common palette row.
pub const COMMON_ELEMENTS: [&str; 10] = ["H", "C", "N", "O", "P", "S", "F", "Cl", "Br", "I"];

const UNDO_LIMIT: usize = 200;
const DEFAULT_ZOOM: f32 = 36.0;

/// The active drawing tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SketchTool {
    /// Draw with the active element (carbon by default).
    Draw,
    /// Draw bonds of the active order.
    Bond,
    /// Lay a run of single bonds in one drag.
    Chain,
    /// Stamp a ring template (with live preview + fusion).
    Ring,
    /// Delete atoms / step bonds down.
    Erase,
    /// Marquee + click selection.
    Select,
    /// Translate (primary drag) / rotate (secondary drag).
    Move,
    /// Step formal charge: primary +1, secondary −1.
    Charge,
}

impl SketchTool {
    /// Tools that only make sense once something is drawn.
    pub fn needs_structure(self) -> bool {
        matches!(
            self,
            SketchTool::Select | SketchTool::Move | SketchTool::Erase
        )
    }
}

/// A pointer gesture in progress, persisted across frames. Coordinates are in
/// model space.
#[derive(Debug, Clone)]
enum Gesture {
    /// Rubber-band a bond from `from` (or an empty anchor) to the release point.
    DrawBond {
        from: Option<usize>,
        anchor: Point2<f32>,
        order: BondType,
    },
    /// Bond-chain: keep appending single bonds as the pointer moves.
    Chain { last: usize, count: usize },
    /// Rectangular marquee selection.
    Marquee { start: Point2<f32>, additive: bool },
    /// Translate a set of atoms (empty `atoms` = the whole sketch).
    Translate {
        last: Point2<f32>,
        atoms: Vec<usize>,
        whole: bool,
    },
    /// Rotate atoms about `center`; `accum` tracks the displayed angle.
    Rotate {
        center: Point2<f32>,
        last: f32,
        accum: f32,
        atoms: Vec<usize>,
        whole: bool,
    },
    /// Erase by dragging across atoms/bonds.
    Erase,
}

/// The transient sketcher session.
pub struct SketcherState {
    pub sketch: Sketch,
    pub tool: SketchTool,
    pub active_element: String,
    /// Recently-used exotic (non-common) elements, most-recent first.
    pub recent_elements: Vec<String>,
    pub active_bond: BondType,
    pub active_ring: RingTemplate,
    pub selected_atoms: BTreeSet<usize>,
    pub selected_bonds: BTreeSet<usize>,
    /// Screen-space offset of the model origin from the canvas centre.
    pub pan: Vec2,
    /// Pixels per model unit.
    pub zoom: f32,
    pub title: String,
    pub smiles_input: String,
    pub heteroatom_color: bool,
    pub status: String,
    /// Free-text element entry (type a symbol to set the active element).
    pub element_query: String,
    /// Periodic-table popup open?
    pub show_periodic_table: bool,
    /// One-shot request to recentre the view (consumed by the canvas).
    needs_fit: bool,
    undo: Vec<Sketch>,
    redo: Vec<Sketch>,
    gesture: Option<Gesture>,
}

impl Default for SketcherState {
    fn default() -> Self {
        Self {
            sketch: Sketch::new(),
            tool: SketchTool::Draw,
            active_element: "C".to_string(),
            recent_elements: Vec::new(),
            active_bond: BondType::Single,
            active_ring: RingTemplate::Benzene,
            selected_atoms: BTreeSet::new(),
            selected_bonds: BTreeSet::new(),
            pan: Vec2::ZERO,
            zoom: DEFAULT_ZOOM,
            title: "Sketch".to_string(),
            smiles_input: String::new(),
            heteroatom_color: true,
            status: String::new(),
            element_query: String::new(),
            show_periodic_table: false,
            needs_fit: false,
            undo: Vec::new(),
            redo: Vec::new(),
            gesture: None,
        }
    }
}

impl SketcherState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a sketcher seeded with an existing sketch (e.g. from SMILES import).
    pub fn with_sketch(sketch: Sketch, title: impl Into<String>) -> Self {
        Self {
            sketch,
            title: title.into(),
            needs_fit: true,
            ..Self::default()
        }
    }

    // --- History ---------------------------------------------------------

    /// Snapshot the current sketch before a mutating edit.
    fn push_undo(&mut self) {
        self.undo.push(self.sketch.clone());
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(previous) = self.undo.pop() {
            self.redo
                .push(std::mem::replace(&mut self.sketch, previous));
            self.clamp_selection();
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.sketch, next));
            self.clamp_selection();
        }
    }

    fn clamp_selection(&mut self) {
        let atoms = self.sketch.atoms.len();
        let bonds = self.sketch.bonds.len();
        self.selected_atoms.retain(|index| *index < atoms);
        self.selected_bonds.retain(|index| *index < bonds);
    }

    // --- Palette setters -------------------------------------------------

    /// Make `symbol` the active element and switch to the draw tool. Exotic
    /// elements are remembered in the recents row.
    pub fn set_element(&mut self, symbol: &str) {
        let symbol = normalized_symbol(symbol);
        if symbol.is_empty() {
            return;
        }
        if !COMMON_ELEMENTS.contains(&symbol.as_str()) {
            self.recent_elements.retain(|element| element != &symbol);
            self.recent_elements.insert(0, symbol.clone());
            self.recent_elements.truncate(4);
        }
        self.active_element = symbol;
        self.tool = SketchTool::Draw;
    }

    // --- Selection -------------------------------------------------------

    pub fn select_all(&mut self) {
        self.selected_atoms = (0..self.sketch.atoms.len()).collect();
        self.selected_bonds = (0..self.sketch.bonds.len()).collect();
    }

    pub fn invert_selection(&mut self) {
        let atoms: BTreeSet<usize> = (0..self.sketch.atoms.len())
            .filter(|index| !self.selected_atoms.contains(index))
            .collect();
        let bonds: BTreeSet<usize> = (0..self.sketch.bonds.len())
            .filter(|index| !self.selected_bonds.contains(index))
            .collect();
        self.selected_atoms = atoms;
        self.selected_bonds = bonds;
    }

    pub fn clear_selection(&mut self) {
        self.selected_atoms.clear();
        self.selected_bonds.clear();
    }

    pub fn has_selection(&self) -> bool {
        !self.selected_atoms.is_empty() || !self.selected_bonds.is_empty()
    }

    /// Delete the current selection (atoms with their bonds, plus selected
    /// bonds). Returns whether anything was removed. Any in-progress gesture is
    /// abandoned, since it may reference now-stale atom indices.
    pub fn delete_selection(&mut self) -> bool {
        if !self.has_selection() {
            return false;
        }
        self.push_undo();
        // Remove standalone selected bonds first (descending so indices hold),
        // then the selected atoms (which drops their incident bonds).
        let mut bonds: Vec<usize> = self.selected_bonds.iter().copied().collect();
        bonds.sort_unstable_by(|a, b| b.cmp(a));
        for index in bonds {
            self.sketch.remove_bond(index);
        }
        let atoms: Vec<usize> = self.selected_atoms.iter().copied().collect();
        self.sketch.remove_atoms(&atoms);
        self.clear_selection();
        self.gesture = None;
        true
    }

    // --- Whole-sketch operations ----------------------------------------

    /// Step the formal charge of the selection (or all atoms if none selected).
    pub fn step_charge(&mut self, delta: i32) {
        let targets: Vec<usize> = if self.selected_atoms.is_empty() {
            (0..self.sketch.atoms.len()).collect()
        } else {
            self.selected_atoms.iter().copied().collect()
        };
        if targets.is_empty() {
            return;
        }
        self.push_undo();
        for index in targets {
            self.sketch.adjust_charge(index, delta);
        }
    }

    /// Force a recentre on the next frame.
    pub fn request_fit(&mut self) {
        self.needs_fit = true;
    }

    pub fn clean_up(&mut self) {
        if self.sketch.atoms.len() < 2 {
            return;
        }
        self.push_undo();
        let selection: Option<Vec<usize>> = if self.selected_atoms.len() >= 2 {
            Some(self.selected_atoms.iter().copied().collect())
        } else {
            None
        };
        crate::domain::sketch::clean_up(&mut self.sketch, selection.as_deref());
        self.needs_fit = selection.is_none();
    }

    pub fn flip_horizontal(&mut self) {
        if self.sketch.is_empty() {
            return;
        }
        self.push_undo();
        self.sketch.flip_horizontal();
    }

    pub fn flip_vertical(&mut self) {
        if self.sketch.is_empty() {
            return;
        }
        self.push_undo();
        self.sketch.flip_vertical();
    }

    /// Replace the drawing with one parsed from a SMILES string.
    pub fn import_smiles(&mut self, text: &str) {
        match crate::domain::smiles::parse(text) {
            Ok(sketch) => {
                self.push_undo();
                self.sketch = sketch;
                self.clear_selection();
                self.needs_fit = true;
                self.status = format!("Imported SMILES ({} atoms)", self.sketch.atoms.len());
            }
            Err(error) => {
                self.status = format!("SMILES error: {error}");
            }
        }
    }

    /// Current drawing as a SMILES string.
    pub fn export_smiles(&self) -> String {
        crate::domain::smiles::to_smiles(&self.sketch)
    }

    /// Commit a ring placement (template fusion) into the sketch.
    fn commit_ring(&mut self, placement: &placement::RingPlacement) {
        self.push_undo();
        let mut mapping = vec![0usize; placement.positions.len()];
        for (local, vertex) in placement.vertices.iter().enumerate() {
            mapping[local] = match vertex {
                Some(existing) => *existing,
                None => self.sketch.add_atom("C", placement.positions[local]),
            };
        }
        for &(a, b, order) in &placement.bonds {
            let (a, b) = (mapping[a], mapping[b]);
            if self.sketch.bond_between(a, b).is_none() {
                self.sketch.add_bond(a, b, order);
            }
        }
    }

    // --- View transform --------------------------------------------------

    pub fn to_screen(&self, rect: Rect, model: Point2<f32>) -> Pos2 {
        rect.center() + self.pan + Vec2::new(model.x * self.zoom, -model.y * self.zoom)
    }

    pub fn to_model(&self, rect: Rect, screen: Pos2) -> Point2<f32> {
        let delta = screen - rect.center() - self.pan;
        Point2::new(delta.x / self.zoom, -delta.y / self.zoom)
    }

    /// Recentre and rescale so the whole drawing fits `rect`.
    pub fn fit(&mut self, rect: Rect) {
        match self.sketch.bounds() {
            Some((min, max)) if rect.width() > 1.0 && rect.height() > 1.0 => {
                let width = (max.x - min.x).max(BOND_UNIT);
                let height = (max.y - min.y).max(BOND_UNIT);
                let margin = 0.82;
                let zoom_x = rect.width() * margin / width;
                let zoom_y = rect.height() * margin / height;
                self.zoom = zoom_x.min(zoom_y).clamp(10.0, 120.0);
                let center = Point2::new((min.x + max.x) * 0.5, (min.y + max.y) * 0.5);
                self.pan = Vec2::new(-center.x * self.zoom, center.y * self.zoom);
            }
            _ => {
                self.pan = Vec2::ZERO;
                self.zoom = DEFAULT_ZOOM;
            }
        }
    }

    fn take_needs_fit(&mut self) -> bool {
        std::mem::take(&mut self.needs_fit)
    }
}

const BOND_UNIT: f32 = 1.5;
