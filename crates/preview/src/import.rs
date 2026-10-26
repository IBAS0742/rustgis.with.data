//! QGIS project import (Phase A): parse a `.qgs` (XML) or `.qgz` (zip) file into a layer plan,
//! auto-match file layers against the configured source roots, and build a RustGIS project from
//! it. Unpaired layers keep their name, order and style and are marked unavailable until the
//! operator pairs them (Phase B); the pairing state lives in the project's import metadata.
//!
//! Security posture: the file is untrusted XML. `roxmltree` does not resolve external entities,
//! input sizes are capped, and PostGIS passwords found in a datasource string are dropped here
//! and never carried into metadata, logs or the project.

use rustgis_engine::map::source::{PostgisOrigin, SourceLayer};
use rustgis_engine::map::{Marker, RasterLayer, Renderer, Rgba, Symbol};
use rustgis_engine::Project;
use rustgis_data::raster::{ColorHint, Crs, GeoTransform, Raster};

use crate::error::StoreError;
use crate::sources::SourceRegistry;

const MAX_QGS_BYTES: usize = 64 << 20;
const MAX_QGZ_ENTRIES: usize = 5_000;

/// Where a QGIS layer takes its data from, after cleaning the provider URI.
#[derive(Clone, Debug, PartialEq)]
pub enum ImportSource {
    /// A vector file (provider `ogr`): the path as written in the project (may be relative).
    File { path: String },
    /// A PostGIS table. The password in the URI is deliberately dropped.
    Postgis {
        host: String,
        port: Option<u16>,
        dbname: String,
        schema: Option<String>,
        table: String,
        geom: Option<String>,
        srid: Option<i32>,
        sql: Option<String>,
    },
    /// A raster file (provider `gdal`, or a raster-typed layer).
    Raster { path: String },
    /// A web service layer (WMS/WFS/XYZ/ArcGIS): not imported into the map.
    Web { url: String },
    /// Any other provider we cannot map yet.
    Unsupported { provider: String },
}

/// How well the style of one layer could be carried over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleFidelity {
    /// Renderer type and symbol properties map directly.
    Full,
    /// Renderer maps, symbol details are approximated (stacked symbol layers, SVG markers…).
    Approximate,
    /// The renderer type itself has no counterpart; a default symbol is used.
    Lost,
}

#[derive(Clone, Debug)]
pub struct ImportedLayer {
    pub name: String,
    pub source: ImportSource,
    /// The raw `<datasource>` string, kept in metadata for later pairing.
    pub datasource: String,
    pub visible: bool,
    /// 0 = opaque … 100 = invisible, like RustGIS `Layer::transparency`.
    pub transparency: f32,
    pub min_scale: Option<f64>,
    pub max_scale: Option<f64>,
    pub subset: Option<String>,
    pub renderer: Option<Renderer>,
    pub fidelity: StyleFidelity,
}

#[derive(Clone, Debug)]
pub struct ImportPlan {
    pub title: String,
    pub layers: Vec<ImportedLayer>,
}

/// Auto-match result per layer, computed against the configured source roots and connections.
#[derive(Clone, Debug, PartialEq)]
pub enum MatchStatus {
    /// The file exists inside a configured source root; the layer gets this absolute path.
    PairedFile { path: String },
    /// A database connection with the same host:port exists; pairing is deferred to Phase B.
    CandidateDb { connection: String },
    /// Data is not there yet; the layer is imported as unavailable.
    Unpaired { reason: String },
}

