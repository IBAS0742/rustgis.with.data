#!/usr/bin/env sh
# Build the RustGIS browser map for the self-hosted preview server.
#
# Trunk writes ../../dist/web (Trunk.toml). The preview server and Dockerfile expect a dist
# inside apps/rustgis-web, so copy it there after a successful build. No source files are
# changed and the generated directory is safe to delete and rebuild.
set -eu
cd "$(dirname "$0")"
trunk build --release
rm -rf dist
cp -R ../../dist/web dist
printf '%s\n' "Preview web assets: $(pwd)/dist"
