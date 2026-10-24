# Build the preview server. The browser assets are built separately (or by CI) into
# apps/rustgis-web/dist; the runtime image only needs that directory, not the Rust toolchain.
#
#   cd apps/rustgis-web && ./build-preview.sh   (or the PowerShell equivalent in docs/preview.md)
#   docker compose up --build
#
# The RustGIS web build is intentionally not compiled in this image: Trunk and the WebAssembly
# toolchain make the build image much larger, and the same dist can be served by a native binary.

FROM rust:1.99-bookworm AS server
WORKDIR /src
COPY . .
# Keep the cargo registry and target directory between builds. A source-only change then rebuilds
# the changed crates instead of downloading and compiling the whole workspace again.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --locked --release -p rustgis-preview-server \
    && cp target/release/rustgis-preview-server /usr/local/bin/rustgis-preview-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/preview preview
WORKDIR /app
COPY --from=server /usr/local/bin/rustgis-preview-server /usr/local/bin/rustgis-preview-server
COPY apps/rustgis-web/dist /app/web
RUN mkdir -p /data && chown -R preview:preview /data /app
USER preview
ENV RUSTGIS_PREVIEW_HOME=/data \
    RUSTGIS_PREVIEW_WEB_DIR=/app/web \
    RUSTGIS_PREVIEW_BIND=0.0.0.0:8765
EXPOSE 8765
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8765/api/health || exit 1
ENTRYPOINT ["/usr/local/bin/rustgis-preview-server"]