/// Parse `.qgs` XML or `.qgz` (zip-wrapped XML) bytes into a plan.
pub fn parse_project(bytes: &[u8]) -> Result<ImportPlan, StoreError> {
    let xml = if bytes.starts_with(b"PK\x03\x04") {
        read_qgs_from_zip(bytes)?
    } else {
        std::str::from_utf8(bytes).map_err(|_| StoreError::InvalidInput("工程文件不是 UTF-8 文本".into()))?.to_owned()
    };
    if xml.len() > MAX_QGS_BYTES {
        return Err(StoreError::InvalidInput("qgs 文件过大".into()));
    }
    let doc = roxmltree::Document::parse(&xml).map_err(|e| StoreError::InvalidInput(format!("无法解析 QGIS 工程 XML:{e}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "qgis" {
        return Err(StoreError::InvalidInput("这不是 QGIS 工程文件(根元素不是 qgis)".into()));
    }
    let title = root.attribute("projectname").unwrap_or("QGIS 工程").trim().to_owned();
    let by_id = collect_map_layers(root);
    let order = layer_tree_order(root);
    let ordered: Vec<roxmltree::Node> = if order.is_empty() {
        by_id.values().copied().collect()
    } else {
        order.iter().filter_map(|id| by_id.get(id).copied()).collect()
    };
    let mut layers = Vec::with_capacity(ordered.len());
    for node in ordered {
        if let Some(layer) = parse_map_layer(node) {
            layers.push(layer);
        }
    }
    Ok(ImportPlan { title, layers })
}

fn read_qgs_from_zip(bytes: &[u8]) -> Result<String, StoreError> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| StoreError::InvalidInput(format!("无法读取 qgz 压缩包:{e}")))?;
    if archive.len() > MAX_QGZ_ENTRIES {
        return Err(StoreError::InvalidInput("qgz 内条目过多".into()));
    }
    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    let qgs = names
        .iter()
        .find(|n| n.to_ascii_lowercase().ends_with(".qgs"))
        .ok_or_else(|| StoreError::InvalidInput("qgz 中没有找到 .qgs 文件".into()))?
        .clone();
    let mut file = archive.by_name(&qgs).map_err(|e| StoreError::InvalidInput(format!("读取 qgs 失败:{e}")))?;
    if file.size() > MAX_QGS_BYTES as u64 {
        return Err(StoreError::InvalidInput("qgs 文件过大".into()));
    }
    let mut out = Vec::with_capacity(file.size() as usize);
    std::io::Read::read_to_end(&mut file, &mut out).map_err(|e| StoreError::InvalidInput(format!("解压 qgs 失败:{e}")))?;
    String::from_utf8(out).map_err(|_| StoreError::InvalidInput("qgs 不是 UTF-8 文本".into()))
}

/// maplayer nodes by their `<id>` (or generated fallback when the project omits it).
fn collect_map_layers<'a>(root: roxmltree::Node<'a, 'a>) -> std::collections::BTreeMap<String, roxmltree::Node<'a, 'a>> {
    let mut out = std::collections::BTreeMap::new();
    for node in root.descendants() {
        if node.tag_name().name() != "maplayer" {
            continue;
        }
        let id = child_text(node, "id").unwrap_or_else(|| format!("maplayer-{}", out.len()));
        out.insert(id, node);
    }
    out
}

/// The drawing order from the layer tree, when it is present (document order is the fallback).
fn layer_tree_order(root: roxmltree::Node<'_, '_>) -> Vec<String> {
    let mut order = Vec::new();
    for node in root.descendants() {
        if node.tag_name().name() == "layer-tree-layer" {
            if let Some(id) = node.attribute("id") {
                order.push(id.to_owned());
            }
        }
    }
    order
}

fn child_text(node: roxmltree::Node<'_, '_>, tag: &str) -> Option<String> {
    node.children()
        .find(|c| c.tag_name().name() == tag)
        .and_then(|c| c.text())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
}

fn parse_map_layer(node: roxmltree::Node<'_, '_>) -> Option<ImportedLayer> {
    let layer_type = node.attribute("type").unwrap_or("vector");
    if layer_type == "plugin" {
        return None;
    }
    let name = child_text(node, "layername").unwrap_or_else(|| "未命名图层".into());
    let datasource = child_text(node, "datasource").unwrap_or_default();
    let provider = child_text(node, "provider").unwrap_or_default().to_ascii_lowercase();
    let source = classify_source(layer_type, &provider, &datasource);
    let visible = node.attribute("visible").map(|v| v != "0").unwrap_or(true);
    let transparency = transparency_of(node);
    let min_scale = node.attribute("minimumScale").and_then(|v| v.parse::<f64>().ok()).filter(|v| *v > 0.0);
    let max_scale = node.attribute("maximumScale").and_then(|v| v.parse::<f64>().ok()).filter(|v| *v > 0.0);
    let subset = child_text(node, "subsetstring");
    let (renderer, fidelity) = renderer_of(node, layer_type);
    Some(ImportedLayer { name, source, datasource, visible, transparency, min_scale, max_scale, subset, renderer, fidelity })
}

