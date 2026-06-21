//! GROMACS topology (`.top`) handling.
//!
//! The caller passes a [`TopologySource`] that either points at an existing
//! file or provides an inline `.top` body; it is materialized inside the run's
//! working directory.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

/// How the topology is supplied for a GROMACS run.
#[derive(Debug, Clone)]
pub enum TopologySource {
    /// Path to a topology file (`.top` / `.itp`) on disk.
    File(PathBuf),
    /// Inline topology text. Useful for AI-agent driven runs.
    Inline(String),
}

impl TopologySource {
    /// Write the topology into `target_dir` and return the resulting path.
    pub fn materialize(&self, target_dir: &Path, file_name: &str) -> Result<PathBuf> {
        fs::create_dir_all(target_dir).with_context(|| {
            format!(
                "failed to create topology directory {}",
                target_dir.display()
            )
        })?;
        let destination = target_dir.join(file_name);
        match self {
            Self::File(path) => {
                fs::copy(path, &destination).with_context(|| {
                    format!(
                        "failed to copy topology from {} to {}",
                        path.display(),
                        destination.display()
                    )
                })?;
            }
            Self::Inline(body) => {
                fs::write(&destination, body).with_context(|| {
                    format!("failed to write topology to {}", destination.display())
                })?;
            }
        }
        Ok(destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_topology_is_written_verbatim() {
        let target = std::env::temp_dir().join("silicolab_topology_inline_test");
        let _ = fs::remove_dir_all(&target);

        let source = TopologySource::Inline("; inline topology\n".to_string());
        let path = source.materialize(&target, "topol.top").expect("write");

        let body = fs::read_to_string(path).expect("read back");
        assert!(body.contains("inline topology"));
    }

    #[test]
    fn file_topology_is_copied() {
        let target = std::env::temp_dir().join("silicolab_topology_file_test");
        let source_path = std::env::temp_dir().join("silicolab_topology_file_test_source.top");
        let _ = fs::remove_dir_all(&target);
        fs::write(&source_path, "; from disk\n").expect("seed");

        let source = TopologySource::File(source_path.clone());
        let destination = source.materialize(&target, "topol.top").expect("copy");

        let body = fs::read_to_string(destination).expect("read back");
        assert!(body.contains("from disk"));
        let _ = fs::remove_file(&source_path);
    }
}
