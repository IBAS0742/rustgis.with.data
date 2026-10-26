# Sources and Databases

## Source folders

Source roots are added by typing a path that exists on the **server**. The browser never submits an
arbitrary path to the file API: file reads use `source-id` plus a normalized, slash-separated
relative path.

* Browsing is one directory level at a time, sorted and paginated (default 100, maximum 200).
  The browser opens in a floating window: no overlay, draggable by its header, resizable at the
  corner, and one shared instance reused for the last-clicked source or database.
* Scans and browsing skip dot files and symlinks. Canonical paths must remain inside the root.
* A rescan updates status counts but does not add anything to a project. New files appear in the
  browser; the user explicitly clicks **Add layer** (添加图层). Added layers land in the scratch map
  (or the open working copy); nothing is persisted until the user saves — saved versions stay
  read-only, and the console copies a version to a working copy before editing it.
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
database) are persisted in `config/databases.json`.

**Password policy (product decision, 2026-10-25):** a password entered when the connection is
saved is persisted in `databases.json` and restored on start, so the connection works again after
a restart or page refresh without retyping it. Connections saved with an empty password keep the
original behaviour: the secret is held in process memory only, and the console asks again after
every page refresh. Either way, a password is verified against the real server before it is
accepted (a wrong password keeps the input for retry), and a session-entered password overrides
the stored one for that session. Passwords never appear in API views, logs, URLs or project files
(there are tests asserting this). Because `databases.json` stores passwords in plain text,
restrict access to the file on the server host.

The catalog and database list open in the same floating window as the source browser: no overlay,
draggable and resizable, one shared instance. The catalog route opens one connection per selected
database and lists schemas, relations and geometry columns according to the account's
permissions. A missing database, schema, table or column yields a normal error response; it does
not crash the server.

A browser cannot open a PostgreSQL socket. The preview therefore adds a **bounded GeoJSON
snapshot** endpoint and feeds it to the normal remote-file path (`ui.addUrl`). Snapshot layers are
not live: changing the table does not update an already added layer. Re-add the table to refresh
it. The default limit is 5,000 features and is capped at 50,000. Geometry conversion, CRS handling
and source reading reuse the existing `rustgis_data::source::postgis` provider. The connection
string and password are not placed in the snapshot URL.

This is intentionally a product limitation, not a PostgreSQL proxy. If live PostGIS editing or
unbounded feature counts are required, a server-side tile/feature service would be a separate
design decision.