fn classify_source(layer_type: &str, provider: &str, datasource: &str) -> ImportSource {
    if provider.starts_with("postgres") {
        return match parse_postgis_uri(datasource) {
            Some(p) => p,
            None => ImportSource::Unsupported { provider: provider.to_owned() },
        };
    }
    if matches!(provider, "wms" | "wcs" | "wfs" | "xyz" | "arcgismapserver" | "arcgisfeatureserver" | "vrt-wms") {
        return ImportSource::Web { url: datasource.to_owned() };
    }
    if layer_type == "raster" || provider == "gdal" {
        return ImportSource::Raster { path: clean_file_uri(datasource) };
    }
    if provider == "ogr" || provider.is_empty() {
        return ImportSource::File { path: clean_file_uri(datasource) };
    }
    ImportSource::Unsupported { provider: provider.to_owned() }
}

/// Drops OGR layer hints (`|layerid=…`, `|subsetstring=…`) and decodes `file://` prefixes.
fn clean_file_uri(raw: &str) -> String {
    let mut path = raw.split('|').next().unwrap_or(raw).trim().to_owned();
    if let Some(rest) = path.strip_prefix("file://") {
        // Keep a Windows drive (`file:///C:/x` → `C:/x`); otherwise treat it as a POSIX path.
        let mut cleaned = rest.to_owned();
        if let Some(drive) = cleaned.strip_prefix('/') {
            if drive.as_bytes().get(1) == Some(&b':') {
                cleaned = drive.to_owned();
            }
        }
        path = cleaned.replace("%20", " ");
    }
    path
}

/// Parses `dbname='x' host=h port=5432 ... table="public"."t" (geom) sql=...`.
/// Passwords are read only to be skipped; they never leave this function.
fn parse_postgis_uri(uri: &str) -> Option<ImportSource> {
    let mut host = None;
    let mut port = None;
    let mut dbname = None;
    let mut srid = None;
    let mut sql = None;
    for part in split_uri_keys(uri) {
        match part.0.as_str() {
            "host" => host = Some(part.1),
            "port" => port = part.1.parse::<u16>().ok(),
            "dbname" => dbname = Some(part.1),
            "srid" => srid = part.1.parse::<i32>().ok(),
            "sql" => sql = Some(part.1).filter(|s| !s.is_empty()),
            _ => {}
        }
    }
    let (table_raw, geom) = extract_table(uri)?;
    let (schema, table) = match table_raw.split_once('.') {
        Some((s, t)) => (Some(s.trim_matches('"').to_owned()), t.trim_matches('"').to_owned()),
        None => (None, table_raw.trim_matches('"').to_owned()),
    };
    if table.is_empty() {
        return None;
    }
    Some(ImportSource::Postgis {
        host: host.unwrap_or_else(|| "localhost".into()),
        port,
        dbname: dbname.unwrap_or_else(|| "postgres".into()),
        schema,
        table,
        geom,
        srid,
        sql,
    })
}

/// Splits the leading `key=value` pairs (single or double quoted values allowed).
fn split_uri_keys(uri: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let chars: Vec<char> = uri.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let rest: String = chars[i..].iter().collect();
        if rest.starts_with("table=") {
            break;
        }
        let key: String = chars[i..].iter().take_while(|c| c.is_alphanumeric() || **c == '_').collect();
        if key.is_empty() {
            i += 1;
            continue;
        }
        i += key.len();
        if i >= chars.len() || chars[i] != '=' {
            continue;
        }
        i += 1;
        let quote = chars.get(i).copied().filter(|c| *c == '\'' || *c == '"');
        let value_start;
        let value_end;
        match quote {
            Some(q) => {
                i += 1;
                value_start = i;
                while i < chars.len() && chars[i] != q {
                    i += 1;
                }
                value_end = i;
                i += 1;
            }
            None => {
                value_start = i;
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                value_end = i;
            }
        }
        let value: String = chars[value_start..value_end].iter().collect();
        out.push((key.to_ascii_lowercase(), value));
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
    }
    out
}

