//! Opening a project must not rewrite it, and a missing external source must not make the whole
//! project fail to open or leak files outside the configured roots. This module derives an
//! availability report and a prepared copy of the project:
//!
//! * the report names every layer, and every file-API COG tile, whose external file was checked,
//!   with a reason;
//! * the prepared copy sets unavailable layers and COG tiles to `visible = false` and adds a
//!   `x_rustgis_availability` marker, but never changes the stored version;
//! * an embedded layer, a remote COG URL, or another URL that cannot be checked offline, is not
//!   hidden;
//! * parsing uses `serde_json::from_str::<Project>` and deliberately skips `Project::normalize`,
//!   so opening a version never touches source files, databases or the network.
//!
//! The prepared JSON is a derived view. Opening the original version again after the data returns
//! restores the layer because the stored visibility was never changed.

use serde::Serialize;
use serde_json::Value;

use rustgis_engine::Project;
use rustgis_engine::map::{Layer, TileKind, TileLayer};

use crate::StoreError;
use crate::sources::{ReferenceCheck, ReferenceState, SourceRegistry};

#[derive(Clone, Debug, Serialize)]
pub struct LayerAvailability {
    pub map_index: usize,
    pub map_name: String,
    pub layer_index: usize,
    pub layer_id: u32,
    /// Set for a COG tile layer in `maps[].tiles`; `layer_id` is 0 for those entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tile_index: Option<usize>,
    pub name: String,
    /// `file`, `raster`, `cog`, `embedded`, `postgis`, `database` or `empty`.
    pub kind: String,
    /// The configured source root that supplied the file, when one matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// `available`, `unavailable` or `unknown`.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub original_visible: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AvailabilityReport {
    pub layers: Vec<LayerAvailability>,
    pub unavailable: usize,
    pub unknown: usize,
}

#[derive(Clone, Debug)]
pub struct PreparedProject {
    pub content: String,
    pub availability: AvailabilityReport,
}

impl AvailabilityReport {
    fn from(layers: Vec<LayerAvailability>) -> Self {
        let unavailable = layers.iter().filter(|l| l.status == "unavailable").count();
        let unknown = layers.iter().filter(|l| l.status == "unknown").count();
        Self { layers, unavailable, unknown }
    }
}

/// Checks every external layer reference without reading the referenced data. A project that is
/// too new or is not valid JSON is rejected with `InvalidProject`; the caller keeps the original
/// version intact.
pub fn prepare(text: &str, sources: &SourceRegistry) -> Result<PreparedProject, StoreError> {
    let project: Project = serde_json::from_str(text).map_err(|e| StoreError::InvalidProject(e.to_string()))?;
    if project.format > rustgis_engine::project::FORMAT {
        return Err(StoreError::InvalidProject(format!("project format {} is newer than this server", project.format)));
    }
    let mut raw: Value = serde_json::from_str(text).map_err(|e| StoreError::InvalidProject(e.to_string()))?;
    let mut report = Vec::new();
    for (map_index, map) in project.maps.iter().enumerate() {
        for (layer_index, layer) in map.layers.iter().enumerate() {
            let check = check_layer(layer, sources);
            if check.status == "unavailable"
                && let Some(raw_layer) = raw
                    .get_mut("maps")
                    .and_then(Value::as_array_mut)
                    .and_then(|maps| maps.get_mut(map_index))
                    .and_then(|map| map.get_mut("layers"))
                    .and_then(Value::as_array_mut)
                    .and_then(|layers| layers.get_mut(layer_index))
                    .and_then(Value::as_object_mut)
            {
                raw_layer.insert("visible".into(), Value::Bool(false));
                raw_layer.insert(
                    "x_rustgis_availability".into(),
                    serde_json::json!({
                        "status": check.status,
                        "reason": check.reason,
                    }),
                );
            }
            report.push(LayerAvailability {
                map_index,
                map_name: map.name.clone(),
                layer_index,
                layer_id: layer.id,
                tile_index: None,
                name: layer.name.clone(),
                kind: check.kind.to_owned(),
                source_id: check.source_id,
                status: check.status.to_owned(),
                reason: check.reason,
                original_visible: layer.visible,
            });
        }
        // A COG added from the map's URL import is stored as a tile layer, not in `layers`.
        // Check only file-API COG URLs; remote tile services stay unknown and visible.
        for (tile_index, tile) in map.tiles.iter().enumerate() {
            if tile.kind != TileKind::Cog {
                continue;
            }
            let check = check_tile(tile, sources);
            if check.status == "unavailable"
                && let Some(raw_tile) = raw
                    .get_mut("maps")
                    .and_then(Value::as_array_mut)
                    .and_then(|maps| maps.get_mut(map_index))
                    .and_then(|map| map.get_mut("tiles"))
                    .and_then(Value::as_array_mut)
                    .and_then(|tiles| tiles.get_mut(tile_index))
                    .and_then(Value::as_object_mut)
            {
                raw_tile.insert("visible".into(), Value::Bool(false));
                raw_tile.insert(
                    "x_rustgis_availability".into(),
                    serde_json::json!({
                        "status": check.status,
                        "reason": check.reason,
                    }),
                );
            }
            report.push(LayerAvailability {
                map_index,
                map_name: map.name.clone(),
                layer_index: tile_index,
                layer_id: 0,
                tile_index: Some(tile_index),
                name: tile.name.clone(),
                kind: check.kind.to_owned(),
                source_id: check.source_id,
                status: check.status.to_owned(),
                reason: check.reason,
                original_visible: tile.visible,
            });
        }
    }
    let content = serde_json::to_string(&raw).map_err(|e| StoreError::InvalidProject(e.to_string()))?;
    Ok(PreparedProject { content, availability: AvailabilityReport::from(report) })
}

