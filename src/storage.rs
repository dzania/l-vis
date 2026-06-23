use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;

use crate::linear::{SnapshotSource, WorkspaceSnapshot};

#[derive(Clone, Debug)]
pub struct IssueCache {
    path: PathBuf,
}

impl IssueCache {
    pub fn new() -> Result<Self> {
        let project_dirs = ProjectDirs::from("dev", "l-vis", "l-vis")
            .context("could not determine a platform cache directory")?;
        Ok(Self {
            path: project_dirs.cache_dir().join("issues.json"),
        })
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn load(&self) -> Result<Option<WorkspaceSnapshot>> {
        if !self.path.exists() {
            return Ok(None);
        }

        let body = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        let mut snapshot: WorkspaceSnapshot =
            serde_json::from_str(&body).context("failed to decode issue cache")?;
        snapshot.source = SnapshotSource::Cache;
        Ok(Some(snapshot))
    }

    pub fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let body =
            serde_json::to_string_pretty(snapshot).context("failed to encode issue cache")?;
        fs::write(&self.path, body)
            .with_context(|| format!("failed to write {}", self.path.display()))?;
        Ok(())
    }
}
