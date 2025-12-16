use anyhow::Result;
use std::path::{Path, PathBuf};

/// Global configuration derived from CLI arguments.
pub struct Config {
    /// Absolute path to the project root being analyzed.
    pub project_root: PathBuf,
    /// Directory where Plex stores its index and data (.plex/).
    pub data_dir: PathBuf,
}

impl Config {
    /// Create a new config rooted at `project_path`.
    /// Creates the `.plex/` data directory if it doesn't exist.
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

    /// Path to the SQLite database file.
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("index.db")
    }

    /// Global directory for cached models (~/.local/share/plex/models/).
    pub fn models_dir() -> PathBuf {
        let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
        let dir = base.join("plex").join("models");
        std::fs::create_dir_all(&dir).ok();
        dir
    }

    /// Return the project name (last component of the root path).
    pub fn project_name(&self) -> &str {
        self.project_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
    }

    /// Check whether a path should be ignored (hidden dirs, node_modules, etc.).
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
