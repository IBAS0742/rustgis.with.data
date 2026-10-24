//! The minimal HTTP server for RustGIS Preview.
//!
//! The default bind is loopback only. This is deliberate: the service has no accounts and is meant
//! for one trusted user or a trusted LAN, not the public internet. It speaks HTTP/1.1 through
//! `tiny_http`; deployment docs put a reverse proxy in front when TLS or wider access is needed.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, ResponseBox, Server, StatusCode};

use crate::StoreError;
use crate::databases::{DatabaseRegistry, NewDatabase};
use crate::project_store::MAX_PROJECT_BYTES;
use crate::sources::SourceRegistry;
use crate::{DataHome, ProjectStore};

type BytesResponse = Response<std::io::Cursor<Vec<u8>>>;

/// How many requests may be handled at once. A long directory scan cannot consume all threads and
/// make the map UI unresponsive.
const MAX_IN_FLIGHT: usize = 32;
/// JSON and project uploads are bounded; the static file handler has its own size policy.
const MAX_API_BODY: usize = MAX_PROJECT_BYTES + (1 << 20);

struct App {
    projects: Mutex<ProjectStore>,
    sources: Mutex<SourceRegistry>,
    databases: Mutex<DatabaseRegistry>,
    web_dir: Option<PathBuf>,
}

struct Permit {
    active: Arc<(Mutex<usize>, Condvar)>,
}