/// Extracts `table="s"."t" (geom)` from the URI tail.
fn extract_table(uri: &str) -> Option<(String, Option<String>)> {
    let start = uri.find("table=")?;
    let tail = &uri[start + "table=".len()..];
    let mut end = tail.len();
    if let Some(pos) = tail.find(" sql=") {
        end = end.min(pos);
    }
    let table_part = tail[..end].trim();
    let (table, geom) = match table_part.find(" (") {
        Some(pos) => {
            let geom = table_part[pos + 2..].trim_end_matches(')').trim().to_owned();
            (table_part[..pos].trim().to_owned(), if geom.is_empty() { None } else { Some(geom) })
        }
        None => (table_part.to_owned(), None),
    };
    if table.is_empty() {
        return None;
    }
    Some((table, geom))
}

fn transparency_of(node: roxmltree::Node<'_, '_>) -> f32 {
    // QGIS writes it as a maplayer attribute or a <transparency> child (0-100, QGIS 2 style),
    // or as a <layerOpacity> child (0-1, QGIS 3 style).
    let percent = node
        .attribute("transparency")
        .and_then(|v| v.parse::<f32>().ok())
        .or_else(|| child_text(node, "transparency").and_then(|v| v.parse::<f32>().ok()));
    if let Some(value) = percent {
        return value.clamp(0.0, 100.0);
    }
    if let Some(opacity) = child_text(node, "layerOpacity").and_then(|v| v.parse::<f32>().ok()) {
        return ((1.0 - opacity.clamp(0.0, 1.0)) * 100.0).round();
    }
    0.0
}

// ---- symbology -----------------------------------------------------------

/// Reads one QGIS `<prop k="…" v="…"/>` or `<Option name="…" value="…"/>` property.
fn prop_of(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    let mut queue: Vec<roxmltree::Node<'_, '_>> = node.children().collect();
    while let Some(child) = queue.pop() {
        let tag = child.tag_name().name();
        if tag == "prop" && child.attribute("k") == Some(name) {
            return child.attribute("v").map(str::to_owned);
        }
        if tag == "Option" && child.attribute("name") == Some(name) {
            if let Some(value) = child.attribute("value") {
                return Some(value.to_owned());
            }
        }
        queue.extend(child.children());
    }
    None
}

fn color_of(text: &str) -> Option<Rgba> {
    let parts: Vec<&str> = text.trim().split(',').collect();
    if parts.len() < 3 {
        return None;
    }
    let r = parts[0].trim().parse::<u8>().ok()?;
    let g = parts[1].trim().parse::<u8>().ok()?;
    let b = parts[2].trim().parse::<u8>().ok()?;
    let a = parts.get(3).and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(255);
    Some(Rgba::new(r, g, b, a))
}

fn fallback_symbol() -> Symbol {
    Symbol { fill: Rgba::rgb(0x4A, 0x90, 0xD9), stroke: Rgba::rgb(0x17, 0x20, 0x2A), stroke_width: 1.0, size: 5.0, marker: Marker::Circle }
}

/// A 1×1 placeholder so `RasterLayer::from_path` can build the layer record; its pixels are
/// skipped by serialization and replaced by a real read when the file is paired.
fn placeholder_raster() -> Result<Raster, StoreError> {
    Raster::new(
        "qgis-import",
        1,
        1,
        vec![vec![0.0]],
        GeoTransform { origin_x: 0.0, origin_y: 0.0, pixel_width: 1.0, pixel_height: -1.0 },
        Crs::LonLat,
        None,
        ColorHint::Gray,
        "Float32",
    )
    .map_err(|e| StoreError::InvalidInput(format!("占位栅格创建失败:{e}")))
}

