//! Phase 6: projects are loaded without `normalize`, missing layers are retained and hidden only
//! in the derived copy, and source paths can recover after the data returns.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustgis_data::raster::{ColorHint, Crs, GeoTransform, Raster};
use rustgis_engine::Project;
use rustgis_engine::map::source::SourceLayer;
use rustgis_engine::map::{RasterLayer, TileKind, TileLayer};
use rustgis_preview::{DataHome, SourceRegistry, availability};

fn temp_home(name: &str) -> (PathBuf, DataHome) {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("rustgis-preview-avail-{name}-{}-{nanos}", std::process::id()));
    let home = DataHome::open(&root).expect("data home");
    (root, home)
}

fn write(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, "{}").expect("fixture");
}

fn source_project(path: &Path, visible: bool) -> String {
    let mut project = Project::new("Availability");
    let mut source = SourceLayer::default();
    source.path = Some(path.to_string_lossy().into_owned());
    let id = project.map_mut().add_streamed("streamed", source);
    if let Some(layer) = project.map_mut().layer_mut(id) {
        layer.visible = visible;
    }
    serde_json::to_string(&project).expect("project json")
}

fn tiny_raster() -> Raster {
    Raster::new(
        "tiny",
        1,
        1,
        vec![vec![1.0]],
        GeoTransform { origin_x: 0.0, origin_y: 0.0, pixel_width: 1.0, pixel_height: -1.0 },
        Crs::LonLat,
        None,
        ColorHint::Gray,
        "Float32",
    )
    .expect("raster")
}

fn layer_visible(content: &str) -> bool {
    let value: serde_json::Value = serde_json::from_str(content).expect("prepared json");
    value["maps"][0]["layers"][0]["visible"].as_bool().expect("visible")
}

fn tile_visible(content: &str) -> bool {
    let value: serde_json::Value = serde_json::from_str(content).expect("prepared json");
    value["maps"][0]["tiles"][0]["visible"].as_bool().expect("visible")
}

