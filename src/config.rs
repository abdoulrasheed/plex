use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct Config {
    pub project_root: PathBuf,
    pub data_dir: PathBuf,
}

impl Config {
    pub fn new(project_path: PathBuf) -> Result<Self> {
        let project_root = if project_path.is_absolute() {
            project_path
        } else {
            std::env::current_dir()?.join(&project_path)
        };
        let project_root = std::fs::canonicalize(&project_root)?;
        let data_dir = project_root.join(".plex");
        std::fs::create_dir_all(&data_dir)?;

        Ok(Config {
            project_root,
            data_dir,
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("index.db")
    }

    pub fn models_dir() -> PathBuf {
        let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
        let dir = base.join("plex").join("models");
        std::fs::create_dir_all(&dir).ok();
        dir
    }

    pub fn project_name(&self) -> &str {
        self.project_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
    }

    pub fn should_ignore(path: &Path) -> bool {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        matches!(
            name,
            ".git"
                | ".hg"
                | ".svn"
                | "node_modules"
                | "__pycache__"
                | ".plex"
                | "target"
                | "build"
                | "dist"
                | ".next"
                | ".nuxt"
                | "vendor"
                | ".venv"
                | "venv"
                | "env"
                | ".env"
                | ".tox"
                | ".mypy_cache"
                | ".pytest_cache"
        )
    }
}

