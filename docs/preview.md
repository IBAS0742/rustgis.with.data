# RustGIS Preview (self-hosted)

This checkout contains a small, self-hosted preview application built around the existing RustGIS
web map, renderer, data providers and `.rgis` project format. It is deliberately unaudited for
public internet exposure: there are no accounts, roles or collaborative editing.

The server adds four things the browser app cannot do by itself:

1. persistent source-folder configuration and controlled file reads;
2. persistent PostGIS connection configuration (passwords in memory only);
3. immutable project versions and mutable working copies;
4. an availability check for external layer references when a project opens.

The browser map is the existing `apps/rustgis-web` WebAssembly application. The server console is a
plain HTML/JavaScript page served at `/`; it embeds the map at `/map.html?embed` and uses the
existing `postMessage` embed protocol for `ui.*` commands and complete project JSON. No second
rendering engine, tile server or database proxy is introduced.

## Build and run natively

Prerequisites: Rust 1.90 or newer (the checked-in environment uses 1.99.1) and a C compiler. The
PostGIS provider is compiled in; a running PostGIS server is only needed for database features.

```sh
cargo build --release -p rustgis-preview-server
cargo run --release -p rustgis-preview-server
```

The default data home is the platform data directory, and the default bind is
`127.0.0.1:8765`. Open <http://127.0.0.1:8765/>.

| Environment variable | Default | Meaning |
|---|---|---|
| `RUSTGIS_PREVIEW_HOME` | platform data directory | Config, projects, cache and logs; must be writable |
| `RUSTGIS_PREVIEW_BIND` | `127.0.0.1:8765` | HTTP listen address |
| `RUSTGIS_PREVIEW_WEB_DIR` | `apps/rustgis-web/dist`, then `dist/web` | Directory containing the browser map's `index.html`, JS and WASM |

If no web build is found, the console and all APIs still start and the map area explains how to
build it. Build the map with Trunk:

```sh
cd apps/rustgis-web
trunk build --release
# Trunk writes ../../dist/web; the server detects that path when run from the repository root.
```

For Docker, which expects `apps/rustgis-web/dist`, use the included helper:

```sh
cd apps/rustgis-web
./build-preview.sh
```

On Windows without a POSIX shell, run the same steps in PowerShell after Trunk finishes:

```powershell
cd apps/rustgis-web
trunk build --release
Remove-Item -Recurse -Force dist -ErrorAction SilentlyContinue
Copy-Item -Recurse ..\..\dist\web dist
```

### Windows (PowerShell)

```powershell
$env:RUSTGIS_PREVIEW_HOME = "$env:LOCALAPPDATA\RustGIS Preview"
cargo run --release -p rustgis-preview-server
```

### Linux and WSL

```sh
export RUSTGIS_PREVIEW_HOME="$HOME/.local/share/rustgis-preview"
cargo run --release -p rustgis-preview-server
```

Windows drive paths and Linux mount paths are not interchangeable. A project saved against
`C:\GIS\data` is not automatically available as `/mnt/c/GIS/data` in WSL. Add the path format used
by the process serving the request. The source registry stores the canonical path returned by the
operating system; use the console to add roots rather than editing configuration by hand.

## Docker

The image runs the server and serves a previously built browser map. Build the map first, then:

```sh
# Optional: a host directory containing GIS data, mounted read-only at /srv/gis.
export RUSTGIS_SOURCE_DIR=/absolute/path/to/gis-data
docker compose up --build -d
```

PowerShell equivalent:

```powershell
$env:RUSTGIS_SOURCE_DIR = "C:\path\to\gis-data"
docker compose up --build -d
```

The Dockerfile uses BuildKit cache mounts for the Cargo registry and target directory. BuildKit is
the default in current Docker Engine and Docker Desktop; if an older daemon uses the legacy builder,
enable BuildKit or remove the `RUN --mount=...` options. The image is built from `rust:1.99-bookworm`
and runs as an unprivileged user.

Open <http://127.0.0.1:8765/>. In the Sources tab add `/srv/gis` (the container path), not the host
path. The named volume `rustgis-preview-data` holds `config/`, `projects/`, `cache/` and `logs/`, so
projects and version history survive `docker compose down` and image rebuilds. Removing the volume
deletes the preview state; it does not delete source data.

The compose file binds the published port to `127.0.0.1` by default. To use a trusted LAN, change
the port mapping and put the service behind a reverse proxy with TLS and access control. The
process is not an authentication boundary.

## Project versions and `.rgis`

A project directory contains metadata, immutable versions and working copies:

```text
<home>/projects/<project-id>/
  project.json
  versions/<version-id>.rgis
  working/<working-id>.rgis
```

