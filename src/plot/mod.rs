//! UI-free charting core: the shared [`spec::ChartSpec`] data model, tick/range
//! layout, a deterministic publication SVG emitter, and the SVG→PNG/PDF export
//! pipeline. Both the on-screen egui_plot adapter and the exporters consume the
//! same spec, so a chart looks the same on screen and on paper.

pub mod export;
pub mod layout;
pub mod spec;

/// The one font embedded in every export, so output is identical on every
/// machine regardless of installed system fonts.
pub const EXPORT_FONT: &[u8] = include_bytes!("../../assets/fonts/LiberationSans-Regular.ttf");
pub const EXPORT_FONT_FAMILY: &str = "Liberation Sans";

pub mod svg;
