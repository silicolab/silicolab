use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub task: u64,
    pub relative: PathBuf,
}
impl Artifact {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.relative.as_os_str().is_empty()
                && self
                    .relative
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
            "invalid artifact path"
        );
        Ok(())
    }
}
#[derive(Debug, Serialize)]
pub struct Page {
    pub text: String,
    pub next_offset: u64,
    pub truncated: bool,
}

pub fn read_page(root: &Path, artifact: &Artifact, offset: u64, limit: usize) -> Result<Page> {
    artifact.validate()?;
    ensure!(
        (4..=2400).contains(&limit),
        "raw limit must be 4..2400 bytes"
    );
    let root = root.canonicalize()?;
    let path = root.join(&artifact.relative).canonicalize()?;
    ensure!(
        path.starts_with(&root),
        "artifact escapes owning run directory"
    );
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    ensure!(offset <= size, "offset exceeds artifact size");
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    let mut end = bytes.len().min(limit);
    while let Err(error) = std::str::from_utf8(&bytes[..end]) {
        ensure!(
            error.error_len().is_none(),
            "invalid UTF-8 or offset is not a UTF-8 boundary"
        );
        end = error.valid_up_to();
    }
    ensure!(end > 0 || offset == size, "invalid UTF-8 boundary");
    let text = std::str::from_utf8(&bytes[..end])?.to_owned();
    let next_offset = offset + end as u64;
    Ok(Page {
        text,
        next_offset,
        truncated: next_offset < size,
    })
}

pub fn copy_registered_runs(
    tasks: &mut crate::backend::tasks::TaskManager,
    project_root: &Path,
) -> Result<()> {
    let records: Vec<_> = tasks
        .runs
        .records
        .all()
        .flat_map(|r| r.artifacts.clone())
        .collect();
    let task_ids: std::collections::BTreeSet<_> = records.iter().map(|a| a.task).collect();
    for task_id in task_ids {
        let task = tasks
            .task_run(task_id)
            .ok_or_else(|| anyhow::anyhow!("artifact task missing"))?;
        let source = task
            .run_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("artifact run directory missing"))?;
        let target = project_root
            .join("runs")
            .join(format!("evidence-{task_id}"));
        if source == &target {
            continue;
        }
        let root = source.canonicalize()?;
        for artifact in records.iter().filter(|a| a.task == task_id) {
            artifact.validate()?;
            let from = source.join(&artifact.relative).canonicalize()?;
            ensure!(from.starts_with(&root), "artifact escapes owning run");
            let to = target.join(&artifact.relative);
            let parent = to
                .parent()
                .ok_or_else(|| anyhow::anyhow!("artifact parent missing"))?;
            std::fs::create_dir_all(parent)?;
            if to.exists() {
                ensure!(
                    file_digest(&from)? == file_digest(&to)?,
                    "conflicting artifact at {}",
                    to.display()
                );
                continue;
            }
            let staging = to.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
            std::fs::copy(from, &staging)?;
            std::fs::OpenOptions::new()
                .write(true)
                .open(&staging)?
                .sync_all()?;
            std::fs::rename(staging, &to)?;
        }
        tasks.set_run_dir(task_id, target);
    }
    Ok(())
}

fn file_digest(path: &Path) -> Result<Vec<u8>> {
    use sha2::{Digest, Sha256};
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().to_vec())
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let staging = path.with_extension("tmp");
    {
        let mut file = File::create(&staging)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(staging, path)?;
    Ok(())
}