struct LayerCheck {
    kind: &'static str,
    status: &'static str,
    reason: Option<String>,
    source_id: Option<String>,
}

fn checked(kind: &'static str, status: &'static str, reason: impl Into<String>) -> LayerCheck {
    LayerCheck { kind, status, reason: Some(reason.into()), source_id: None }
}

fn check_layer(layer: &Layer, sources: &SourceRegistry) -> LayerCheck {
    if let Some(raster) = &layer.raster {
        if raster.embedded.is_some() {
            return checked("embedded", "available", "栅格内嵌在工程中");
        }
        let Some(path) = raster.path.as_deref() else {
            return checked("raster", "unavailable", "栅格既没有路径也没有内嵌数据");
        };
        return from_reference(source_check(sources, path), "raster");
    }
    if let Some(source) = &layer.source {
        if source.embedded.is_some() {
            return checked(
                "embedded",
                "available",
                "数据源内嵌在工程中;显示时不需要原始数据",
            );
        }
        if let Some(path) = source.path.as_deref() {
            return from_reference(source_check(sources, path), "file");
        }
        if source.postgis.is_some() {
            if !layer.dataset.features.is_empty() {
                return checked("postgis", "available", "存储的快照包含要素;不会查询数据库中的原表");
            }
            return checked(
                "postgis",
                "unavailable",
                "浏览器预览不支持实时 PostGIS 图层;请在数据库浏览器中添加有界快照",
            );
        }
        if source.mysql.is_some() || source.sqlserver.is_some() || source.duckdb.is_some() {
            if !layer.dataset.features.is_empty() {
                return checked("database", "available", "工程中保存了要素副本");
            }
            return checked("database", "unavailable", "该数据库 provider 在浏览器预览中不可用");
        }
        if source.gdal.is_some() {
            if !layer.dataset.features.is_empty() {
                return checked("embedded", "available", "工程中保存了要素副本");
            }
            return checked("file", "unavailable", "GDAL 后端的图层在浏览器预览中不可用");
        }
        if !layer.dataset.features.is_empty() {
            return checked("embedded", "available", "要素存储在工程中");
        }
        return checked("empty", "unavailable", "流式数据源没有路径、内嵌数据或要素");
    }
    checked("embedded", "available", "图层及其要素存储在工程中")
}

fn check_tile(tile: &TileLayer, sources: &SourceRegistry) -> LayerCheck {
    match sources.check_file_url(&tile.url) {
        Some(check) => from_reference(check, "cog"),
        None => checked("cog", "unknown", "服务端不检查远程瓦片 URL"),
    }
}

fn source_check(sources: &SourceRegistry, path: &str) -> ReferenceCheck {
    sources.check_reference(path)
}

fn from_reference(check: ReferenceCheck, kind: &'static str) -> LayerCheck {
    let source_id = check.source_id.clone();
    match check.state {
        ReferenceState::Available => LayerCheck { kind, status: "available", reason: Some(check.reason), source_id },
        ReferenceState::Remote => LayerCheck { kind, status: "unknown", reason: Some(check.reason), source_id },
        ReferenceState::Missing | ReferenceState::Disabled | ReferenceState::Outside => {
            LayerCheck { kind, status: "unavailable", reason: Some(check.reason), source_id }
        }
    }
}