* Creating a project publishes V1.
* A saved version is read-only through the API. `PUT`, `PATCH`, `DELETE` and `POST` to a version
  route return `409 version_immutable`.
* Editing starts by copying a version to a working copy. Saving the working copy publishes a new
  unique version; the base version is never modified.
* Publication writes a temporary file, syncs it, then uses a no-overwrite operation (`hard_link`
  on Unix, `rename` on Windows) and updates metadata atomically. The temporary name is removed
  after publication, and on restart the index is reconciled with complete files.
* The original `.rgis` semantics are retained. Small vector datasets may be embedded in the
  project; windowed/streamed sources keep a path or URL reference and do not copy all source data.
  A TIFF added through the map's URL import is probed first: a cloud-optimised GeoTIFF (COG) becomes
  a streamed `maps[].tiles` entry with its URL, while a non-COG TIFF is downloaded whole and stored
  as embedded raster bytes in `maps[].layers`. The source browser therefore refuses TIFFs larger
  than 64 MiB; use a known COG from the map's **Add Data From URL** for larger rasters. This is the
  existing RustGIS format, not a new project format.
* Availability checking covers `maps[].layers` and COG entries in `maps[].tiles` whose URL points
  at this server's `/api/files/...` route. A missing COG file is reported as unavailable and the
  derived copy hides that tile layer; saving a working copy restores the stored visibility, and
  the layer recovers when the file returns. Remote COG URLs stay `unknown` because the server does
  not make a network request while opening a project.

Opening a version calls `GET /api/projects/{id}/versions/{version}/prepared`. The server parses the
`.rgis` JSON without `Project::normalize`, checks external file references and returns:

* the original project content with only unavailable layers (including file-API COG tile layers)
  changed to `visible: false`;
* an `availability` report with layer name, map, status, source root if any, and reason. A COG
  entry also carries `tile_index` (its position in `maps[].tiles`) and uses `layer_id: 0`.

The stored `.rgis` is not modified. If the file returns, opening the version again restores the
original visibility because the stored version still carries it. **Recheck** re-reads the stored
version or working copy when there are no unsaved edits; when the map is dirty it posts the current
JSON to `/api/projects/{id}/availability` and re-shows only layers that the previous check hid and
that are now available. The derived copy adds an ignored `x_rustgis_availability` marker; RustGIS
ignores unknown fields and drops the marker the next time it saves the project. If a working copy is
saved while a checked layer or COG tile is unavailable, the console restores its original visibility
in the stored JSON and removes the derived marker; opening the new version still hides it until the
data returns.

The check does not read file contents, open a database, or fetch a URL. Local references must
resolve inside a configured, enabled source root; `..`, absolute paths outside every root, and
symlink/junction escapes are marked unavailable. Remote URLs are reported as `unknown` and are not
hidden, because checking them would make opening a project depend on the network. Live PostGIS,
MySQL, SQL Server, DuckDB and GDAL references without embedded features are marked unavailable in
this browser preview; a PostGIS layer with stored features remains available.

## Sources and files

Source roots are added by typing a path that exists on the **server**. The browser never submits an
arbitrary path to the file API: file reads use `source-id` plus a normalized, slash-separated
relative path.

* Browsing is one directory level at a time, sorted and paginated (default 100, maximum 200).
* Scans and browsing skip dot files and symlinks. Canonical paths must remain inside the root.
* A rescan updates status counts but does not add anything to a project. New files appear in the
  browser; the user explicitly clicks **Add layer**. Adding a layer requires an open working copy:
  saved versions are read-only, so use **Edit as working copy** first.
* Supported extensions are listed in `crates/preview/src/sources.rs` (`DATA_EXTENSIONS`); a
  sidecar file can be served only when requested through its source path and is not listed as a
  standalone data file.
* `GET /api/files/{source_id}/{relative_path}` supports `HEAD` and a single `Range` request with
  `206`, `Content-Range`, `Accept-Ranges` and `Content-Length`. Bodies are streamed with
  `File::take`; files are never buffered in full for a Range request.
* Re-adding a file to the map does not modify source data. Cache directories are rebuildable and
  are not the only copy of anything.

## PostgreSQL/PostGIS

Connection settings (host, port, username, SSL mode, display name and an optional default
database) are persisted in `config/databases.json`. The password is held only in process memory.
After a restart, the connection remains listed but reports `password_required: true`; enter the
password again in the console. Passwords are never written to project JSON, URLs, logs or API
responses. Error text is passed through a credential-stripping function before it reaches a client.

The catalog route opens one connection per selected database and lists schemas, relations and
geometry columns according to the account's permissions. A missing database, schema, table or
column yields a normal error response; it does not crash the server.

