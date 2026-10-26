//! Layer pairing for imported projects (Phase B). The console can bind an unpaired layer to a
//! file inside a configured source root, or to a PostGIS table (fetched as a bounded snapshot
//! and embedded, like every database layer in the preview).
//!
//! Pairing always writes a working copy; immutable versions are never touched.

use serde::Deserialize;

use crate::databases::DatabaseRegistry;
use crate::error::StoreError;
use crate::sources::SourceRegistry;
use crate::ProjectStore;

#[derive(Deserialize)]
pub struct PairRequest {
    #[serde(rename = "layerName")]
    pub layer_name: String,
    #[serde(default)]
    pub file: Option<PairFile>,
    #[serde(default)]
    pub postgis: Option<PairPostgis>,
}

#[derive(Deserialize)]
pub struct PairFile {
    pub path: String,
}

#[derive(Deserialize)]
pub struct PairPostgis {
    pub connection: String,
    pub database: String,
    #[serde(default)]
    pub schema: Option<String>,
    pub table: String,
}

/// Pairs one layer of a working copy. Returns the updated project metadata and the layer's
/// availability state (available once its data could be read).
pub fn pair_layer(
    store: &mut ProjectStore,
    sources: &SourceRegistry,
    databases: &DatabaseRegistry,
    project: &str,
    working: &str,
    request: &PairRequest,
) -> Result<crate::project_store::ProjectMeta, StoreError> {
    let mut copy = store.read_working_copy(project, working)?;
    let mut value: serde_json::Value =
        serde_json::from_str(&copy.content).map_err(|e| StoreError::InvalidProject(format!("工作副本不是有效 JSON:{e}")))?;
    let Some(maps) = value["maps"].as_array_mut() else {
        return Err(StoreError::InvalidProject("工程中没有地图".into()));
    };
    let Some(map) = maps.first_mut() else {
        return Err(StoreError::InvalidProject("工程中没有地图".into()));
    };
    let Some(layers) = map["layers"].as_array_mut() else {
        return Err(StoreError::InvalidProject("工程中没有图层列表".into()));
    };
    let mut found = false;
    for layer in layers.iter_mut() {
        if layer["name"].as_str() != Some(request.layer_name.as_str()) {
            continue;
        }
        found = true;
        if let Some(file) = &request.file {
            pair_file_layer(layer, sources, file)?;
        } else if let Some(postgis) = &request.postgis {
            pair_postgis_layer(layer, databases, postgis)?;
        } else {
            return Err(StoreError::InvalidInput("请求既没有文件也没有数据库配对信息".into()));
        }
        break;
    }
    if !found {
        return Err(StoreError::InvalidInput(format!("工程中没有名为「{}」的图层", request.layer_name)));
    }
    let text = serde_json::to_string(&value).map_err(|e| StoreError::InvalidProject(e.to_string()))?;
    crate::project_store::validate_project(&text)?;
    store.save_working_copy_content(project, working, &text)?;
    mark_paired(store, project, &request.layer_name, request)?;
    store.project(project)
}

fn pair_file_layer(layer: &mut serde_json::Value, sources: &SourceRegistry, file: &PairFile) -> Result<(), StoreError> {
    let reference = file.path.trim();
    if reference.is_empty() {
        return Err(StoreError::InvalidInput("文件路径为空".into()));
    }
    if let crate::sources::ReferenceState::Available = sources.check_reference(reference).state {
        // Prefer the canonical absolute path so the reference stays valid across rescans.
        let absolute = std::path::Path::new(reference);
        let bound = if absolute.is_absolute() {
            reference.to_owned()
        } else {
            sources
                .folders()
                .iter()
                .filter(|f| f.enabled)
                .filter_map(|f| std::fs::canonicalize(&f.root_path).ok())
                .filter_map(|root| std::fs::canonicalize(root.join(reference)).ok())
                .find(|found| found.is_file())
                .map(|found| found.to_string_lossy().into_owned())
                .unwrap_or_else(|| reference.to_owned())
        };
        if layer["raster"].is_object() {
            layer["raster"]["path"] = serde_json::Value::String(bound);
        } else {
            layer["source"]["path"] = serde_json::Value::String(bound);
        }
        Ok(())
    } else {
        Err(StoreError::InvalidInput("文件不在任何已配置的数据源目录内".into()))
    }
}

fn pair_postgis_layer(layer: &mut serde_json::Value, databases: &DatabaseRegistry, postgis: &PairPostgis) -> Result<(), StoreError> {
    let table = match &postgis.schema {
        Some(schema) if !schema.is_empty() => format!("{}.{}", schema, postgis.table),
        _ => postgis.table.clone(),
    };
    // The snapshot is bounded exactly like the database browser's「加入地图」.
    let bytes = databases.geojson(&postgis.connection, &postgis.database, &table, 5_000)?;
    let (dataset, _) = rustgis_data::io::read_bytes("pair.geojson", &bytes)
        .map_err(|e| StoreError::InvalidInput(format!("快照解析失败:{e}")))?;
    let dataset = serde_json::to_value(&dataset).map_err(|e| StoreError::InvalidProject(e.to_string()))?;
    layer["dataset"] = dataset;
    Ok(())
}

fn mark_paired(store: &mut ProjectStore, project: &str, layer_name: &str, request: &PairRequest) -> Result<(), StoreError> {
    let mut meta = store.project(project)?;
    let Some(import) = meta.import.as_mut() else { return Ok(()) };
    for entry in import.layers.iter_mut() {
        if entry.name == layer_name {
            entry.status = "paired".into();
            entry.target = Some(match (&request.file, &request.postgis) {
                (Some(file), _) => file.path.clone(),
                (None, Some(postgis)) => format!("{}:{}/{}", postgis.connection, postgis.database, table_of(postgis)),
                _ => String::new(),
            });
        }
    }
    let import = std::mem::take(&mut meta.import).unwrap_or_default();
    store.set_import_meta(project, import)?;
    Ok(())
}

fn table_of(postgis: &PairPostgis) -> String {
    match &postgis.schema {
        Some(schema) if !schema.is_empty() => format!("{}.{}", schema, postgis.table),
        _ => postgis.table.clone(),
    }
}
