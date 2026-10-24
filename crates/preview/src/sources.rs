//! Persistent source roots and bounded, path-checked file browsing.
//!
//! A configured root is stored as an absolute, canonical path. Every request resolves a relative
//! path under that root again and canonicalizes the result, so `..` and symlinks/junctions cannot
//! escape it. Symlink entries are not followed while listing; a direct link is accepted only when
//! its canonical target is still inside the root. Listing is paginated and bounded; scanning is
//! explicit (there is no watcher and no automatic import into a project).

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::StoreError;
use crate::home::DataHome;
use crate::ids;

/// The config file format.
const FORMAT: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 8 << 20;
/// The most entries a rescan visits, and the most a browser page's directory scan holds.
const MAX_SCAN_ENTRIES: usize = 200_000;
const MAX_BROWSE_ENTRIES: usize = 20_000;
const MAX_DEPTH: usize = 32;

/// Extensions that are shown as data. Sidecars are not listed alone but may be served when a
/// provider asks for them.
pub const DATA_EXTENSIONS: &[&str] = &[
    "geojson",
    "json",
    "csv",
    "tsv",
    "txt",
    "shp",
    "zip",
    "kml",
    "kmz",
    "gpx",
    "gpkg",
    "parquet",
    "geoparquet",
    "fgb",
    "xlsx",
    "xlsm",
    "xlsb",
    "xls",
    "ods",
    "rss",
    "atom",
    "georss",
    "xml",
    "landxml",
    "gml",
    "dxf",
    "mif",
    "pbf",
    "topojson",
    "las",
    "laz",
    "tif",
    "tiff",
    "asc",
    "png",
    "jpg",
    "jpeg",
    "mbtiles",
    "pmtiles",
    "jp2",
    "j2k",
];
pub const SIDECAR_EXTENSIONS: &[&str] = &["dbf", "shx", "prj", "cpg", "tfw", "wld", "aux", "ovr", "qmd"];

/// One configured directory.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FolderSource {
    pub id: String,
    pub display_name: String,
    pub root_path: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub last_scan_at_ms: Option<u64>,
    /// `unknown`, `ready`, `unavailable` or `error`.
    #[serde(default = "unknown")]
    pub scan_status: String,
    #[serde(default)]
    pub last_error: Option<String>,
}

fn yes() -> bool {
    true
}
fn unknown() -> String {
    "unknown".to_owned()
}

/// The result of an explicit rescan. A missing or unreadable root is reported in `error` while the
/// source record stays in the registry, so it can recover when the data returns.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanSummary {
    pub source: FolderSource,
    pub files: usize,
    pub directories: usize,
    pub truncated: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    /// Slash-separated, relative to the configured root. This is what file APIs accept.
    pub relative_path: String,
    /// `file` or `directory`.
    pub kind: String,
    pub size: u64,
    pub modified_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilePage {
    pub source_id: String,
    pub path: String,
    pub page: u32,
    pub page_size: u32,
    pub total: usize,
    pub truncated: bool,
    pub entries: Vec<FileEntry>,
}

/// A file that passed the source-root check, ready for a provider or a Range response.
#[derive(Clone, Debug)]
pub struct ResolvedFile {
    pub source_id: String,
    pub name: String,
    pub relative_path: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified_ms: u64,
}

/// The result of checking a project's file reference without opening or reading the file. The
/// project format stores absolute paths from the machine that saved it, so a reference is only
/// considered available when it resolves inside an explicitly configured source root.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceState {
    Available,
    Missing,
    Outside,
    Disabled,
    Remote,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ReferenceCheck {
    pub state: ReferenceState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    pub reason: String,
}

