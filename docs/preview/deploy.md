# Deploy the Preview Server

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