A browser cannot open a PostgreSQL socket. The preview therefore adds a **bounded GeoJSON
snapshot** endpoint and feeds it to the normal remote-file path (`ui.addUrl`). Snapshot layers are
not live: changing the table does not update an already added layer. Re-add the table to refresh
it. The default limit is 5,000 features and is capped at 50,000. Geometry conversion, CRS handling
and source reading reuse the existing `rustgis_data::source::postgis` provider. The connection
string and password are not placed in the snapshot URL.

This is intentionally a product limitation, not a PostgreSQL proxy. If live PostGIS editing or
unbounded feature counts are required, a server-side tile/feature service would be a separate
design decision.

## HTTP API

All response errors use `{"error":{"code":...,"message":...}}`. The server hides OS errors and
credential-bearing database errors from responses.

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/api/health` | Status and whether a web build is available |
| `GET` | `/api/sources` or `/api/sources/folders` | List source roots |
| `POST` | `/api/sources/folders` | Add `{path, displayName?}` |
| `GET` | `/api/sources/folders/{id}` | One source |
| `DELETE` | `/api/sources/folders/{id}` | Remove configuration; never deletes files |
| `PATCH` | `/api/sources/folders/{id}` | `{enabled: bool}` |
| `POST` | `/api/sources/folders/{id}/scan` | Rescan and update status |
| `GET` | `/api/sources/folders/{id}/items?path=&page=&page_size=` | Paginated directory listing |
| `GET`/`HEAD` | `/api/files/{source_id}/{relative_path}` | Controlled file read and Range |
| `GET`/`POST` | `/api/sources/databases` | List/add non-secret connection settings |
| `GET`/`DELETE` | `/api/sources/databases/{id}` | Read/remove one connection |
| `POST` | `/api/sources/databases/{id}/password` | `{password}`; memory only |
| `GET` | `/api/sources/databases/{id}/databases` | Accessible databases |
| `GET` | `/api/sources/databases/{id}/catalog?database=` | Schemas, relations, geometry columns |
| `GET` | `/api/sources/databases/{id}/geojson?database=&table=&limit=` | Bounded GeoJSON snapshot |
| `GET`/`POST` | `/api/projects` | List/create projects |
| `GET` | `/api/projects/{id}` | Metadata with versions/working copies |
| `GET` | `/api/projects/{id}/versions` | Version list |
| `GET` | `/api/projects/{id}/versions/{version}` | Read-only saved content |
| `GET` | `/api/projects/{id}/versions/{version}/prepared` | Derived project plus availability |
| `PUT`/`PATCH`/`DELETE`/`POST` | `/api/projects/{id}/versions/{version}` | Always `409 version_immutable` |
| `POST` | `/api/projects/{id}/copies` | `{versionId}` → working copy |
| `POST` | `/api/projects/{id}/versions` | `{workingCopyId, content}` → new version (alternate save route) |
| `GET` | `/api/projects/{id}/working/{working}` | Read working copy |
| `GET` | `/api/projects/{id}/working/{working}/prepared` | Derived working copy plus availability |
| `POST` | `/api/projects/{id}/working/{working}/versions` | `{content}` → new version |
| `POST` | `/api/projects/{id}/availability` | `{content}` → derived content/report without storing |

Request body size is bounded by `MAX_PROJECT_BYTES` plus 1 MiB. The server has at most 32
in-flight requests; this keeps a scan from blocking the map indefinitely.

## Security and operational limits

* No accounts or authorization. Default loopback binding is intentional. Use a firewall and a
  reverse proxy if access from another machine is required.
* Source data is treated as read-only. The server writes only inside `RUSTGIS_PREVIEW_HOME`.
* Paths from project JSON are not trusted: they are checked only against configured roots and are
  never used to open arbitrary files during availability checks.
* The file API accepts only supported data extensions and sidecars; directories and dot files are
  not served.
* Database secrets are process-memory only. The config file records `password_required` as a
  non-secret boolean so the UI can ask again after restart.
* Logs currently go to stderr and do not include request bodies or connection passwords.
* No claim is made about 10 GB TIFF performance, concurrent viewer capacity, or non-COG remote
  reads. The source browser's 64 MiB TIFF guard is a safety limit, not a COG detector or a
  performance guarantee. Large-raster and concurrency checks require the acceptance tests in
  `ACCEPTANCE_TESTS.md` with real data and were not run here.

## Tests

```sh
cargo test -p rustgis-preview
cargo fmt --all --check
cargo clippy -p rustgis-preview -p rustgis-preview-server --all-targets -- -D warnings
cargo xtask ci
```

The preview tests cover immutable versions, restart reconciliation, source-root traversal and
symlink escapes, Range reads, password persistence boundaries, GeoJSON compilation paths and
missing-layer recovery. Live PostGIS catalog/snapshot tests and a 10 GB reference file require
external infrastructure and are not part of the default test suite.
