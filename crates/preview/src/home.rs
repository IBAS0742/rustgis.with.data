//! The persistent data home (`RUSTGIS_PREVIEW_HOME`).
//!
//! Everything the preview server owns lives under one directory so a Docker volume can preserve it
//! independently from read-only source data:
//!
//! ```text
//! <home>/config/     source and database configuration
//! <home>/projects/   project metadata, immutable versions and working copies
//! <home>/cache/      rebuildable derived data
//! <home>/logs/       server logs
//! ```

use std::path::{Path, PathBuf};

use crate::StoreError;

/// The directory holding all preview state.
#[derive(Clone, Debug)]
pub struct DataHome {
    root: PathBuf,
}

impl DataHome {
    /// Opens (and creates) the home named by `RUSTGIS_PREVIEW_HOME`, or the platform default.
    pub fn from_env() -> Result<Self, StoreError> {
        let root = match std::env::var_os("RUSTGIS_PREVIEW_HOME") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            _ => default_home()?,
        };
        Self::open(root)
    }

    /// Opens (and creates) a home at `root`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let root = root.as_ref();
        std::fs::create_dir_all(root)?;
        let root = std::fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(StoreError::InvalidInput(format!("data home is not a directory: {}", root.display())));
        }
        for dir in ["config", "projects", "cache", "logs"] {
            std::fs::create_dir_all(root.join(dir))?;
        }
        Ok(DataHome { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    pub fn projects(&self) -> PathBuf {
        self.root.join("projects")
    }

    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
}

/// The default data home. Keeping it out of the checkout prevents tests and a development run from
/// writing into the repository.
pub fn default_home() -> Result<PathBuf, StoreError> {
    if cfg!(target_os = "windows") {
        if let Some(base) = std::env::var_os("LOCALAPPDATA") {
            return Ok(PathBuf::from(base).join("RustGIS Preview"));
        }
        if let Some(base) = std::env::var_os("APPDATA") {
            return Ok(PathBuf::from(base).join("RustGIS Preview"));
        }
    }
    if let Some(base) = std::env::var_os("XDG_DATA_HOME")
        && !base.is_empty()
    {
        return Ok(PathBuf::from(base).join("rustgis-preview"));
    }
    if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        return Ok(PathBuf::from(home).join(".local/share/rustgis-preview"));
    }
    std::env::current_dir().map(|dir| dir.join("rustgis-preview-home")).map_err(StoreError::Io)
}