impl Permit {
    fn acquire(active: Arc<(Mutex<usize>, Condvar)>) -> Self {
        let (lock, cv) = &*active;
        let mut n = lock.lock().unwrap_or_else(|e| e.into_inner());
        while *n >= MAX_IN_FLIGHT {
            n = cv.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n += 1;
        drop(n);
        Permit { active }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let (lock, cv) = &*self.active;
        let mut n = lock.lock().unwrap_or_else(|e| e.into_inner());
        *n = n.saturating_sub(1);
        cv.notify_one();
    }
}

/// Runs the server from the environment:
/// - `RUSTGIS_PREVIEW_HOME` (data home; platform default otherwise)
/// - `RUSTGIS_PREVIEW_BIND` (default `127.0.0.1:8765`)
/// - `RUSTGIS_PREVIEW_WEB_DIR` (default `apps/rustgis-web/dist` when present)
pub fn run() -> Result<(), String> {
    let home = DataHome::from_env().map_err(|e| format!("data home: {e}"))?;
    let bind = std::env::var("RUSTGIS_PREVIEW_BIND").unwrap_or_else(|_| "127.0.0.1:8765".into());
    let web = std::env::var_os("RUSTGIS_PREVIEW_WEB_DIR")
        .map(PathBuf::from)
        .or_else(|| ["apps/rustgis-web/dist", "dist/web"].into_iter().map(PathBuf::from).find(|dir| dir.is_dir()));
    run_at(home, &bind, web)
}

/// Runs with explicit settings. Kept separate so a test or a launcher can use a temporary home.
pub fn run_at(home: DataHome, bind: &str, web_dir: Option<PathBuf>) -> Result<(), String> {
    let store = ProjectStore::open(home.clone()).map_err(|e| format!("open projects: {e}"))?;
    let sources = SourceRegistry::open(home.clone()).map_err(|e| format!("open source registry: {e}"))?;
    let databases = DatabaseRegistry::open(home).map_err(|e| format!("open database registry: {e}"))?;
    let web_dir = web_dir.and_then(|dir| fs::canonicalize(dir).ok()).filter(|dir| dir.is_dir());
    let app = Arc::new(App { projects: Mutex::new(store), sources: Mutex::new(sources), databases: Mutex::new(databases), web_dir });
    let server = Server::http(bind).map_err(|e| format!("bind {bind}: {e}"))?;
    println!("RustGIS Preview listening on http://{bind}");
    if bind.starts_with("0.0.0.0") || bind.starts_with("[::]") {
        println!("warning: bound to all interfaces; use a firewall or reverse proxy on untrusted networks");
    }
    if app.web_dir.is_none() {
        println!("note: no web build found; serving the built-in API page from apps/rustgis-web/dist");
    }
    let active = Arc::new((Mutex::new(0usize), Condvar::new()));
    loop {
        let request = match server.recv() {
            Ok(request) => request,
            Err(e) => {
                eprintln!("accept error: {e}");
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let permit = Permit::acquire(active.clone());
        let app = app.clone();
        std::thread::spawn(move || {
            let _permit = permit;
            if let Err(e) = handle(app, request) {
                eprintln!("request error: {e}");
            }
        });
    }
}

fn handle(app: Arc<App>, mut request: Request) -> Result<(), String> {
    let method = request.method().clone();
    let url = request.url().to_owned();
    let path = url.split('?').next().unwrap_or("/").to_owned();
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    let is_api = path == "/api" || path.starts_with("/api/");
    let is_file_api = path.starts_with("/api/files/");

    if is_file_api {
        if method != Method::Get && method != Method::Head {
            let response = json_response(405, json!({"error": {"code": "method_not_allowed", "message": "source files are read-only"}}));
            request.respond(response).map_err(|e| e.to_string())?;
            return Ok(());
        }
        let range = request.headers().iter().find(|h| h.field.equiv("Range")).map(|h| h.value.as_str().to_owned());
        let reply = file_response(app.as_ref(), &path, range.as_deref(), method == Method::Head);
        let status = reply.status;
        request.respond(reply.response).map_err(|e| e.to_string())?;
        println!("{method:?} {path} -> {status}");
        return Ok(());
    }

    let response = if is_api {
        let body = match method {
            Method::Post | Method::Put | Method::Patch => match read_body(&mut request, MAX_API_BODY) {
                Ok(body) => body,
                Err(e) => return request.respond(error_response(e)).map_err(|e| e.to_string()),
            },
            _ => Vec::new(),
        };
        api_response(app.as_ref(), &method, &path, query, &body)
    } else {
        static_response(app.as_ref(), &method, &path)
    };

    let status = response.status_code().0;
    request.respond(response).map_err(|e| e.to_string())?;
    if is_api {
        println!("{method:?} {path} -> {status}");
    }
    Ok(())
}

fn read_body(request: &mut Request, max: usize) -> Result<Vec<u8>, HttpResponseError> {
    if request.body_length().is_some_and(|n| n > max) {
        return Err(HttpResponseError::TooLarge);
    }
    let mut body = Vec::new();
    request.as_reader().take(max as u64 + 1).read_to_end(&mut body).map_err(|_| HttpResponseError::BadBody)?;
    if body.len() > max {
        return Err(HttpResponseError::TooLarge);
    }
    Ok(body)
}

enum HttpResponseError {
    TooLarge,
    BadBody,
}

fn error_response(error: HttpResponseError) -> BytesResponse {
    match error {
        HttpResponseError::TooLarge => json_response(413, json!({"error": {"code": "too_large", "message": "request body is too large"}})),
        HttpResponseError::BadBody => {
            json_response(400, json!({"error": {"code": "bad_body", "message": "could not read the request body"}}))
        }
    }
}

fn api_response(app: &App, method: &Method, path: &str, query: &str, body: &[u8]) -> BytesResponse {
    let Some(rest) = path.strip_prefix("/api/") else {
        return json_response(404, json!({"error": {"code": "not_found", "message": "no such API endpoint"}}));
    };
    let segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    match (method, segments.as_slice()) {
        (Method::Get, ["health"]) => json_response(
            200,
            json!({
                "status": "ok",
                "service": "rustgis-preview",
                "web_ready": app.web_dir.is_some(),
                "postgis": true,
            }),
        ),
        (Method::Get, ["sources"] | ["sources", "folders"]) => {
            let sources = app.sources.lock().unwrap_or_else(|e| e.into_inner());
            json_response(200, json!({"folders": sources.folders()}))
        }
        (Method::Post, ["sources", "folders"]) => {
            let request: AddFolder = match parse_json(body) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let mut sources = app.sources.lock().unwrap_or_else(|e| e.into_inner());
            match sources.add_folder(&request.path, request.display_name.as_deref()) {
                Ok(source) => json_response(201, source),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["sources", "folders", id]) => {
            let sources = app.sources.lock().unwrap_or_else(|e| e.into_inner());
            match sources.folder(id) {
                Ok(source) => json_response(200, source),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Delete, ["sources", "folders", id]) => {
            let mut sources = app.sources.lock().unwrap_or_else(|e| e.into_inner());
            match sources.remove_folder(id) {
                Ok(_) => empty_response(204),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Patch, ["sources", "folders", id]) | (Method::Post, ["sources", "folders", id, "enabled"]) => {
            let request: SetEnabled = match parse_json(body) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let mut sources = app.sources.lock().unwrap_or_else(|e| e.into_inner());
            match sources.set_enabled(id, request.enabled) {
                Ok(source) => json_response(200, source),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Post, ["sources", "folders", id, "scan"]) => {
            let mut sources = app.sources.lock().unwrap_or_else(|e| e.into_inner());
            match sources.rescan(id) {
                Ok(summary) => json_response(200, summary),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["sources", "folders", id, "items"]) => {
            let relative = query_value(query, "path").unwrap_or_default();
            let page = query_u32(query, "page").unwrap_or(0);
            let page_size = query_u32(query, "page_size").unwrap_or(100);
            let sources = app.sources.lock().unwrap_or_else(|e| e.into_inner());
            match sources.browse(id, &relative, page, page_size) {
                Ok(page) => json_response(200, page),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["sources", "databases"]) => {
            let databases = app.databases.lock().unwrap_or_else(|e| e.into_inner());
            json_response(200, json!({"connections": databases.views()}))
        }
        (Method::Post, ["sources", "databases"]) => {
            let request: AddDatabase = match parse_json(body) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let mut databases = app.databases.lock().unwrap_or_else(|e| e.into_inner());
            match databases.add(NewDatabase {
                display_name: request.display_name,
                host: request.host,
                port: request.port,
                username: request.username,
                password: request.password,
                ssl_mode: request.ssl_mode,
                database: request.database,
            }) {
                Ok(view) => json_response(201, view),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["sources", "databases", id]) => {
            let databases = app.databases.lock().unwrap_or_else(|e| e.into_inner());
            match databases.view_for(id) {
                Ok(view) => json_response(200, view),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Delete, ["sources", "databases", id]) => {
            let mut databases = app.databases.lock().unwrap_or_else(|e| e.into_inner());
            match databases.remove(id) {
                Ok(()) => empty_response(204),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Post, ["sources", "databases", id, "password"]) => {
            let request: SetPassword = match parse_json(body) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let mut databases = app.databases.lock().unwrap_or_else(|e| e.into_inner());
            match databases.set_password(id, &request.password) {
                Ok(view) => json_response(200, view),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["sources", "databases", id, "databases"]) => {
            let databases = app.databases.lock().unwrap_or_else(|e| e.into_inner());
            match databases.databases(id) {
                Ok(names) => json_response(200, json!({"databases": names})),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["sources", "databases", id, "catalog"]) => {
            let Some(database) = query_value(query, "database") else {
                return json_response(
                    400,
                    json!({"error": {"code": "invalid_input", "message": "the database query parameter is required"}}),
                );
            };
            let databases = app.databases.lock().unwrap_or_else(|e| e.into_inner());
            match databases.catalog(id, &database) {
                Ok(catalog) => json_response(200, catalog),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["sources", "databases", id, "geojson"]) => {
            let (Some(database), Some(table)) = (query_value(query, "database"), query_value(query, "table")) else {
                return json_response(
                    400,
                    json!({"error": {"code": "invalid_input", "message": "database and table query parameters are required"}}),
                );
            };
            let limit = query_u32(query, "limit").unwrap_or(5000) as usize;
            let databases = app.databases.lock().unwrap_or_else(|e| e.into_inner());
            match databases.geojson(id, &database, &table, limit) {
                Ok(bytes) => bytes_response(200, "application/geo+json", bytes),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Post, ["projects", project, "availability"]) => {
            let request: PrepareContent = match parse_json(body) {
                Ok(value) => value,
                Err(response) => return response,
            };
            {
                let store = app.projects.lock().unwrap_or_else(|e| e.into_inner());
                if let Err(e) = store.project(project) {
                    return store_error_response(e);
                }
            }
            let sources = app.sources.lock().unwrap_or_else(|e| e.into_inner());
            match crate::availability::prepare(&request.content, &sources) {
                Ok(prepared) => json_response(
                    200,
                    json!({
                        "content": prepared.content,
                        "availability": prepared.availability,
                        "prepared": true,
                    }),
                ),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["projects"]) => {
            let store = app.projects.lock().unwrap_or_else(|e| e.into_inner());
            json_response(200, json!({"projects": store.projects()}))
        }
        (Method::Post, ["projects"]) => {
            let request: CreateProject = match parse_json(body) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let mut store = app.projects.lock().unwrap_or_else(|e| e.into_inner());
            match store.create_project(&request.name, request.content) {
                Ok(created) => json_response(201, created),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["projects", project]) => {
            let store = app.projects.lock().unwrap_or_else(|e| e.into_inner());
            match store.project(project) {
                Ok(meta) => json_response(200, meta),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["projects", project, "versions"]) => {
            let store = app.projects.lock().unwrap_or_else(|e| e.into_inner());
            match store.versions(project) {
                Ok(versions) => json_response(200, json!({"versions": versions})),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["projects", project, "versions", version, "prepared"]) => {
            let store = app.projects.lock().unwrap_or_else(|e| e.into_inner());
            let content = match store.read_version(project, version) {
                Ok(content) => content,
                Err(e) => return store_error_response(e),
            };
            drop(store);
            let sources = app.sources.lock().unwrap_or_else(|e| e.into_inner());
            match crate::availability::prepare(&content.content, &sources) {
                Ok(prepared) => json_response(
                    200,
                    json!({
                        "project": content.project,
                        "version": content.version,
                        "content": prepared.content,
                        "read_only": true,
                        "prepared": true,
                        "availability": prepared.availability,
                    }),
                ),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["projects", project, "versions", version]) => {
            let store = app.projects.lock().unwrap_or_else(|e| e.into_inner());
            match store.read_version(project, version) {
                Ok(content) => json_response(200, content),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Post, ["projects", project, "versions"]) => {
            let request: SaveVersion = match parse_json(body) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let mut store = app.projects.lock().unwrap_or_else(|e| e.into_inner());
            match store.save_working_copy(project, &request.working_copy_id, &request.content) {
                Ok(version) => json_response(201, version),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Post, ["projects", project, "copies"]) => {
            let request: CopyVersion = match parse_json(body) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let mut store = app.projects.lock().unwrap_or_else(|e| e.into_inner());
            match store.create_working_copy(project, &request.version_id) {
                Ok(working) => json_response(201, working),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["projects", project, "working", working, "prepared"]) => {
            let store = app.projects.lock().unwrap_or_else(|e| e.into_inner());
            let content = match store.read_working_copy(project, working) {
                Ok(content) => content,
                Err(e) => return store_error_response(e),
            };
            drop(store);
            let sources = app.sources.lock().unwrap_or_else(|e| e.into_inner());
            match crate::availability::prepare(&content.content, &sources) {
                Ok(prepared) => json_response(
                    200,
                    json!({
                        "project": content.project,
                        "id": content.id,
                        "base_version": content.base_version,
                        "content": prepared.content,
                        "read_only": false,
                        "prepared": true,
                        "availability": prepared.availability,
                    }),
                ),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Get, ["projects", project, "working", working]) => {
            let store = app.projects.lock().unwrap_or_else(|e| e.into_inner());
            match store.read_working_copy(project, working) {
                Ok(working) => json_response(200, working),
                Err(e) => store_error_response(e),
            }
        }
        (Method::Post, ["projects", project, "working", working, "versions"]) => {
            let request: SaveContent = match parse_json(body) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let mut store = app.projects.lock().unwrap_or_else(|e| e.into_inner());
            match store.save_working_copy(project, working, &request.content) {
                Ok(version) => json_response(201, version),
                Err(e) => store_error_response(e),
            }
        }
        // A published version is immutable. The route exists only to return an explicit conflict;
        // it never calls a write method.
        (Method::Put | Method::Patch | Method::Delete | Method::Post, ["projects", project, "versions", version]) => json_response(
            409,
            json!({"error": {
                "code": "version_immutable",
                "message": format!("version {version} in project {project} is immutable; copy it to a working copy and publish a new version")
            }}),
        ),
        (Method::Options, _) => empty_response(204),
        _ => json_response(404, json!({"error": {"code": "not_found", "message": "no such API endpoint"}})),
    }
}

fn store_error_response(error: StoreError) -> BytesResponse {
    let status = error.status();
    let message = match &error {
        // Do not echo OS paths or database details in an HTTP response.
        StoreError::Io(_) => "a storage operation failed".to_owned(),
        _ => error.to_string(),
    };
    json_response(status, json!({"error": {"code": error.code(), "message": message}}))
}

struct FileReply {
    status: u16,
    response: ResponseBox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    end: u64,
}

/// Serves a source file with an optional single HTTP byte range. The file stays on disk and only
/// the requested slice is handed to the response reader; a 10 GB file is never put in one buffer.
fn file_response(app: &App, path: &str, range_header: Option<&str>, head: bool) -> FileReply {
    let Some(suffix) = path.strip_prefix("/api/files/") else {
        return error_file_reply(404, "not_found", "no such file endpoint");
    };
    let Some(decoded) = percent_decode(suffix) else {
        return error_file_reply(400, "invalid_path", "the file path is not valid URL text");
    };
    let Some((source_id, relative)) = decoded.split_once('/') else {
        return error_file_reply(400, "invalid_path", "the file path must be source_id/relative/path");
    };
    let resolved = {
        let sources = app.sources.lock().unwrap_or_else(|e| e.into_inner());
        match sources.resolve_file(source_id, relative) {
            Ok(file) => file,
            Err(e) => {
                let status = e.status();
                let message = e.to_string();
                return error_file_reply(status, e.code(), &message);
            }
        }
    };

    let range = match range_header {
        Some(value) => match parse_range(value, resolved.size) {
            Ok(Some(range)) => Some(range),
            Ok(None) => None,
            Err(()) => return range_not_satisfiable(resolved.size),
        },
        None => None,
    };
    let (status, start, end) = match range {
        Some(range) => (206, range.start, range.end),
        None if resolved.size == 0 => (200, 0, 0),
        None => (200, 0, resolved.size - 1),
    };
    let length = if resolved.size == 0 { 0 } else { end - start + 1 };

    let mut headers = Vec::new();
    if let Ok(h) = Header::from_bytes("Content-Type", file_content_type(&resolved.name)) {
        headers.push(h);
    }
    if let Ok(h) = Header::from_bytes("Accept-Ranges", "bytes") {
        headers.push(h);
    }
    if let Ok(h) = Header::from_bytes("Cache-Control", "no-store") {
        headers.push(h);
    }
    if status == 206
        && let Ok(h) = Header::from_bytes("Content-Range", format!("bytes {start}-{end}/{}", resolved.size))
    {
        headers.push(h);
    }

    let file = match fs::File::open(&resolved.path) {
        Ok(file) => file,
        Err(_) => return error_file_reply(404, "file_not_found", "the file could not be opened"),
    };
    if head {
        let mut response = Response::new(StatusCode(status), headers, std::io::empty(), Some(0), None);
        if let Ok(h) = Header::from_bytes("Content-Length", length.to_string()) {
            response.add_header(h);
        }
        return FileReply { status, response: response.boxed() };
    }
    let mut file = file;
    if file.seek(SeekFrom::Start(start)).is_err() {
        return error_file_reply(500, "io_error", "the file could not be read");
    }
    let reader = file.take(length);
    let response = Response::new(StatusCode(status), headers, reader, usize::try_from(length).ok(), None);
    FileReply { status, response: response.boxed() }
}

fn error_file_reply(status: u16, code: &str, message: &str) -> FileReply {
    let body = json!({"error": {"code": code, "message": message}});
    let response = json_response(status, body);
    FileReply { status, response: response.boxed() }
}

fn range_not_satisfiable(total: u64) -> FileReply {
    let mut response =
        json_response(416, json!({"error": {"code": "range_not_satisfiable", "message": "the requested byte range is outside the file"}}));
    if let Ok(h) = Header::from_bytes("Content-Range", format!("bytes */{total}")) {
        response.add_header(h);
    }
    if let Ok(h) = Header::from_bytes("Accept-Ranges", "bytes") {
        response.add_header(h);
    }
    FileReply { status: 416, response: response.boxed() }
}

/// Parses one `Range: bytes=…` range. Multiple ranges are refused rather than silently served as
/// the first one; callers get a 416 and can retry without Range.
fn parse_range(value: &str, total: u64) -> Result<Option<ByteRange>, ()> {
    let Some(spec) = value.strip_prefix("bytes=") else {
        return Err(());
    };
    if spec.contains(',') {
        return Err(());
    }
    let (start, end) = spec.split_once('-').ok_or(())?;
    if total == 0 {
        return Err(());
    }
    if start.is_empty() {
        let suffix: u64 = end.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let start = total.saturating_sub(suffix);
        return Ok(Some(ByteRange { start, end: total - 1 }));
    }
    let start: u64 = start.parse().map_err(|_| ())?;
    if start >= total {
        return Err(());
    }
    let end = if end.is_empty() { total - 1 } else { end.parse::<u64>().map_err(|_| ())?.min(total - 1) };
    if end < start {
        return Err(());
    }
    Ok(Some(ByteRange { start, end }))
}

fn file_content_type(name: &str) -> &'static str {
    match Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("geojson") => "application/geo+json",
        Some("json") => "application/json",
        Some("csv") | Some("tsv") | Some("txt") => "text/csv; charset=utf-8",
        Some("tif") | Some("tiff") => "image/tiff",
        Some("parquet") | Some("geoparquet") => "application/vnd.apache.parquet",
        Some("fgb") => "application/octet-stream",
        Some("gpkg") => "application/geopackage+sqlite3",
        Some("zip") => "application/zip",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("kml") => "application/vnd.google-earth.kml+xml",
        Some("kmz") => "application/vnd.google-earth.kmz",
        Some("gpx") => "application/gpx+xml",
        _ => "application/octet-stream",
    }
}

#[derive(Deserialize)]
struct CreateProject {
    name: String,
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct AddFolder {
    path: String,
    #[serde(rename = "displayName", default)]
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct SetEnabled {
    enabled: bool,
}

#[derive(Deserialize)]
struct AddDatabase {
    host: String,
    port: u16,
    username: String,
    #[serde(rename = "displayName", default)]
    display_name: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(rename = "sslMode", default)]
    ssl_mode: Option<String>,
    #[serde(default)]
    database: Option<String>,
}

#[derive(Deserialize)]
struct SetPassword {
    password: String,
}

#[derive(Deserialize)]
struct CopyVersion {
    #[serde(rename = "versionId")]
    version_id: String,
}

#[derive(Deserialize)]
struct SaveVersion {
    #[serde(rename = "workingCopyId")]
    working_copy_id: String,
    content: String,
}

#[derive(Deserialize)]
struct SaveContent {
    content: String,
}

#[derive(Deserialize)]
struct PrepareContent {
    content: String,
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, BytesResponse> {
    serde_json::from_slice(body)
        .map_err(|e| json_response(400, json!({"error": {"code": "invalid_json", "message": format!("invalid JSON request: {e}")}})))
}

fn query_value(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(name).as_deref() == Some(key) {
            return percent_decode(value);
        }
    }
    None
}

fn query_u32(query: &str, key: &str) -> Option<u32> {
    query_value(query, key)?.parse().ok()
}

fn json_response<T: serde::Serialize>(status: u16, value: T) -> BytesResponse {
    let body = match serde_json::to_vec(&value) {
        Ok(body) => body,
        Err(e) => {
            eprintln!("could not encode response: {e}");
            br#"{"error":{"code":"internal","message":"could not encode the response"}}"#.to_vec()
        }
    };
    bytes_response(status, "application/json; charset=utf-8", body)
}

fn empty_response(status: u16) -> BytesResponse {
    bytes_response(status, "text/plain; charset=utf-8", Vec::new())
}

fn bytes_response(status: u16, content_type: &str, body: Vec<u8>) -> BytesResponse {
    let mut response = Response::from_data(body).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes("Content-Type", content_type) {
        response = response.with_header(header);
    }
    if let Ok(header) = Header::from_bytes("X-Content-Type-Options", "nosniff") {
        response = response.with_header(header);
    }
    if let Ok(header) = Header::from_bytes("Cache-Control", "no-store") {
        response = response.with_header(header);
    }
    response
}

fn static_response(app: &App, method: &Method, path: &str) -> BytesResponse {
    if *method != Method::Get && *method != Method::Head {
        return json_response(405, json!({"error": {"code": "method_not_allowed", "message": "static files are read-only"}}));
    }
    let console = matches!(path, "/" | "/console" | "/console.html");
    if console {
        return asset_response(method, "text/html; charset=utf-8", CONSOLE_HTML.as_bytes().to_vec());
    }
    if path == "/console.js" {
        return asset_response(method, "text/javascript; charset=utf-8", CONSOLE_JS.as_bytes().to_vec());
    }
    let path = if matches!(path, "/map" | "/map.html") { "/index.html" } else { path };
    let Some(root) = &app.web_dir else {
        if matches!(path, "/index.html" | "/map.html") {
            return asset_response(method, "text/html; charset=utf-8", builtin_page().as_bytes().to_vec());
        }
        return bytes_response(404, "text/plain; charset=utf-8", b"not found".to_vec());
    };
    let Some(relative) = safe_relative(path) else {
        return bytes_response(404, "text/plain; charset=utf-8", b"not found".to_vec());
    };
    let candidate = root.join(if relative.as_os_str().is_empty() { Path::new("index.html") } else { &relative });
    let Ok(path) = fs::canonicalize(&candidate) else {
        return bytes_response(404, "text/plain; charset=utf-8", b"not found".to_vec());
    };
    if !path.starts_with(root) || !path.is_file() {
        return bytes_response(404, "text/plain; charset=utf-8", b"not found".to_vec());
    }
    if *method == Method::Head {
        let mut response = Response::from_data(Vec::new()).with_status_code(StatusCode(200));
        if let Ok(header) = Header::from_bytes("Content-Type", content_type(&path)) {
            response = response.with_header(header);
        }
        return response;
    }
    match fs::read(&path) {
        Ok(body) => bytes_response(200, content_type(&path), body),
        Err(_) => bytes_response(500, "text/plain; charset=utf-8", b"could not read the web asset".to_vec()),
    }
}

fn asset_response(method: &Method, content_type: &str, body: Vec<u8>) -> BytesResponse {
    if *method == Method::Head {
        let mut response = Response::from_data(Vec::new()).with_status_code(StatusCode(200));
        if let Ok(header) = Header::from_bytes("Content-Type", content_type) {
            response = response.with_header(header);
        }
        return response;
    }
    bytes_response(200, content_type, body)
}

const CONSOLE_HTML: &str = include_str!("../web/console.html");
const CONSOLE_JS: &str = include_str!("../web/console.js");

fn safe_relative(path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(path.trim_start_matches('/'))?;
    if decoded.contains('\\') || decoded.contains('\0') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in decoded.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains(':') {
            return None;
        }
        out.push(part);
    }
    Some(out)
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

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") | Some("map") => "application/json; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn builtin_page() -> &'static str {
    r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>RustGIS Preview</title>
<style>body{font:16px system-ui,sans-serif;max-width:54rem;margin:3rem auto;padding:0 1rem;color:#17202a}h1{font-weight:600}code{background:#eef2f5;padding:.15rem .35rem;border-radius:.25rem}li{margin:.4rem 0}</style>
</head><body>
<h1>RustGIS Preview API is running</h1>
<p>The web application build was not found. This server is serving its API, so the backend can be tested now; build the browser app with <code>trunk build --release</code> in <code>apps/rustgis-web</code> and set <code>RUSTGIS_PREVIEW_WEB_DIR</code> to its <code>dist</code> directory.</p>
<ul>
<li><code>GET /api/health</code></li>
<li><code>GET /api/projects</code>, <code>POST /api/projects</code></li>
<li><code>GET /api/projects/{id}/versions</code></li>
<li><code>POST /api/projects/{id}/copies</code></li>
<li><code>POST /api/projects/{id}/working/{workingId}/versions</code></li>
<li><code>GET /api/projects/{id}/versions/{versionId}</code> (read-only)</li>
</ul>
<p>Do not expose this service directly to the public internet.</p>
</body></html>"#
}

/// A small HTTP client-free smoke test helper: returns the response for one request against an
/// already-open app, without binding a socket. Used by unit tests in this crate.
#[cfg(test)]
fn test_app(store: ProjectStore) -> Arc<App> {
    let home = store.home().clone();
    let sources = SourceRegistry::open(home.clone()).expect("source registry");
    let databases = DatabaseRegistry::open(home).expect("database registry");
    Arc::new(App { projects: Mutex::new(store), sources: Mutex::new(sources), databases: Mutex::new(databases), web_dir: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> (PathBuf, ProjectStore) {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let root = std::env::temp_dir().join(format!("rustgis-preview-http-{name}-{}-{nanos}", std::process::id()));
        let home = DataHome::open(&root).expect("home");
        let store = ProjectStore::open(home).expect("store");
        (root, store)
    }

    #[test]
    fn api_creates_and_reads_a_project_without_http() {
        let (root, store) = temp_store("api");
        let app = test_app(store);
        let created = api_response(app.as_ref(), &Method::Post, "/api/projects", "", br#"{"name":"HTTP","content":null}"#);
        assert_eq!(created.status_code().0, 201);
        let project_id = {
            let body: serde_json::Value = serde_json::from_reader(created.into_reader()).expect("json");
            body["project"]["id"].as_str().expect("project id").to_owned()
        };
        let list = api_response(app.as_ref(), &Method::Get, "/api/projects", "", &[]);
        assert_eq!(list.status_code().0, 200);
        let body: serde_json::Value = serde_json::from_reader(list.into_reader()).expect("json");
        assert_eq!(body["projects"].as_array().map(Vec::len), Some(1));
        let read = api_response(app.as_ref(), &Method::Get, &format!("/api/projects/{project_id}/versions"), "", &[]);
        assert_eq!(read.status_code().0, 200);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn console_is_served_even_when_the_map_build_is_missing() {
        let (root, store) = temp_store("console");
        let app = test_app(store);
        let home = static_response(app.as_ref(), &Method::Get, "/");
        assert_eq!(home.status_code().0, 200);
        let text = String::from_utf8(home.into_reader().bytes().filter_map(Result::ok).collect()).expect("utf-8");
        assert!(text.contains("RustGIS Preview"));
        let js = static_response(app.as_ref(), &Method::Get, "/console.js");
        let js = String::from_utf8(js.into_reader().bytes().filter_map(Result::ok).collect()).expect("utf-8");
        assert!(js.contains("RustGIS Preview console"));
        let map = static_response(app.as_ref(), &Method::Get, "/map.html");
        let map = String::from_utf8(map.into_reader().bytes().filter_map(Result::ok).collect()).expect("utf-8");
        assert!(map.contains("web application build was not found"));
        let missing = static_response(app.as_ref(), &Method::Get, "/missing.js");
        assert_eq!(missing.status_code().0, 404);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn web_paths_cannot_escape_the_dist_directory() {
        assert!(safe_relative("/../secret").is_none());
        assert!(safe_relative("/a%2F..%2Fsecret").is_none());
        assert!(safe_relative("/C:/secret").is_none());
        assert_eq!(safe_relative("/assets/app.wasm").and_then(|p| p.to_str().map(str::to_owned)).as_deref(), Some("assets/app.wasm"));
    }

    #[test]
    fn immutable_route_is_a_conflict() {
        let (root, store) = temp_store("immutable");
        let app = test_app(store);
        let response = api_response(app.as_ref(), &Method::Put, "/api/projects/p/versions/v", "", b"{}");
        assert_eq!(response.status_code().0, 409);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn byte_ranges_are_parsed_without_reading_the_file() {
        assert_eq!(parse_range("bytes=0-99", 1000), Ok(Some(ByteRange { start: 0, end: 99 })));
        assert_eq!(parse_range("bytes=900-", 1000), Ok(Some(ByteRange { start: 900, end: 999 })));
        assert_eq!(parse_range("bytes=-100", 1000), Ok(Some(ByteRange { start: 900, end: 999 })));
        assert_eq!(parse_range("bytes=0-2000", 1000), Ok(Some(ByteRange { start: 0, end: 999 })));
        assert_eq!(parse_range("bytes=-5000", 1000), Ok(Some(ByteRange { start: 0, end: 999 })));
        assert_eq!(parse_range("bytes=1000-", 1000), Err(()));
        assert_eq!(parse_range("bytes=2-1", 1000), Err(()));
        assert_eq!(parse_range("bytes=0-1,3-4", 1000), Err(()));
        assert_eq!(parse_range("items=0-1", 1000), Err(()));
        assert_eq!(parse_range("bytes=0-", 0), Err(()));
    }

    #[test]
    fn range_file_requests_do_not_use_a_whole_file_buffer() {
        let (root, store) = temp_store("range");
        let app = test_app(store);
        let data = root.join("data");
        std::fs::create_dir_all(&data).expect("data");
        let file = data.join("large.tif");
        std::fs::write(&file, vec![7u8; 4096]).expect("fixture");
        let source_id = {
            let mut sources = app.sources.lock().unwrap_or_else(|e| e.into_inner());
            sources.add_folder(data.to_str().expect("path"), None).expect("add");
            sources.folders()[0].id.clone()
        };
        let path = format!("/api/files/{source_id}/large.tif");
        let reply = file_response(app.as_ref(), &path, Some("bytes=100-199"), false);
        assert_eq!(reply.status, 206);
        let head = file_response(app.as_ref(), &path, None, true);
        assert_eq!(head.status, 200);
        std::fs::remove_dir_all(root).ok();
    }
}