fn symbol_from(node: roxmltree::Node<'_, '_>) -> (Symbol, StyleFidelity) {
    let default = fallback_symbol();
    let Some(layer) = node.children().find(|c| c.tag_name().name() == "layer") else {
        return (default, StyleFidelity::Approximate);
    };
    let class = layer.attribute("class").unwrap_or("");
    let number = |name: &str| prop_of(layer, name).and_then(|v| v.parse::<f32>().ok());
    match class {
        "SimpleFill" => {
            let mut s = default;
            s.fill = prop_of(layer, "color").and_then(|v| color_of(&v)).unwrap_or(s.fill);
            s.stroke = prop_of(layer, "outline_color").and_then(|v| color_of(&v)).unwrap_or(s.stroke);
            s.stroke_width = number("outline_width").unwrap_or(s.stroke_width);
            (s, StyleFidelity::Full)
        }
        "SimpleLine" => {
            let mut s = default;
            let line = prop_of(layer, "line_color").and_then(|v| color_of(&v)).unwrap_or(s.fill);
            s.fill = line;
            s.stroke = line;
            s.stroke_width = number("line_width").unwrap_or(s.stroke_width);
            (s, StyleFidelity::Full)
        }
        "SimpleMarker" | "PointSymbol" => {
            let mut s = default;
            s.fill = prop_of(layer, "color").and_then(|v| color_of(&v)).unwrap_or(s.fill);
            s.stroke = prop_of(layer, "outline_color").and_then(|v| color_of(&v)).unwrap_or(s.stroke);
            s.stroke_width = number("outline_width").unwrap_or(s.stroke_width);
            s.size = number("size").unwrap_or(s.size);
            (s, StyleFidelity::Full)
        }
        _ => (default, StyleFidelity::Approximate),
    }
}

/// Symbols of a `<renderer-v2>`, indexed like QGIS references them (`symbol="0"` on a category
/// or range). The symbol's own index attribute is `name` in QGIS 3 and `id` in older files.
fn renderer_symbols<'a>(node: roxmltree::Node<'a, 'a>) -> Vec<(usize, roxmltree::Node<'a, 'a>)> {
    let index_of = |s: roxmltree::Node<'_, '_>| {
        s.attribute("name").or_else(|| s.attribute("id")).or_else(|| s.attribute("symbol")).and_then(|v| v.parse::<usize>().ok())
    };
    let mut out = Vec::new();
    for sym in node.children().filter(|c| c.tag_name().name() == "symbols") {
        for s in sym.children().filter(|c| c.tag_name().name() == "symbol") {
            if let Some(index) = index_of(s) {
                out.push((index, s));
            }
        }
    }
    out.sort_by_key(|(i, _)| *i);
    out
}

