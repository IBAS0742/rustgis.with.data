# RustGIS

**A fast, free and open-source GIS in pure Rust for mapping, analyzing and sharing geospatial
data — running natively, in the browser, and self-hosted.**

This repository pairs the RustGIS engine with a small, self-hosted **preview server**: run it
locally or in Docker, browse your GIS data through controlled source roots, and edit projects with
immutable version history. Your data stays on your machine unless you send it to an AI model you
choose, and every action is a command you can script from the command line, a notebook or the AI
Assistant.

<div class="hero-actions" markdown>

[Run the preview server](preview/index.md){ .md-button .md-button--primary }
[Get started](getting-started.md){ .md-button }
[User Guide](user-guide/index.md){ .md-button }
[Compare](comparison/index.md){ .md-button }

</div>

<figure markdown="span">
  [![The RustGIS web map served by the self-hosted preview server: the World Atlas sample with the layers pane](preview/images/map-app.png)](preview/images/map-app.png){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption>The RustGIS browser map (WebAssembly), served by the self-hosted preview server.</figcaption>
</figure>

## Highlights

<div class="grid cards" markdown>

-   **Self-hosted preview server**

    One binary plus Docker: controlled source roots, bounded file reads, persistent PostGIS
    connections (passwords never on disk), and `.rgis` projects with immutable versions and
    working copies.
    [Preview Server](preview/index.md) · [Deploy](preview/deploy.md) · [HTTP API](preview/api.md)

-   **Your data, any source**

    GeoJSON, Shapefile, GeoPackage, GeoParquet, GeoTIFF and more, live PostGIS and MySQL
    databases, and public data from STAC catalogs, Overture Maps and open-data portals.
    [Data](user-guide/data.md) · [Web Data](user-guide/web-data.md) · [Formats](user-guide/data-formats.md)

-   **Cartography and 3D**

    Graduated colours, collision-free labels, page layouts and map series; a 3D globe, any
    projection, terrain and extrusions, and maps of the Moon, Mars and other worlds.
    [Cartography](user-guide/cartography.md) · [Maps](user-guide/maps.md) · [Layouts](user-guide/layouts.md)

-   **Analysis**

    SQL selection, overlay and proximity tools, guided workflows, ModelBuilder, imagery tools and
    over 1,000 GeoLibre and Whitebox tools for terrain, hydrology and LiDAR.
    [Analysis](user-guide/analysis.md)

-   **Editing**

    Create features and edit their vertices with snapping and templates, keep attributes valid
    with domains, subtypes and rules, check topology, and undo any change.
    [Editing](user-guide/editing.md)

-   **AI Assistant**

    Ask about your data or tell the map what to do. The assistant runs RustGIS commands with the
    model you choose, asks before going online, and every change can be undone.
    [AI Assistant](user-guide/assistant.md)

-   **Scriptable and extensible**

    `rustgis-cli` and the `rustgis` Python package run any command and render maps; the MCP
    server lets AI agents make maps; sandboxed WebAssembly plugins add tools.
    [Command Line](cli.md) · [Python](python.md) · [MCP](mcp.md) · [Plugins](plugins.md)

</div>

## A tour

<div class="grid" markdown>

<figure markdown="span">
  [![The preview console: a configured source root and the floating file browser with its paginated listing and add-layer buttons, over the running map](preview/images/sources-browse.png)](preview/images/sources-browse.png){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Data sources.</strong> Server-side folders browsed in a floating window; adding a layer never modifies source data.</figcaption>
</figure>

<figure markdown="span">
  [![The projects tree: a project with its published versions, a working copy and the layer availability panel](preview/images/project-versions.png)](preview/images/project-versions.png){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Immutable versions.</strong> Saved versions are read-only; editing happens in working copies and saving publishes a new version.</figcaption>
</figure>

<figure markdown="span">
  [![The Style tab and the Symbology pane, with the larger African countries selected](https://assets.rustgis.app/images/symbology-table.webp)](https://assets.rustgis.app/images/symbology-table.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Symbology and tables.</strong> Graduated colours, with the selection synced to the table.</figcaption>
</figure>

<figure markdown="span">
  [![The AI Assistant selecting the five most populous African countries](https://assets.rustgis.app/images/assistant.webp)](https://assets.rustgis.app/images/assistant.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>AI Assistant.</strong> Ask about your data or tell the map what to do.</figcaption>
</figure>

<figure markdown="span">
  [![The Analyze tab's tool gallery and dissolved buffers around African cities](https://assets.rustgis.app/images/buffer-history.webp)](https://assets.rustgis.app/images/buffer-history.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Analysis.</strong> A tool gallery, the Tools pane and 1,000+ GeoLibre tools.</figcaption>
</figure>

<figure markdown="span">
  [![ModelBuilder: a model of buffer, clip and dissolve after a run](https://assets.rustgis.app/images/modelbuilder.webp)](https://assets.rustgis.app/images/modelbuilder.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>ModelBuilder.</strong> Chain tools on a node canvas and run them in one go.</figcaption>
</figure>

<figure markdown="span">
  [![A shaded-relief elevation model with the Style tab](https://assets.rustgis.app/images/raster.webp)](https://assets.rustgis.app/images/raster.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Rasters.</strong> GeoTIFF and COG, with stretch, RGB and shaded-relief styles.</figcaption>
</figure>

<figure markdown="span">
  [![The World Atlas on the 3D globe](https://assets.rustgis.app/images/globe.webp)](https://assets.rustgis.app/images/globe.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Globe and projections.</strong> A 3D globe, Equal Earth, Robinson or any EPSG code.</figcaption>
</figure>

<figure markdown="span">
  [![Mount Rainier in USGS imagery draped over terrain, tilted](https://assets.rustgis.app/images/terrain.webp)](https://assets.rustgis.app/images/terrain.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Terrain.</strong> Tilt the map and drape it over elevation.</figcaption>
</figure>

<figure markdown="span">
  [![An OpenFreeMap vector style over Paris with the Basemap gallery](https://assets.rustgis.app/images/tiles.webp)](https://assets.rustgis.app/images/tiles.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Web basemaps.</strong> XYZ, WMS and vector tiles, and MapLibre styles.</figcaption>
</figure>

<figure markdown="span">
  [![The Sentinel-2 Explorer in the Web Data pane, with scene footprints on the map](https://assets.rustgis.app/images/web-data.webp)](https://assets.rustgis.app/images/web-data.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Web Data.</strong> STAC, Overture Maps, NASA and open-data portals, where the map is.</figcaption>
</figure>

<figure markdown="span">
  [![A page layout with a map frame, legend, north arrow and scale bar](https://assets.rustgis.app/images/layout.webp)](https://assets.rustgis.app/images/layout.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Layouts.</strong> Map frames, legends, scale bars, map series and PDF export.</figcaption>
</figure>

<figure markdown="span">
  [![Dark mode over Europe on the Midnight basemap](https://assets.rustgis.app/images/dark.webp)](https://assets.rustgis.app/images/dark.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Dark mode</strong> and the View tab's effects, panes and linked views.</figcaption>
</figure>

</div>
