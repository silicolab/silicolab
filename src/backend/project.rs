use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::backend::{
    config::{
        AppConfig, RecentProject, remember_recent_project, save_config, save_recent_projects,
    },
    storage::{
        ProjectSnapshot, ProjectSnapshotRef, initialize_project_databases, load_project_snapshot,
        save_project_snapshot, save_project_snapshot_ref,
    },
};

pub const SILICOLAB_DIR: &str = ".silicolab";
pub const PROJECT_DB: &str = "project.db";
pub const COMPOUNDS_DB: &str = "compounds.db";
pub const PROJECT_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct ProjectSession {
    pub root: PathBuf,
    pub silicolab_dir: PathBuf,
    pub project_db: PathBuf,
    pub compounds_db: PathBuf,
    pub name: String,
}

impl ProjectSession {
    pub fn from_root(root: PathBuf, name: String) -> Self {
        let silicolab_dir = root.join(SILICOLAB_DIR);
        Self {
            project_db: silicolab_dir.join(PROJECT_DB),
            compounds_db: silicolab_dir.join(COMPOUNDS_DB),
            silicolab_dir,
            root,
            name,
        }
    }
}

#[derive(Debug, Clone)]
pub enum WorkspaceSession {
    Scratch,
    Project(ProjectSession),
}

impl WorkspaceSession {
    pub fn is_project(&self) -> bool {
        matches!(self, Self::Project(_))
    }

    pub fn project(&self) -> Option<&ProjectSession> {
        match self {
            Self::Project(project) => Some(project),
            Self::Scratch => None,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Scratch => "Scratch".to_string(),
            Self::Project(project) => project.name.clone(),
        }
    }
}

pub fn is_valid_project_dir(path: &Path) -> bool {
    path.join(SILICOLAB_DIR).is_dir() && path.join(SILICOLAB_DIR).join(PROJECT_DB).is_file()
}

pub fn create_project(root: &Path, name: &str) -> Result<ProjectSession> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("project name cannot be empty");
    }
    let project_root = root.join(trimmed);
    std::fs::create_dir_all(project_root.join(SILICOLAB_DIR))
        .with_context(|| format!("failed to create {}", project_root.display()))?;
    let session = ProjectSession::from_root(project_root, trimmed.to_string());
    initialize_project_databases(&session)?;
    crate::backend::housekeeping::write_manifest(&session)?;
    Ok(session)
}

pub fn open_project(path: &Path) -> Result<(ProjectSession, ProjectSnapshot)> {
    if !is_valid_project_dir(path) {
        bail!("not a valid SilicoLab project");
    }
    let fallback_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("SilicoLab Project")
        .to_string();
    let mut session = ProjectSession::from_root(path.to_path_buf(), fallback_name);
    // Check the plaintext manifest before touching the databases so an
    // incompatible (newer-format) project fails fast with a clear message.
    crate::backend::housekeeping::check_manifest_compatibility(&session)?;
    let snapshot = load_project_snapshot(&session)?;
    session.name = snapshot.name.clone();
    Ok((session, snapshot))
}

pub fn save_project(
    session: &ProjectSession,
    snapshot: &ProjectSnapshot,
    persist_history: bool,
) -> Result<()> {
    save_project_snapshot(session, snapshot, persist_history)?;
    // Keep the manifest's recorded versions in step with what wrote the DBs.
    crate::backend::housekeeping::write_manifest(session)?;
    Ok(())
}

/// Borrowed-input variant of [`save_project`] for the autosave hot path: saves
/// straight from references into the live application state without cloning the
/// workspace.
pub fn save_project_ref(
    session: &ProjectSession,
    snapshot: &ProjectSnapshotRef<'_>,
    persist_history: bool,
) -> Result<()> {
    save_project_snapshot_ref(session, snapshot, persist_history)?;
    crate::backend::housekeeping::write_manifest(session)?;
    Ok(())
}

pub fn remember_opened_project(
    config: &mut AppConfig,
    recent_projects: &mut Vec<RecentProject>,
    project: &ProjectSession,
) -> Result<()> {
    config.last_project_path = Some(project.root.clone());
    config.closed_to_scratch = false;
    config.default_project_dir = project
        .root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| project.root.clone());
    save_config(config)?;

    remember_recent_project(recent_projects, &project.root, &project.name);
    save_recent_projects(recent_projects)
}
