# HTTP API

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
| `GET`/`POST` | `/api/sources/databases` | List connections / add `{host, port, username, sslMode, displayName?, database?, password?}`; a save-time password is persisted and verified before acceptance |
| `GET`/`DELETE` | `/api/sources/databases/{id}` | Read/remove one connection |
| `POST` | `/api/sources/databases/{id}/password` | `{password}`; verified against the server, then session-memory only (overrides a stored one) |
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
* Database passwords entered at save time are persisted in `config/databases.json` (plain text;
  restrict access to that file). Passwords entered later live in process memory only. The secret
  is never returned by any API view, log or URL, and a `password_required` flag tells the UI when
  to ask again.
* Logs currently go to stderr and do not include request bodies or connection passwords.
* No claim is made about 10 GB TIFF performance, concurrent viewer capacity, or non-COG remote
  reads. The source browser's 64 MiB TIFF guard is a safety limit, not a COG detector or a
  performance guarantee. Large-raster and concurrency checks require the acceptance tests in
  `agent-pack/docs/ACCEPTANCE_TESTS.md` with real data and were not run here.

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
