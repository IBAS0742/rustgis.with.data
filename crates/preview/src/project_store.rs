//! Immutable project versions and mutable working copies.
//!
//! On disk a project is a directory:
//!
//! ```text
//! <home>/projects/<project-id>/
//!   project.json              metadata (name, version index, working copies)
//!   versions/<version-id>.rgis  published, never overwritten
//!   working/<working-id>.rgis   editable copies made from a version
//! ```
//!
//! A version is published by writing and syncing a temporary file in the same directory, then
//! linking it into place. On Unix `hard_link` fails if the name exists; on Windows `rename` does.
//! Only after publication is the metadata index atomically replaced. A crash therefore leaves
//! either no version or a complete, discoverable version; load reconciles files with the index.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rustgis_engine::Project;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::StoreError;
use crate::home::DataHome;
use crate::ids;

/// The same limit as `Project::from_json`, so a version cannot contain an unreadable project.
pub const MAX_PROJECT_BYTES: usize = 512 << 20;
const MAX_INDEX_BYTES: u64 = 8 << 20;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionMeta {
    pub id: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub size: u64,
    /// Hex SHA-256 of the project bytes, when the index has it. Reconciled versions may omit it.
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkingCopyMeta {
    pub id: String,
    #[serde(default)]
    pub base_version: Option<String>,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
    #[serde(default)]
    pub size: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMeta {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
    #[serde(default)]
    pub versions: Vec<VersionMeta>,
    #[serde(default)]
    pub working_copies: Vec<WorkingCopyMeta>,
}

/// A saved version's text. It is read-only: the API never accepts its ID as a write target.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectContent {
    pub project: ProjectMeta,
    pub version: VersionMeta,
    pub content: String,
    pub read_only: bool,
}

/// A mutable copy of a version. Saving it publishes a new version; the base stays untouched.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkingCopy {
    pub project: ProjectMeta,
    pub id: String,
    pub base_version: Option<String>,
    pub content: String,
    pub read_only: bool,
}

/// All project metadata in the home, with operations serialized by the caller (or by the HTTP
/// server) through ordinary Rust ownership.
pub struct ProjectStore {
    home: DataHome,
    projects: BTreeMap<String, ProjectMeta>,
}

