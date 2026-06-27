use std::collections::BTreeMap;

use crate::frontend::state::{
    DisorderedSystemPrompt, DockingPrompt, EngineDraft, EntryListState, LayoutState, MdRunPrompt,
    MdSystemPrompt, OptimizationPrompt, ProteinPrepPrompt, QmPrompt, RemoteHostDraft,
    RemoteHostStatus, SupercellPrompt,
};
use crate::frontend::{
    AtomSelection, BuildingBlockEditor, CommandConsoleState, NanosheetBuilderPanel,
    ReticularBuilderPanel, StructureEditor, ViewCamera, ViewportVisualState,
    viewport::ViewportCache,
};

use super::monitor::{MonitorHistory, RemoteGpuLive};

/// State backing the Settings primary view. The engine registry is probed
/// lazily (probing spawns `--version` subprocesses) and cached here.
#[derive(Debug, Clone, Default)]
pub struct SettingsState {
    pub engine_registry: Option<crate::engines::registry::EngineRegistry>,
    pub engine_drafts: BTreeMap<String, EngineDraft>,
    /// When engine `--version` strings were last detected. Detection is slow
    /// (a WSL launch cold-starts the VM), so it runs only on explicit user
    /// request; the panel shows this so the displayed versions can be judged.
    pub engine_versions_checked_at: Option<std::time::SystemTime>,
    /// Free-text filter for the settings panel sections.
    pub search_query: String,
    /// Category selected in the Settings modal's left rail. Drives which
    /// category's groups the right pane shows while the search box is empty.
    pub selected_category: crate::frontend::ui::settings_registry::SettingCategory,
    /// Per-host editable drafts (keyed by host id) for the Remote Hosts panel.
    pub remote_host_drafts: BTreeMap<String, RemoteHostDraft>,
    /// The "add a host" form draft.
    pub new_remote_host: RemoteHostDraft,
    /// Per-host connection status (keyed by host id).
    pub remote_status: BTreeMap<String, RemoteHostStatus>,
    /// When a host's passwordless setup is being shown: `(host_id, install_cmd)`.
    pub remote_bootstrap: Option<(String, String)>,
    /// Cached remote hardware inventory, keyed by host id (Hardware ▸ Remote).
    pub remote_hardware: BTreeMap<String, crate::engines::remote::hardware::RemoteHardwareInfo>,
    /// Remote host currently selected in the remote-hardware panel.
    pub remote_hardware_host: Option<String>,
    /// Live remote-GPU monitoring data for the host currently being watched
    /// (Hardware ▸ Remote host ▸ Live GPU). `None` when no monitor is running.
    pub remote_gpu_live: Option<RemoteGpuLive>,
}

/// State backing the Style primary view — the per-structure view and
/// representation properties relocated out of Settings. These belong to the
/// structure currently being viewed, not to global app preferences.
#[derive(Debug, Clone, Default)]
pub struct StyleState {
    /// Free-text filter for the Style panel sections.
    pub search_query: String,
}

