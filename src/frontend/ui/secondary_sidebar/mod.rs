//! Per-task detail panels. These render the body of a `DockTab::Task` tab in
//! whichever dock area hosts it; the dock-area chrome (tab strip, drag-and-drop,
//! the generic `render_task_body` wrapper) lives in [`super::dock`]. The shared
//! imports below are re-exported to the submodules via their `use super::*`.

use eframe::egui::{self, Align, Frame, Layout, RichText};

use crate::frontend::{
    actions::AppAction,
    state::{AppState, CoordinateOptimizationScope},
};

mod disorder;
mod md_run;
mod md_system;
mod stage_detail;
mod task_panels;

pub(crate) use disorder::*;
pub(crate) use md_run::*;
pub(crate) use md_system::*;
pub(crate) use stage_detail::*;
pub(crate) use task_panels::*;