fn renderer_of(node: roxmltree::Node<'_, '_>, layer_type: &str) -> (Option<Renderer>, StyleFidelity) {
    if layer_type == "raster" {
        // Raster renderers are not mapped in Phase A; the default raster style is used.
        return (None, StyleFidelity::Approximate);
    }
    let Some(renderer) = node.children().find(|c| c.tag_name().name() == "renderer-v2") else {
        return (None, StyleFidelity::Lost);
    };
    let symbols = renderer_symbols(renderer);
    let symbol_at = |index: usize| -> Option<Symbol> { symbols.iter().find(|(i, _)| *i == index).map(|(_, n)| symbol_from(*n).0) };
    match renderer.attribute("type").unwrap_or("") {
        "singleSymbol" => {
            let (symbol, fidelity) =
                symbol_at(0).map(|s| (s, StyleFidelity::Full)).unwrap_or_else(|| (fallback_symbol(), StyleFidelity::Lost));
            (Some(Renderer::Single(symbol)), fidelity)
        }
        "categorizedSymbol" => {
            let field = renderer.attribute("attr").unwrap_or_default().to_owned();
            let mut classes: Vec<(String, Symbol)> = Vec::new();
            let mut other = fallback_symbol();
            let mut got = 0;
            for cats in renderer.children().filter(|c| c.tag_name().name() == "categories") {
                for c in cats.children().filter(|c| c.tag_name().name() == "category") {
                    let Some(index) = c.attribute("symbol").and_then(|v| v.parse::<usize>().ok()) else { continue };
                    let Some(symbol) = symbol_at(index) else { continue };
                    let value = c.attribute("value").unwrap_or_default().to_owned();
                    if got == 0 {
                        other = symbol.clone();
                    }
                    classes.push((value, symbol));
                    got += 1;
                }
            }
            if classes.is_empty() {
                return (None, StyleFidelity::Lost);
            }
            (
                Some(Renderer::Unique {
                    field,
                    classes,
                    other,
                    ramp: rustgis_engine::map::Ramp::Viridis,
                    class_labels: Default::default(),
                    other_label: None,
                }),
                StyleFidelity::Full,
            )
        }
        "graduatedSymbol" => {
            let field = renderer.attribute("attr").unwrap_or_default().to_owned();
            let mut breaks = Vec::new();
            let mut class_symbols = Vec::new();
            for ranges in renderer.children().filter(|c| c.tag_name().name() == "ranges") {
                for r in ranges.children().filter(|c| c.tag_name().name() == "range") {
                    let Some(index) = r.attribute("symbol").and_then(|v| v.parse::<usize>().ok()) else { continue };
                    let Some(upper) = r.attribute("upper").and_then(|v| v.parse::<f64>().ok()) else { continue };
                    let Some(symbol) = symbol_at(index) else { continue };
                    breaks.push(upper);
                    class_symbols.push(symbol);
                }
            }
            if breaks.is_empty() {
                return (None, StyleFidelity::Lost);
            }
            let min = *breaks.first().unwrap_or(&0.0);
            (
                Some(Renderer::Graduated {
                    field,
                    breaks,
                    symbols: class_symbols,
                    ramp: rustgis_engine::map::Ramp::Viridis,
                    method: rustgis_engine::map::ClassMethod::NaturalBreaks,
                    min,
                    class_labels: Default::default(),
                }),
                StyleFidelity::Full,
            )
        }
        _ => (None, StyleFidelity::Lost),
    }
}

// ---- matching and project building ---------------------------------------

/// Resolve one layer's data binding against the configured sources and connections.
pub fn match_layer(layer: &ImportedLayer, sources: &SourceRegistry, connections: &[crate::databases::DatabaseView]) -> MatchStatus {
    match &layer.source {
        ImportSource::File { path } | ImportSource::Raster { path } => {
            if let crate::sources::ReferenceState::Available = sources.check_reference(path).state {
                if let Some(resolved) = absolute_for(sources, path) {
                    return MatchStatus::PairedFile { path: resolved };
                }
            }
            // Try the bare file name as a relative reference in every root.
            let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
            if !name.is_empty() {
                if let crate::sources::ReferenceState::Available = sources.check_reference(name).state {
                    if let Some(resolved) = absolute_for(sources, name) {
                        return MatchStatus::PairedFile { path: resolved };
                    }
                }
            }
            MatchStatus::Unpaired { reason: "未找到数据文件,请先准备数据或上传".into() }
        }
        ImportSource::Postgis { host, port, .. } => {
            let hit = connections.iter().find(|c| c.host == *host && port.map_or(true, |p| c.port == p));
            match hit {
                Some(c) => MatchStatus::CandidateDb { connection: c.display_name.clone() },
                None => MatchStatus::Unpaired { reason: "没有同主机的数据库连接,请先添加连接".into() },
            }
        }
        ImportSource::Web { .. } => MatchStatus::Unpaired { reason: "Web 服务图层暂不导入".into() },
        ImportSource::Unsupported { provider } => MatchStatus::Unpaired { reason: format!("暂不支持的 provider:{provider}") },
    }
}