pub struct UiState {
    pub layout: LayoutState,
    pub entry_list: EntryListState,
    pub settings: SettingsState,
    pub style: StyleState,
    pub camera: ViewCamera,
    pub viewport_cache: ViewportCache,
    /// Set once at startup when the GPU molecule renderer initializes
    /// successfully; gates the GPU rendering path in the viewport.
    pub gpu_ready: bool,
    /// Detected GPU adapter name (from the wgpu render state at startup). `None`
    /// when the renderer doesn't expose one. Display-only.
    pub gpu_name: Option<String>,
    /// Resolved once per frame: whether the frosted-glass material should be
    /// revealed (user enabled it, the platform supports it, and Reduce
    /// Transparency is off). Drives the transparent clear color and the
    /// semi-transparent chrome fills. See [`crate::frontend::glass`].
    pub glass_active: bool,
    /// Effective chrome-fill alpha while glass is revealed this frame, mapped
    /// from the persisted `glass_intensity`; `None` means opaque chrome (glass
    /// off, unsupported, or Reduce Transparency on). Resolved next to
    /// `glass_active` and passed to [`crate::frontend::theme::chrome_fill`].
    pub glass_alpha: Option<u8>,
    pub hovered_atom: Option<usize>,
    pub selection: AtomSelection,
    pub viewport: ViewportVisualState,
    pub project_viewport: ViewportVisualState,
    pub entry_viewports: BTreeMap<u64, ViewportVisualState>,
    pub scripted_viewport_size: [u32; 2],
    pub console: CommandConsoleState,
    pub editor: Option<StructureEditor>,
    pub sketcher: Option<crate::frontend::sketcher::SketcherState>,
    pub reticular_builder: Option<ReticularBuilderPanel>,
    pub nanosheet_builder: Option<NanosheetBuilderPanel>,
    pub block_editor: Option<BuildingBlockEditor>,
    pub pending_optimization: Option<OptimizationPrompt>,
    pub pending_qm: Option<QmPrompt>,
    pub pending_supercell: Option<SupercellPrompt>,
    pub pending_protein_prep: Option<ProteinPrepPrompt>,
    pub pending_md_system: Option<MdSystemPrompt>,
    pub pending_md_run: Option<MdRunPrompt>,
    pub pending_disorder: Option<DisorderedSystemPrompt>,
    pub pending_docking: Option<DockingPrompt>,
    pub pending_pdb_fetch: Option<String>,
    /// The single active non-modal notification (a message plus optional action
    /// buttons), or `None`. Posting a new one replaces any current one.
    pub notification: Option<crate::frontend::actions::Notification>,
    /// The active entry whose full-detail render is held pending the user's
    /// answer to the heavy-structure wireframe suggestion, or `None` when nothing
    /// is gated. While set (and equal to the active entry) the viewport shows the
    /// prompt instead of the molecule, rather than silently simplifying it.
    pub pending_heavy_gate: Option<u64>,
    /// Entries for which the user has already answered the heavy-structure
    /// suggestion this session (accepted wireframe or chose full detail), so the
    /// prompt is not raised again on every re-activation. Transient.
    pub heavy_render_decided: std::collections::BTreeSet<u64>,
    /// Cached solvation count preview for the System Builder panel. Recomputed
    /// (which opens the force-field DB and grid-fills the box) only when
    /// `md_solvation_preview_key` changes, so the panel stays responsive.
    pub md_solvation_preview:
        Option<Result<crate::workflows::molecular_dynamics::SolvationEstimate, String>>,
    pub md_solvation_preview_key: u64,
    /// Active trajectory playback (loaded from an MD-output entry's run
    /// directory), or `None` when nothing is playing.
    pub trajectory: Option<crate::frontend::trajectory::TrajectoryPlayback>,
    /// A newer published release found by the background update check; renders
    /// a link to the release page in the status bar.
    pub available_update: Option<crate::io::update_check::AvailableUpdate>,
    /// An open plain-text viewer window, or `None` when closed. General
    /// purpose: any tool with textual output (QM reports, future engines)
    /// shows it here rather than adding its own window.
    pub text_viewer: Option<TextViewer>,
    /// Progress of a one-click in-place self-update (the download/replace
    /// triggered from the update badge), distinct from `available_update`
    /// which only records that a newer release *exists*.
    pub self_update: SelfUpdateStatus,
    /// In-app LLM assistant session: neutral conversation history, the turn
    /// state machine, the in-flight tool batch, and the Assistant-tab transcript.
    /// Like the editor sessions above it lives across frames; only the
    /// dispatcher and the poll-driven loop mutate it.
    pub agent: crate::frontend::agent::AgentSession,
    /// Parse/image cache backing the `egui_commonmark` viewer that formats the
    /// assistant's Markdown replies. Transient render state (egui texture
    /// handles, per-frame layout): never persisted, lives across frames, and is
    /// a sibling of `agent` so the transcript can be read while the cache is
    /// mutated during rendering.
    pub markdown_cache: egui_commonmark::CommonMarkCache,
    /// Latest CPU utilization sample (0–100 %). Updated by `poll_metrics` while
    /// the sampler is running; 0.0 when the sampler is off.
    pub cpu_pct: f32,
    /// Latest per-GPU live samples (util / VRAM / temp), one per GPU the sampler
    /// could read. Empty when the sampler is off or no live backend is available
    /// (then the gauges read N/A). Joined to the GPU inventory by PCI bus id.
    pub gpus: Vec<crate::frontend::gpu_monitor::GpuSample>,
    /// Latest memory utilization sample (0–100 %), or `None` when unavailable.
    pub mem_pct: Option<f32>,
    /// Rolling utilization history feeding the monitor popover sparklines.
    pub monitor_history: MonitorHistory,
    /// Snapshot of the global remote-job registry for display in the task
    /// monitor. Refreshed from `jobs.db` on submit, on an opt-in refresh, and
    /// after a scratch cleanup — never polled automatically.
    pub remote_jobs: Vec<crate::backend::storage::jobs::RemoteJob>,
}

/// Lifecycle of a user-initiated in-place update: idle until the user clicks
/// "update", downloading while the worker replaces the executable, then either
/// installed (offer a restart) or failed (show the error, leave the releases
/// link as a fallback).
#[derive(Default, Clone)]
pub enum SelfUpdateStatus {
    #[default]
    Idle,
    Downloading,
    Installed {
        version: String,
    },
    Failed {
        error: String,
    },
}

/// A read-only plain-text document shown in the shared viewer window: a
/// window title and the text to display (monospace, scrollable).
pub struct TextViewer {
    pub title: String,
    pub text: String,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            layout: LayoutState::default(),
            entry_list: EntryListState::default(),
            settings: SettingsState::default(),
            style: StyleState::default(),
            camera: ViewCamera::default(),
            viewport_cache: ViewportCache::default(),
            gpu_ready: false,
            gpu_name: None,
            glass_active: false,
            glass_alpha: None,
            hovered_atom: None,
            selection: AtomSelection::default(),
            viewport: ViewportVisualState::default(),
            project_viewport: ViewportVisualState::default(),
            entry_viewports: BTreeMap::new(),
            scripted_viewport_size: [1180, 760],
            console: CommandConsoleState::default(),
            editor: None,
            sketcher: None,
            reticular_builder: None,
            nanosheet_builder: None,
            block_editor: None,
            pending_optimization: None,
            pending_qm: None,
            pending_supercell: None,
            pending_protein_prep: None,
            pending_md_system: None,
            pending_md_run: None,
            pending_disorder: None,
            pending_docking: None,
            pending_pdb_fetch: None,
            notification: None,
            pending_heavy_gate: None,
            heavy_render_decided: std::collections::BTreeSet::new(),
            md_solvation_preview: None,
            md_solvation_preview_key: 0,
            trajectory: None,
            available_update: None,
            text_viewer: None,
            self_update: SelfUpdateStatus::default(),
            agent: crate::frontend::agent::AgentSession::default(),
            markdown_cache: egui_commonmark::CommonMarkCache::default(),
            cpu_pct: 0.0,
            gpus: Vec::new(),
            mem_pct: None,
            monitor_history: MonitorHistory::default(),
            remote_jobs: Vec::new(),
        }
    }
}