#[test]
fn missing_file_is_unavailable_hidden_and_recovers_without_rewriting_the_project() {
    let (root, home) = temp_home("recover");
    let data = root.join("data");
    let file = data.join("places.geojson");
    write(&file);
    let mut sources = SourceRegistry::open(home).expect("registry");
    let folder = sources.add_folder(data.to_str().expect("utf-8"), None).expect("folder");
    let text = source_project(&file, true);

    let prepared = availability::prepare(&text, &sources).expect("prepare");
    assert_eq!(prepared.availability.unavailable, 0);
    assert_eq!(prepared.availability.layers.len(), 1);
    assert_eq!(prepared.availability.layers[0].status, "available");
    assert!(layer_visible(&prepared.content));

    std::fs::remove_file(&file).expect("delete source");
    let prepared = availability::prepare(&text, &sources).expect("prepare missing");
    assert_eq!(prepared.availability.unavailable, 1);
    assert_eq!(prepared.availability.layers[0].status, "unavailable");
    assert!(prepared.availability.layers[0].original_visible);
    assert_eq!(prepared.availability.layers[0].reason.as_deref(), Some("文件缺失"));
    assert!(!layer_visible(&prepared.content), "an unavailable layer is hidden in the derived copy");
    assert_eq!(text, source_project(&file, true), "preparing does not rewrite the stored project");

    write(&file);
    let prepared = availability::prepare(&text, &sources).expect("prepare recovered");
    assert_eq!(prepared.availability.unavailable, 0);
    assert_eq!(prepared.availability.layers[0].status, "available");
    assert!(layer_visible(&prepared.content));
    assert_eq!(prepared.availability.layers[0].source_id.as_deref(), Some(folder.id.as_str()));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_file_outside_every_source_root_is_never_probed() {
    let (root, home) = temp_home("outside");
    let data = root.join("data");
    let outside = root.join("outside").join("secret.geojson");
    write(&outside);
    std::fs::create_dir_all(&data).expect("data");
    let mut sources = SourceRegistry::open(home).expect("registry");
    sources.add_folder(data.to_str().expect("utf-8"), None).expect("folder");
    let text = source_project(&outside, true);
    let prepared = availability::prepare(&text, &sources).expect("prepare");
    assert_eq!(prepared.availability.layers[0].status, "unavailable");
    assert!(prepared.availability.layers[0].reason.as_deref().is_some_and(|r| r.contains("之外")));
    assert!(!layer_visible(&prepared.content));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn disabled_sources_and_remote_urls_have_distinct_states() {
    let (root, home) = temp_home("states");
    let data = root.join("data");
    let file = data.join("places.geojson");
    write(&file);
    let mut sources = SourceRegistry::open(home).expect("registry");
    let folder = sources.add_folder(data.to_str().expect("utf-8"), None).expect("folder");
    sources.set_enabled(&folder.id, false).expect("disable");
    let text = source_project(&file, true);
    let prepared = availability::prepare(&text, &sources).expect("prepare disabled");
    assert_eq!(prepared.availability.layers[0].status, "unavailable");
    assert_eq!(prepared.availability.layers[0].reason.as_deref(), Some("数据源目录已停用"));

    let remote = source_project(Path::new("https://example.test/places.geojson"), true);
    let prepared = availability::prepare(&remote, &sources).expect("prepare remote");
    assert_eq!(prepared.availability.layers[0].status, "unknown");
    assert!(layer_visible(&prepared.content), "remote references are not hidden without a network check");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn embedded_data_and_raster_references_are_checked_without_reads() {
    let (root, home) = temp_home("embedded");
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data");
    let mut sources = SourceRegistry::open(home).expect("registry");
    sources.add_folder(data.to_str().expect("utf-8"), None).expect("folder");

    let mut project = Project::new("Embedded");
    let mut source = SourceLayer::default();
    source.embedded = Some(Arc::new(vec![1, 2, 3]));
    project.map_mut().add_streamed("embedded", source);
    let prepared = availability::prepare(&serde_json::to_string(&project).expect("json"), &sources).expect("prepare");
    assert_eq!(prepared.availability.layers[0].status, "available");
    assert_eq!(prepared.availability.layers[0].kind, "embedded");

    let mut project = Project::new("Raster");
    project.map_mut().add_raster("missing", RasterLayer::from_path(tiny_raster(), data.join("missing.tif").to_string_lossy()));
    let prepared = availability::prepare(&serde_json::to_string(&project).expect("json"), &sources).expect("prepare");
    assert_eq!(prepared.availability.layers[0].status, "unavailable");
    assert_eq!(prepared.availability.layers[0].kind, "raster");
    assert!(prepared.availability.layers[0].reason.as_deref().is_some_and(|r| r.contains("缺失")));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn cog_tile_urls_are_checked_hidden_and_recovered() {
    let (root, home) = temp_home("cog-tile");
    let data = root.join("data");
    let file = data.join("tiny.tif");
    write(&file);
    let mut sources = SourceRegistry::open(home).expect("registry");
    let folder = sources.add_folder(data.to_str().expect("utf-8"), None).expect("folder");
    let url = format!("http://127.0.0.1:8765/api/files/{}/tiny.tif", folder.id);
    let mut project = Project::new("COG");
    project.map_mut().tiles.push(TileLayer::new("tiny.tif", TileKind::Cog, url));
    let text = serde_json::to_string(&project).expect("project json");

    let prepared = availability::prepare(&text, &sources).expect("prepare available");
    assert_eq!(prepared.availability.layers.len(), 1);
    assert_eq!(prepared.availability.layers[0].kind, "cog");
    assert_eq!(prepared.availability.layers[0].status, "available");
    assert_eq!(prepared.availability.layers[0].tile_index, Some(0));
    assert!(tile_visible(&prepared.content));

    std::fs::remove_file(&file).expect("delete source");
    let prepared = availability::prepare(&text, &sources).expect("prepare missing");
    assert_eq!(prepared.availability.unavailable, 1);
    assert_eq!(prepared.availability.layers[0].status, "unavailable");
    assert_eq!(prepared.availability.layers[0].reason.as_deref(), Some("文件缺失"));
    assert!(!tile_visible(&prepared.content), "an unavailable COG is hidden in the derived copy");
    assert_eq!(text, serde_json::to_string(&project).expect("json"), "preparing does not rewrite the stored project");

    write(&file);
    let prepared = availability::prepare(&text, &sources).expect("prepare recovered");
    assert_eq!(prepared.availability.unavailable, 0);
    assert_eq!(prepared.availability.layers[0].status, "available");
    assert!(tile_visible(&prepared.content));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn prepared_markers_do_not_make_a_project_unreadable() {
    let (root, home) = temp_home("marker");
    let data = root.join("data");
    let file = data.join("gone.geojson");
    std::fs::create_dir_all(&data).expect("data");
    let mut sources = SourceRegistry::open(home).expect("registry");
    sources.add_folder(data.to_str().expect("utf-8"), None).expect("folder");
    let text = source_project(&file, true);
    let prepared = availability::prepare(&text, &sources).expect("prepare");
    let project: Project = serde_json::from_str(&prepared.content).expect("prepared project remains loadable");
    assert_eq!(project.maps[0].layers.len(), 1);
    assert!(!project.maps[0].layers[0].visible);
    std::fs::remove_dir_all(root).ok();
}