/// The absolute path a relative reference resolved to (the first root that holds it).
fn absolute_for(sources: &SourceRegistry, reference: &str) -> Option<String> {
    let path = std::path::Path::new(reference);
    if path.is_absolute() {
        return Some(reference.to_owned());
    }
    for folder in sources.folders() {
        if !folder.enabled {
            continue;
        }
        let Ok(root) = std::fs::canonicalize(&folder.root_path) else { continue };
        let candidate = root.join(reference);
        if let Ok(found) = std::fs::canonicalize(&candidate) {
            if found.starts_with(&root) && found.is_file() {
                return Some(found.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Builds the RustGIS project for an import. `matches` aligns with `plan.layers`.
pub fn build_project(plan: &ImportPlan, matches: &[MatchStatus]) -> Result<String, StoreError> {
    let mut project = Project::new(plan.title.as_str());
    for (layer, status) in plan.layers.iter().zip(matches) {
        if matches!(layer.source, ImportSource::Web { .. }) {
            continue; // web layers are metadata-only in Phase A
        }
        match &layer.source {
            ImportSource::File { path } => {
                let bound = match status {
                    MatchStatus::PairedFile { path } => path.clone(),
                    _ => path.clone(),
                };
                let mut source = SourceLayer::default();
                source.path = Some(bound);
                project.map_mut().add_streamed(layer.name.clone(), source);
            }
            ImportSource::Raster { path } => {
                let bound = match status {
                    MatchStatus::PairedFile { path } => path.clone(),
                    _ => path.clone(),
                };
                // `RasterLayer` can only be built through `from_path`; the pixels are
                // `#[serde(skip)]`, so this placeholder is never written into the project.
                let placeholder = placeholder_raster()?;
                let raster = RasterLayer::from_path(placeholder, bound);
                project.map_mut().add_raster(layer.name.clone(), raster);
                continue;
            }
            ImportSource::Postgis { host, port, schema, table, geom, srid, .. } => {
                let table_ref = match (schema, geom) {
                    (Some(s), Some(g)) => format!("{s}.{table}:{g}"),
                    (Some(s), None) => format!("{s}.{table}"),
                    (None, Some(g)) => format!("{table}:{g}"),
                    (None, None) => table.clone(),
                };
                let origin = PostgisOrigin {
                    database: format!("postgres:host={host} port={}", port.unwrap_or(5432)),
                    table: table_ref,
                    family: None,
                    crs: srid.map(|s| format!("EPSG:{s}")),
                };
                let mut source = SourceLayer::default();
                source.postgis = Some(origin);
                project.map_mut().add_streamed(layer.name.clone(), source);
            }
            ImportSource::Unsupported { .. } | ImportSource::Web { .. } => continue,
        }
        finish_layer(&mut project, layer);
    }
    serde_json::to_string(&project).map_err(|e| StoreError::InvalidProject(e.to_string()))
}

fn finish_layer(project: &mut Project, layer: &ImportedLayer) {
    let Some(target) = project.map().layer_by_name(&layer.name) else { return };
    let id = target.id;
    let Some(l) = project.map_mut().layer_mut(id) else { return };
    l.visible = layer.visible;
    l.transparency = layer.transparency;
    l.min_scale = layer.min_scale;
    l.max_scale = layer.max_scale;
    if let Some(query) = &layer.subset {
        l.definition_query = Some(query.clone());
    }
    if let Some(renderer) = &layer.renderer {
        l.renderer = renderer.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<qgis projectname="测区底图" version="3.34">
      <projectlayers>
        <maplayer type="vector" visible="1" geometry="Point" minimumScale="0" maximumScale="50000">
          <id>pts</id>
          <layername>监测点</layername>
          <provider>postgres</provider>
          <datasource>dbname='gis' host=db.example port=55432 user='u' password='SECRET' srid=4326 table="public"."points" (geom) sql=</datasource>
          <renderer-v2 type="categorizedSymbol" attr="type">
            <categories>
              <category symbol="0" value="A" label="甲类"/>
              <category symbol="1" value="B" label="乙类"/>
            </categories>
            <symbols>
              <symbol type="marker" name="0"><layer class="SimpleMarker"><Option type="Map">
                <Option name="color" value="227,26,28,255"/><Option name="outline_color" value="0,0,0,255"/><Option name="size" value="4.5"/>
              </Option></layer></symbol>
              <symbol type="marker" name="1"><layer class="SimpleMarker"><Option type="Map">
                <Option name="color" value="31,120,180,255"/>
              </Option></layer></symbol>
            </symbols>
          </renderer-v2>
        </maplayer>
        <maplayer type="vector" visible="1">
          <id>lines</id>
          <layername>道路</layername>
          <provider>ogr</provider>
          <datasource>./roads.shp|layername=roads</datasource>
          <transparency>20</transparency>
          <renderer-v2 type="singleSymbol">
            <symbols>
              <symbol type="line" name="0"><layer class="SimpleLine"><Option type="Map">
                <Option name="line_color" value="51,160,44,255"/><Option name="line_width" value="1.2"/>
              </Option></layer></symbol>
            </symbols>
          </renderer-v2>
        </maplayer>
        <maplayer type="vector" visible="0">
          <id>wms</id>
          <layername>底图服务</layername>
          <provider>wms</provider>
          <datasource>url=https://example.test/tiles</datasource>
        </maplayer>
      </projectlayers>
      <layer-tree-group><layer-tree-layer id="wms"/><layer-tree-layer id="lines"/><layer-tree-layer id="pts"/></layer-tree-group>
    </qgis>"#;

    #[test]
    fn parses_layers_styles_and_order() {
        let plan = parse_project(SAMPLE.as_bytes()).expect("parse");
        assert_eq!(plan.title, "测区底图");
        assert_eq!(plan.layers.len(), 3);
        // Drawing order follows the layer tree: wms, lines, pts.
        assert_eq!(plan.layers[0].name, "底图服务");
        assert_eq!(plan.layers[1].name, "道路");
        assert_eq!(plan.layers[2].name, "监测点");

        let road = &plan.layers[1];
        assert_eq!(road.source, ImportSource::File { path: "./roads.shp".into() });
        assert_eq!(road.transparency, 20.0);
        assert!(matches!(road.renderer, Some(Renderer::Single(_))));

        let points = &plan.layers[2];
        match &points.source {
            ImportSource::Postgis { host, port, schema, table, geom, srid, .. } => {
                assert_eq!(host, "db.example");
                assert_eq!(*port, Some(55432));
                assert_eq!(schema.as_deref(), Some("public"));
                assert_eq!(table, "points");
                assert_eq!(geom.as_deref(), Some("geom"));
                assert_eq!(*srid, Some(4326));
            }
            other => panic!("expected postgis, got {other:?}"),
        }
        // The secret never survives parsing.
        assert!(!format!("{:?}", points.source).contains("SECRET"));
        match &points.renderer {
            Some(Renderer::Unique { field, classes, .. }) => {
                assert_eq!(field, "type");
                assert_eq!(classes.len(), 2);
                assert_eq!(classes[0].0, "A");
            }
            other => panic!("expected unique renderer, got {other:?}"),
        }
        assert!(matches!(plan.layers[0].source, ImportSource::Web { .. }));
    }

    #[test]
    fn builds_a_project_with_unpaired_layers_retained() {
        let plan = parse_project(SAMPLE.as_bytes()).expect("parse");
        let statuses = vec![
            MatchStatus::Unpaired { reason: "web".into() },
            MatchStatus::Unpaired { reason: "missing".into() },
            MatchStatus::CandidateDb { connection: "c".into() },
        ];
        let text = build_project(&plan, &statuses).expect("build");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        let layers = value["maps"][0]["layers"].as_array().expect("layers");
        assert_eq!(layers.len(), 2, "web layer is not imported");
        assert!(text.contains("道路"));
        assert!(text.contains("./roads.shp"));
        assert!(text.contains("监测点"));
        assert!(text.contains("postgres:host=db.example"));
        // Style carried over: the categorized renderer keeps its classes.
        assert!(text.contains("\"classes\""));
    }

    #[test]
    fn zip_files_are_unwrapped() {
        let mut zip_bytes = Vec::new();
        {
            let mut cursor = std::io::Cursor::new(&mut zip_bytes);
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options: zip::write::SimpleFileOptions = Default::default();
            writer.start_file("project.qgs", options).expect("start");
            std::io::Write::write_all(&mut writer, SAMPLE.as_bytes()).expect("write");
            writer.finish().expect("finish");
        }
        let plan = parse_project(&zip_bytes).expect("parse qgz");
        assert_eq!(plan.layers.len(), 3);
    }

    #[test]
    fn non_qgis_files_are_rejected() {
        let err = parse_project(b"<html></html>").expect_err("not qgis");
        assert!(matches!(err, StoreError::InvalidInput(_)));
    }
}