impl ReferenceCheck {
    pub fn is_available(&self) -> bool {
        self.state == ReferenceState::Available
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceFile {
    #[serde(default = "format")]
    format: u32,
    #[serde(default)]
    folders: Vec<FolderSource>,
}

fn format() -> u32 {
    FORMAT
}

/// The source-root registry, persisted as `<home>/config/sources.json`.
pub struct SourceRegistry {
    home: DataHome,
    folders: Vec<FolderSource>,
}

impl SourceRegistry {
    pub fn open(home: DataHome) -> Result<Self, StoreError> {
        let path = home.config().join("sources.json");
        let folders = match fs::metadata(&path) {
            Ok(meta) if meta.len() > MAX_CONFIG_BYTES => {
                return Err(StoreError::InvalidInput("sources.json is too large".into()));
            }
            Ok(_) => {
                let text = fs::read_to_string(&path)?;
                let file: SourceFile = serde_json::from_str(&text).map_err(|e| StoreError::InvalidInput(format!("sources.json: {e}")))?;
                if file.format > FORMAT {
                    return Err(StoreError::InvalidInput(format!("sources.json format {} is newer than this server", file.format)));
                }
                file.folders.into_iter().filter(|f| ids::is_component(&f.id)).collect()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(SourceRegistry { home, folders })
    }

    pub fn folders(&self) -> &[FolderSource] {
        &self.folders
    }

    pub fn folder(&self, id: &str) -> Result<FolderSource, StoreError> {
        if !ids::is_component(id) {
            return Err(StoreError::InvalidInput(format!("invalid source id: {id}")));
        }
        self.folders.iter().find(|f| f.id == id).cloned().ok_or_else(|| StoreError::SourceNotFound(id.to_owned()))
    }

    /// Adds an existing directory. The path is canonicalized and must be a directory; no arbitrary
    /// browser path is stored.
    pub fn add_folder(&mut self, path: &str, display_name: Option<&str>) -> Result<FolderSource, StoreError> {
        let root = fs::canonicalize(path).map_err(|e| StoreError::InvalidInput(format!("{path}: {e}")))?;
        if !root.is_dir() {
            return Err(StoreError::NotADirectory(path.to_owned()));
        }
        let root_text = path_string(&root)?;
        if self.folders.iter().any(|f| same_path(&f.root_path, &root_text)) {
            return Err(StoreError::InvalidInput("that directory is already configured".into()));
        }
        let name = display_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| root.file_name().and_then(|n| n.to_str()).map(str::to_owned))
            .unwrap_or_else(|| root_text.clone());
        if name.len() > 200 || name.chars().any(char::is_control) {
            return Err(StoreError::InvalidInput("source name is too long or contains control characters".into()));
        }
        let now = now_ms();
        let source = FolderSource {
            id: ids::new_id("src"),
            display_name: name,
            root_path: root_text,
            enabled: true,
            created_at_ms: now,
            last_scan_at_ms: None,
            scan_status: "unknown".into(),
            last_error: None,
        };
        self.folders.push(source.clone());
        self.save()?;
        Ok(source)
    }

    pub fn remove_folder(&mut self, id: &str) -> Result<FolderSource, StoreError> {
        let index = self.folders.iter().position(|f| f.id == id).ok_or_else(|| StoreError::SourceNotFound(id.to_owned()))?;
        let removed = self.folders.remove(index);
        self.save()?;
        Ok(removed)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<FolderSource, StoreError> {
        let source = self.folders.iter_mut().find(|f| f.id == id).ok_or_else(|| StoreError::SourceNotFound(id.to_owned()))?;
        source.enabled = enabled;
        if !enabled {
            source.scan_status = "disabled".into();
        }
        let out = source.clone();
        self.save()?;
        Ok(out)
    }

    /// Revalidates the root without following symlinks and updates the persisted scan status.
    /// Files are discovered live by [`Self::browse`]; a rescan never adds them to a project.
    pub fn rescan(&mut self, id: &str) -> Result<ScanSummary, StoreError> {
        let index = self.folders.iter().position(|f| f.id == id).ok_or_else(|| StoreError::SourceNotFound(id.to_owned()))?;
        let source = self.folders.get(index).cloned().ok_or_else(|| StoreError::SourceNotFound(id.to_owned()))?;
        let mut summary = scan_source(&source);
        if let Some(folder) = self.folders.get_mut(index) {
            folder.last_scan_at_ms = Some(now_ms());
            if let Some(error) = &summary.error {
                let lower = error.to_ascii_lowercase();
                folder.scan_status = if lower.contains("not found")
                    || lower.contains("no such file")
                    || lower.contains("cannot find the file")
                    || lower.contains("cannot find the path")
                {
                    "unavailable".into()
                } else {
                    "error".into()
                };
                folder.last_error = Some(error.clone());
            } else if folder.enabled {
                folder.scan_status = "ready".into();
                folder.last_error = None;
            } else {
                folder.scan_status = "disabled".into();
            }
            summary.source = folder.clone();
        }
        self.save()?;
        Ok(summary)
    }

    /// Lists one directory level, sorted and paginated. The root itself is `""`.
    pub fn browse(&self, id: &str, relative: &str, page: u32, page_size: u32) -> Result<FilePage, StoreError> {
        let source = self.folder(id)?;
        if !source.enabled {
            return Err(StoreError::SourceUnavailable("the source is disabled".into()));
        }
        let root = canonical_root(&source)?;
        let rel = normalize_relative(relative)?;
        let dir = resolve_inside(&root, &rel, true)?;
        let page_size = page_size.clamp(1, 200);
        let offset = u64::from(page).saturating_mul(u64::from(page_size));

        let mut entries = Vec::new();
        let mut truncated = false;
        let read = fs::read_dir(&dir).map_err(|e| StoreError::SourceUnavailable(format!("{}: {e}", source.root_path)))?;
        for (visited, entry) in read.enumerate() {
            if visited >= MAX_BROWSE_ENTRIES {
                truncated = true;
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => return Err(StoreError::SourceUnavailable(format!("{}: {e}", source.root_path))),
            };
            if entries.len() >= MAX_BROWSE_ENTRIES {
                truncated = true;
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let rel_path = join_relative(&rel, &name);
            if meta.is_dir() {
                entries.push(FileEntry {
                    name,
                    relative_path: rel_path,
                    kind: "directory".into(),
                    size: 0,
                    modified_ms: modified_ms(&meta),
                    format: None,
                });
            } else if meta.is_file() && is_data_file(&name) {
                entries.push(FileEntry {
                    name: name.clone(),
                    relative_path: rel_path,
                    kind: "file".into(),
                    size: meta.len(),
                    modified_ms: modified_ms(&meta),
                    format: format_for(&name),
                });
            }
        }
        entries.sort_by(|a, b| {
            let ad = a.kind == "directory";
            let bd = b.kind == "directory";
            bd.cmp(&ad).then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase())).then_with(|| a.name.cmp(&b.name))
        });
        let total = entries.len();
        if offset > total as u64 {
            entries.clear();
        }
        let entries = entries.into_iter().skip(usize::try_from(offset).unwrap_or(usize::MAX)).take(page_size as usize).collect();
        Ok(FilePage { source_id: id.to_owned(), path: rel, page, page_size, total, truncated, entries })
    }

    /// Resolves a file for a provider or Range endpoint. Rejects traversal, symlink escapes and
    /// directories; data files and their supported sidecars are readable.
    pub fn resolve_file(&self, id: &str, relative: &str) -> Result<ResolvedFile, StoreError> {
        let source = self.folder(id)?;
        if !source.enabled {
            return Err(StoreError::SourceUnavailable("the source is disabled".into()));
        }
        let root = canonical_root(&source)?;
        let rel = normalize_relative(relative)?;
        if rel.is_empty() {
            return Err(StoreError::FileNotFound("a file path is required".into()));
        }
        let path = resolve_inside(&root, &rel, false)?;
        let meta = fs::metadata(&path)?;
        if !meta.is_file() {
            return Err(StoreError::FileNotFound(relative.to_owned()));
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or(&rel).to_owned();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !is_servable_extension(&ext) {
            return Err(StoreError::PathNotAllowed(format!("{name}: only supported data files can be read")));
        }
        Ok(ResolvedFile { source_id: id.to_owned(), name, relative_path: rel, size: meta.len(), modified_ms: modified_ms(&meta), path })
    }

    /// Checks a COG tile URL that points back at this server's controlled file API. Returns `None`
    /// for other URLs (a remote service, a data URI, a path template), which stay `unknown`
    /// because checking them would make opening a project depend on the network. This is a
    /// metadata check; it never reads the raster.
    pub fn check_file_url(&self, url: &str) -> Option<ReferenceCheck> {
        let url = url.split(['?', '#']).next().unwrap_or(url);
        let rest = url.split_once("/api/files/")?.1;
        let (id, encoded) = rest.split_once('/')?;
        let source_id = id.to_owned();
        let Some(relative) = percent_decode(encoded) else {
            return Some(ReferenceCheck {
                state: ReferenceState::Outside,
                source_id: None,
                reason: "the file URL is not valid URL text".into(),
            });
        };
        match self.resolve_file(id, &relative) {
            Ok(_) => Some(ReferenceCheck {
                state: ReferenceState::Available,
                source_id: Some(source_id),
                reason: "the file exists inside a configured source".into(),
            }),
            Err(e) => {
                let (state, reason) = match &e {
                    StoreError::FileNotFound(_) => (ReferenceState::Missing, "the file is missing".to_owned()),
                    StoreError::SourceUnavailable(_) => (ReferenceState::Disabled, "the source folder is disabled".to_owned()),
                    StoreError::SourceNotFound(_) => (ReferenceState::Outside, "the source folder no longer exists".to_owned()),
                    StoreError::PathNotAllowed(_) | StoreError::InvalidInput(_) => (ReferenceState::Outside, e.to_string()),
                    _ => (ReferenceState::Missing, e.to_string()),
                };
                Some(ReferenceCheck { state, source_id: Some(source_id), reason })
            }
        }
    }

    /// Checks a file reference stored in a project without opening it. Only configured, enabled
    /// roots may supply a local file; an absolute path outside every root is reported as
    /// `outside` rather than probed. Remote URLs are `remote` (unknown) because checking them would
    /// make opening a project depend on the network. This is a metadata check, never a read.
    pub fn check_reference(&self, reference: &str) -> ReferenceCheck {
        let reference = reference.trim();
        if reference.is_empty() {
            return ReferenceCheck { state: ReferenceState::Missing, source_id: None, reason: "the layer has no file path".into() };
        }
        if reference.contains('\0') {
            return ReferenceCheck { state: ReferenceState::Outside, source_id: None, reason: "the path contains NUL".into() };
        }
        if looks_remote(reference) {
            return ReferenceCheck {
                state: ReferenceState::Remote,
                source_id: None,
                reason: "remote references are not checked by the server".into(),
            };
        }
        let path = Path::new(reference);
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return ReferenceCheck {
                state: ReferenceState::Outside,
                source_id: None,
                reason: "parent traversal is not allowed in a project reference".into(),
            };
        }
        let absolute = path.is_absolute();
        let mut disabled: Option<(String, String)> = None;
        let mut missing: Option<(String, String)> = None;
        let mut permission: Option<String> = None;
        for folder in &self.folders {
            let Ok(root) = fs::canonicalize(&folder.root_path) else {
                continue;
            };
            if !root.is_dir() {
                continue;
            }
            if absolute && !is_within(path, &root) {
                continue;
            }
            let candidate = if absolute { path.to_path_buf() } else { root.join(path) };
            match fs::canonicalize(&candidate) {
                Ok(found) => {
                    if !is_within(&found, &root) {
                        return ReferenceCheck {
                            state: ReferenceState::Outside,
                            source_id: Some(folder.id.clone()),
                            reason: "the path resolves outside its configured source root".into(),
                        };
                    }
                    if !folder.enabled {
                        disabled = Some((folder.id.clone(), "the source folder is disabled".into()));
                        continue;
                    }
                    if found.is_file() {
                        return ReferenceCheck {
                            state: ReferenceState::Available,
                            source_id: Some(folder.id.clone()),
                            reason: "the file exists inside a configured source".into(),
                        };
                    }
                    missing = Some((folder.id.clone(), "the reference is not a regular file".into()));
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    permission = Some("permission denied while checking the reference".to_string());
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if !folder.enabled {
                        disabled = Some((folder.id.clone(), "the source folder is disabled".into()));
                    } else {
                        missing = Some((folder.id.clone(), "the file is missing".into()));
                    }
                }
                Err(_) => {
                    missing = Some((folder.id.clone(), "the reference could not be checked".into()));
                }
            }
        }
        if let Some((source_id, reason)) = disabled {
            return ReferenceCheck { state: ReferenceState::Disabled, source_id: Some(source_id), reason };
        }
        if let Some(source_id) = missing.as_ref().map(|(id, _)| id.clone()) {
            let reason = missing.map(|(_, reason)| reason).unwrap_or_else(|| "the file is missing".into());
            return ReferenceCheck { state: ReferenceState::Missing, source_id: Some(source_id), reason };
        }
        if let Some(reason) = permission {
            return ReferenceCheck { state: ReferenceState::Missing, source_id: None, reason };
        }
        let reason = if self.folders.is_empty() {
            "no source folders are configured".to_owned()
        } else if absolute {
            "the path is outside every configured source root".to_owned()
        } else {
            "the relative reference was not found in any configured source root".to_owned()
        };
        ReferenceCheck { state: ReferenceState::Outside, source_id: None, reason }
    }

    fn save(&self) -> Result<(), StoreError> {
        let file = SourceFile { format: FORMAT, folders: self.folders.clone() };
        let bytes = serde_json::to_vec_pretty(&file)?;
        rustgis_data::io::write_atomic(&self.home.config().join("sources.json"), &bytes)?;
        Ok(())
    }
}

fn scan_source(source: &FolderSource) -> ScanSummary {
    let mut files = 0usize;
    let mut directories = 0usize;
    let mut truncated = false;
    let mut error = None;
    let root = match fs::canonicalize(&source.root_path) {
        Ok(root) if root.is_dir() => root,
        Ok(_) => {
            return ScanSummary {
                source: source.clone(),
                files,
                directories,
                truncated,
                error: Some(format!("{}: not a directory", source.root_path)),
            };
        }
        Err(e) => {
            return ScanSummary {
                source: source.clone(),
                files,
                directories,
                truncated,
                error: Some(format!("{}: {e}", source.root_path)),
            };
        }
    };
    let mut stack = vec![(root, 0usize)];
    let mut visited = 0usize;
    let mut seen = HashSet::new();
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            truncated = true;
            continue;
        }
        let read = match fs::read_dir(&dir) {
            Ok(read) => read,
            Err(e) => {
                error = Some(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        for entry in read {
            if visited >= MAX_SCAN_ENTRIES {
                truncated = true;
                break;
            }
            let Ok(entry) = entry else {
                continue;
            };
            visited += 1;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                directories += 1;
                if seen.insert(path.clone()) {
                    stack.push((path, depth + 1));
                }
            } else if meta.is_file() && entry.file_name().to_string_lossy().rsplit_once('.').is_some_and(|(_, ext)| is_data_extension(ext))
            {
                files += 1;
            }
        }
        if truncated {
            break;
        }
    }
    ScanSummary { source: source.clone(), files, directories, truncated, error }
}

fn canonical_root(source: &FolderSource) -> Result<PathBuf, StoreError> {
    let root = PathBuf::from(&source.root_path);
    match fs::canonicalize(&root) {
        Ok(root) if root.is_dir() => Ok(root),
        Ok(_) => Err(StoreError::SourceUnavailable(format!("{}: not a directory", source.root_path))),
        Err(e) => Err(StoreError::SourceUnavailable(format!("{}: {e}", source.root_path))),
    }
}

/// Normalizes a slash-separated relative path. Absolute paths, drive prefixes, colons and `..` are
/// refused; `.` and repeated separators are harmless.
fn normalize_relative(relative: &str) -> Result<String, StoreError> {
    if relative.starts_with('/') || relative.contains('\\') || relative.contains('\0') {
        return Err(StoreError::PathNotAllowed("absolute paths, backslashes and NUL are not allowed".into()));
    }
    let mut parts = Vec::new();
    for part in relative.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(StoreError::PathNotAllowed("parent traversal is not allowed".into()));
        }
        if part.contains(':') {
            return Err(StoreError::PathNotAllowed("drive and alternate-stream prefixes are not allowed".into()));
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn resolve_inside(root: &Path, relative: &str, want_dir: bool) -> Result<PathBuf, StoreError> {
    let path = if relative.is_empty() { root.to_path_buf() } else { root.join(relative) };
    let path = fs::canonicalize(&path).map_err(|e| {
        if want_dir { StoreError::SourceUnavailable(format!("{relative}: {e}")) } else { StoreError::FileNotFound(relative.to_owned()) }
    })?;
    if !is_within(&path, root) {
        return Err(StoreError::PathNotAllowed(relative.to_owned()));
    }
    if want_dir && !path.is_dir() {
        return Err(StoreError::NotADirectory(relative.to_owned()));
    }
    Ok(path)
}

#[cfg(windows)]
fn is_within(path: &Path, root: &Path) -> bool {
    if path.starts_with(root) {
        return true;
    }
    let p = path.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    let r = root.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    p == r || p.strip_prefix(&r).is_some_and(|rest| rest.starts_with('\\'))
}

#[cfg(not(windows))]
fn is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

fn join_relative(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_owned() } else { format!("{parent}/{name}") }
}

fn path_string(path: &Path) -> Result<String, StoreError> {
    path.to_str().map(str::to_owned).ok_or_else(|| StoreError::InvalidInput("the configured path is not valid UTF-8".into()))
}

#[cfg(windows)]
fn same_path(a: &str, b: &str) -> bool {
    a.replace('/', "\\").eq_ignore_ascii_case(&b.replace('/', "\\"))
}

#[cfg(not(windows))]
fn same_path(a: &str, b: &str) -> bool {
    a == b
}

fn is_data_extension(ext: &str) -> bool {
    let ext = ext.to_ascii_lowercase();
    DATA_EXTENSIONS.contains(&ext.as_str())
}

fn is_servable_extension(ext: &str) -> bool {
    let ext = ext.to_ascii_lowercase();
    DATA_EXTENSIONS.contains(&ext.as_str()) || SIDECAR_EXTENSIONS.contains(&ext.as_str())
}

fn is_data_file(name: &str) -> bool {
    Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(is_data_extension)
}

fn looks_remote(reference: &str) -> bool {
    reference.contains("://")
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = *bytes.get(i + 1)?;
            let lo = *bytes.get(i + 2)?;
            out.push(hex(hi)? << 4 | hex(lo)?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn format_for(name: &str) -> Option<String> {
    Path::new(name).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_uppercase())
}

fn modified_ms(meta: &fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Validates a path component from a URL before it reaches this module; useful to the HTTP layer.
pub fn is_safe_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.contains(['/', '\\', ':', '\0'])
        && Path::new(component).components().all(|c| matches!(c, Component::Normal(_)))
}