impl ProjectStore {
    /// Reads the project index. Corrupt metadata is rebuilt from the version files instead of
    /// making all projects unopenable.
    pub fn open(home: DataHome) -> Result<Self, StoreError> {
        let mut projects = BTreeMap::new();
        let root = home.projects();
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if !ids::is_component(&id) {
                continue;
            }
            let meta = load_project(&entry.path(), &id)?;
            projects.insert(id, meta);
        }
        Ok(ProjectStore { home, projects })
    }

    pub fn home(&self) -> &DataHome {
        &self.home
    }

    pub fn projects(&self) -> Vec<ProjectMeta> {
        let mut out: Vec<_> = self.projects.values().cloned().collect();
        out.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms).then_with(|| a.name.cmp(&b.name)));
        out
    }

    pub fn project(&self, id: &str) -> Result<ProjectMeta, StoreError> {
        if !ids::is_component(id) {
            return Err(StoreError::InvalidInput(format!("invalid project id: {id}")));
        }
        self.projects.get(id).cloned().ok_or_else(|| StoreError::ProjectNotFound(id.to_owned()))
    }

    /// Creates a project and publishes its first version. Without content, a new empty project is
    /// used, so every project starts with an immutable V1 rather than an unsaved state.
    pub fn create_project(&mut self, name: &str, content: Option<String>) -> Result<ProjectContent, StoreError> {
        let name = clean_name(name)?;
        let text = match content {
            Some(text) => {
                validate_project(&text)?;
                text
            }
            None => serde_json::to_string(&Project::new(name.clone()))?,
        };
        let (id, dir) = self.new_project_dir()?;
        let versions = dir.join("versions");
        let working = dir.join("working");
        fs::create_dir_all(&versions)?;
        fs::create_dir_all(&working)?;
        let now = now_ms();
        let version = publish_version(&versions, &ids::new_id("ver"), &text, None)?;
        let meta = ProjectMeta {
            id: id.clone(),
            name,
            created_at_ms: now,
            updated_at_ms: now,
            versions: vec![version.clone()],
            working_copies: Vec::new(),
        };
        write_index(&dir, &meta)?;
        self.projects.insert(id, meta.clone());
        Ok(ProjectContent { project: meta, version, content: text, read_only: true })
    }

    pub fn versions(&self, project: &str) -> Result<Vec<VersionMeta>, StoreError> {
        let meta = self.project(project)?;
        Ok(meta.versions)
    }

    /// Reads a saved version. Its ID is never accepted by a write operation.
    pub fn read_version(&self, project: &str, version: &str) -> Result<ProjectContent, StoreError> {
        if !ids::is_component(version) {
            return Err(StoreError::InvalidInput(format!("invalid version id: {version}")));
        }
        let meta = self.project(project)?;
        let version_meta =
            meta.versions.iter().find(|v| v.id == version).cloned().ok_or_else(|| StoreError::VersionNotFound(version.to_owned()))?;
        let text = read_project_text(&self.project_dir(project)?.join("versions").join(format!("{version}.rgis")))?;
        validate_project(&text)?;
        Ok(ProjectContent { project: meta, version: version_meta, content: text, read_only: true })
    }

    /// Copies a saved version into a new mutable working copy.
    pub fn create_working_copy(&mut self, project: &str, version: &str) -> Result<WorkingCopy, StoreError> {
        let saved = self.read_version(project, version)?;
        let dir = self.project_dir(project)?;
        let working_dir = dir.join("working");
        fs::create_dir_all(&working_dir)?;
        let id = ids::new_id("wrk");
        let path = working_dir.join(format!("{id}.rgis"));
        write_new_text(&path, &saved.content)?;
        let now = now_ms();
        let working = WorkingCopyMeta {
            id: id.clone(),
            base_version: Some(version.to_owned()),
            created_at_ms: now,
            updated_at_ms: now,
            size: saved.content.len() as u64,
        };
        let mut meta = self.project(project)?;
        meta.working_copies.retain(|w| w.id != id);
        meta.working_copies.push(working);
        meta.updated_at_ms = now;
        write_index(&dir, &meta)?;
        self.projects.insert(project.to_owned(), meta.clone());
        Ok(WorkingCopy { project: meta, id, base_version: Some(version.to_owned()), content: saved.content, read_only: false })
    }

    /// Reads a working copy after a restart (in-progress edits are kept on disk).
    pub fn read_working_copy(&self, project: &str, working: &str) -> Result<WorkingCopy, StoreError> {
        if !ids::is_component(working) {
            return Err(StoreError::InvalidInput(format!("invalid working-copy id: {working}")));
        }
        let meta = self.project(project)?;
        let w = meta
            .working_copies
            .iter()
            .find(|w| w.id == working)
            .cloned()
            .ok_or_else(|| StoreError::WorkingCopyNotFound(working.to_owned()))?;
        let content = read_project_text(&self.project_dir(project)?.join("working").join(format!("{working}.rgis")))?;
        validate_project(&content)?;
        Ok(WorkingCopy { project: meta, id: w.id, base_version: w.base_version, content, read_only: false })
    }

    /// Saves a working copy as a new immutable version. Existing version names are never reused.
    /// The working file is updated first so a crash cannot lose edits that were already confirmed.
    pub fn save_working_copy(&mut self, project: &str, working: &str, content: &str) -> Result<VersionMeta, StoreError> {
        validate_project(content)?;
        if !ids::is_component(working) {
            return Err(StoreError::InvalidInput(format!("invalid working-copy id: {working}")));
        }
        let dir = self.project_dir(project)?;
        let mut meta = self.project(project)?;
        let w_index =
            meta.working_copies.iter().position(|w| w.id == working).ok_or_else(|| StoreError::WorkingCopyNotFound(working.to_owned()))?;
        let base = meta.working_copies.get(w_index).and_then(|w| w.base_version.clone());
        if let Some(base) = &base
            && !meta.versions.iter().any(|v| &v.id == base)
        {
            return Err(StoreError::InvalidInput(format!("working copy's base version is missing: {base}")));
        }
        let working_path = dir.join("working").join(format!("{working}.rgis"));
        rustgis_data::io::write_atomic(&working_path, content.as_bytes())?;
        let version = publish_version(&dir.join("versions"), &ids::new_id("ver"), content, base)?;
        let now = now_ms();
        if let Some(w) = meta.working_copies.get_mut(w_index) {
            w.updated_at_ms = now;
            w.size = content.len() as u64;
        }
        meta.versions.push(version.clone());
        meta.updated_at_ms = now;
        write_index(&dir, &meta)?;
        self.projects.insert(project.to_owned(), meta);
        Ok(version)
    }

    /// Publishes a caller-chosen version ID. This exists for protocol-level conflict tests and a
    /// future import path; the browser API does not expose it. The publication itself refuses to
    /// replace an existing version file.
    pub fn publish_version_with_id(
        &mut self,
        project: &str,
        version: &str,
        content: &str,
        parent: Option<&str>,
    ) -> Result<VersionMeta, StoreError> {
        if !ids::is_component(version) {
            return Err(StoreError::InvalidInput(format!("invalid version id: {version}")));
        }
        validate_project(content)?;
        let dir = self.project_dir(project)?;
        let mut meta = self.project(project)?;
        if meta.versions.iter().any(|v| v.id == version) {
            return Err(StoreError::VersionExists(version.to_owned()));
        }
        if let Some(parent) = parent
            && !meta.versions.iter().any(|v| v.id == parent)
        {
            return Err(StoreError::InvalidInput(format!("parent version not found: {parent}")));
        }
        let published = publish_version(&dir.join("versions"), version, content, parent.map(str::to_owned))?;
        meta.versions.push(published.clone());
        meta.updated_at_ms = now_ms();
        write_index(&dir, &meta)?;
        self.projects.insert(project.to_owned(), meta);
        Ok(published)
    }

    fn new_project_dir(&self) -> Result<(String, PathBuf), StoreError> {
        for _ in 0..32 {
            let id = ids::new_id("prj");
            let dir = self.home.projects().join(&id);
            match fs::create_dir(&dir) {
                Ok(()) => return Ok((id, dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(StoreError::InvalidInput("could not allocate a unique project id".into()))
    }

    fn project_dir(&self, id: &str) -> Result<PathBuf, StoreError> {
        if !ids::is_component(id) {
            return Err(StoreError::InvalidInput(format!("invalid project id: {id}")));
        }
        let dir = self.home.projects().join(id);
        if dir.is_dir() { Ok(dir) } else { Err(StoreError::ProjectNotFound(id.to_owned())) }
    }
}

fn load_project(dir: &Path, id: &str) -> Result<ProjectMeta, StoreError> {
    for name in ["versions", "working"] {
        fs::create_dir_all(dir.join(name))?;
    }
    clean_temps(&dir.join("versions"))?;
    clean_temps(&dir.join("working"))?;

    let mut meta = match fs::read_to_string(dir.join("project.json")) {
        Ok(text) if (text.len() as u64) <= MAX_INDEX_BYTES => match serde_json::from_str::<ProjectMeta>(&text) {
            Ok(mut meta) if meta.id == id => {
                meta.versions.retain(|v| ids::is_component(&v.id));
                meta.working_copies.retain(|w| ids::is_component(&w.id));
                meta
            }
            _ => fresh_meta(id),
        },
        _ => fresh_meta(id),
    };

    let mut changed = false;
    let version_files = files_with_extension(&dir.join("versions"), "rgis")?;
    let mut seen = std::collections::BTreeSet::new();
    for (version_id, path) in version_files {
        if !seen.insert(version_id.clone()) {
            continue;
        }
        let len = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if let Some(v) = meta.versions.iter_mut().find(|v| v.id == version_id) {
            if v.size != len {
                v.size = len;
                v.sha256 = None;
                changed = true;
            }
        } else {
            meta.versions.push(VersionMeta { id: version_id, parent: None, created_at_ms: file_ms(&path), size: len, sha256: None });
            changed = true;
        }
    }
    let before = meta.versions.len();
    meta.versions.retain(|v| seen.contains(&v.id));
    changed |= before != meta.versions.len();
    meta.versions.sort_by(|a, b| a.created_at_ms.cmp(&b.created_at_ms).then_with(|| a.id.cmp(&b.id)));

    let working_files = files_with_extension(&dir.join("working"), "rgis")?;
    let mut seen = std::collections::BTreeSet::new();
    for (working_id, path) in working_files {
        if !seen.insert(working_id.clone()) {
            continue;
        }
        let len = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if let Some(w) = meta.working_copies.iter_mut().find(|w| w.id == working_id) {
            if w.size != len {
                w.size = len;
                changed = true;
            }
        } else {
            let at = file_ms(&path);
            meta.working_copies.push(WorkingCopyMeta {
                id: working_id,
                base_version: None,
                created_at_ms: at,
                updated_at_ms: at,
                size: len,
            });
            changed = true;
        }
    }
    let before = meta.working_copies.len();
    meta.working_copies.retain(|w| seen.contains(&w.id));
    changed |= before != meta.working_copies.len();

    if changed || !dir.join("project.json").is_file() {
        write_index(dir, &meta)?;
    }
    Ok(meta)
}

fn fresh_meta(id: &str) -> ProjectMeta {
    let now = now_ms();
    ProjectMeta {
        id: id.to_owned(),
        name: id.to_owned(),
        created_at_ms: now,
        updated_at_ms: now,
        versions: Vec::new(),
        working_copies: Vec::new(),
    }
}

fn write_index(dir: &Path, meta: &ProjectMeta) -> Result<(), StoreError> {
    if !ids::is_component(&meta.id) {
        return Err(StoreError::InvalidInput(format!("invalid project id: {}", meta.id)));
    }
    let text = serde_json::to_vec_pretty(meta)?;
    rustgis_data::io::write_atomic(&dir.join("project.json"), &text)?;
    Ok(())
}

fn publish_version(dir: &Path, id: &str, text: &str, parent: Option<String>) -> Result<VersionMeta, StoreError> {
    if !ids::is_component(id) {
        return Err(StoreError::InvalidInput(format!("invalid version id: {id}")));
    }
    fs::create_dir_all(dir)?;
    let final_path = dir.join(format!("{id}.rgis"));
    let temp_path = dir.join(format!(".tmp-{}", ids::new_id("tmp")));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&temp_path)?;
    let write = (|| -> std::io::Result<()> {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    drop(file);
    if let Err(e) = write {
        let _ = fs::remove_file(&temp_path);
        return Err(e.into());
    }
    if let Err(e) = publish_temp(&temp_path, &final_path) {
        let _ = fs::remove_file(&temp_path);
        return match e.kind() {
            std::io::ErrorKind::AlreadyExists => Err(StoreError::VersionExists(id.to_owned())),
            _ => Err(e.into()),
        };
    }
    // On Unix the publication is a hard link, so the temporary name is still one link to the
    // published inode. Removing it is safe after the final name exists; on Windows `rename` has
    // already consumed it and this is a harmless no-op.
    let _ = fs::remove_file(&temp_path);
    sync_dir(dir);
    let size = text.len() as u64;
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let sha256 = hex(&hasher.finalize());
    Ok(VersionMeta { id: id.to_owned(), parent, created_at_ms: now_ms(), size, sha256: Some(sha256) })
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn publish_temp(temp: &Path, final_path: &Path) -> std::io::Result<()> {
    // Windows `rename` refuses an existing destination; Unix `rename` would replace it, so use a
    // hard link, whose creation is atomic and exclusive.
    #[cfg(windows)]
    {
        fs::rename(temp, final_path)
    }
    #[cfg(not(windows))]
    {
        fs::hard_link(temp, final_path)
    }
}

#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(file) = fs::File::open(dir) {
        let _ = file.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

fn write_new_text(path: &Path, text: &str) -> Result<(), StoreError> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    sync_dir(path.parent().unwrap_or_else(|| Path::new(".")));
    Ok(())
}

fn read_project_text(path: &Path) -> Result<String, StoreError> {
    let len = fs::metadata(path)?.len();
    if len > MAX_PROJECT_BYTES as u64 {
        return Err(StoreError::InvalidProject("the project file is too large".into()));
    }
    Ok(fs::read_to_string(path)?)
}

/// Validates project JSON without `Project::normalize`: opening a stored version must not touch
/// source files, external databases or the network. The full engine load happens in the client.
pub fn validate_project(text: &str) -> Result<(), StoreError> {
    if text.len() > MAX_PROJECT_BYTES {
        return Err(StoreError::InvalidProject("the project file is too large".into()));
    }
    let project: Project = serde_json::from_str(text).map_err(|e| StoreError::InvalidProject(e.to_string()))?;
    if project.format > rustgis_engine::project::FORMAT {
        return Err(StoreError::InvalidProject(format!("project format {} is newer than this server", project.format)));
    }
    Ok(())
}

fn clean_name(name: &str) -> Result<String, StoreError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(StoreError::InvalidInput("project name is empty".into()));
    }
    if name.len() > 200 || name.chars().any(char::is_control) {
        return Err(StoreError::InvalidInput("project name is too long or contains control characters".into()));
    }
    Ok(name.to_owned())
}

fn files_with_extension(dir: &Path, ext: &str) -> Result<Vec<(String, PathBuf)>, StoreError> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(ext) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if ids::is_component(stem) {
            out.push((stem.to_owned(), path));
        }
    }
    Ok(out)
}

fn clean_temps(dir: &Path) -> Result<(), StoreError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.file_name().to_string_lossy().starts_with(".tmp-") {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn file_ms(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
