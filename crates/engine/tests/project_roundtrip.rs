//! Phase 1 experiments: what a `.rgis` file really keeps, and how opening a project treats
//! sources that are no longer there.
//!
//! These tests describe current upstream behaviour. They are deliberately kept at the format
//! boundary (save → JSON → load) so a later versioned-project store can reuse the same checks.

use std::path::PathBuf;

use rustgis_data::raster::{ColorHint, Crs, GeoTransform, Raster};
use rustgis_data::{Dataset, Field, FieldType, Geometry, GeometryType, Value};
use rustgis_engine::Project;
use rustgis_engine::map::RasterLayer;
use rustgis_engine::map::source::SourceLayer;
use rustgis_geom::Coord;

fn temp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("rustgis-phase1-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temporary test directory");
    dir
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
    .expect("create tiny raster")
}

#[test]
fn project_roundtrip_keeps_layers_style_view_and_source_reference() {
    let dir = temp_dir("roundtrip");
    let path = dir.join("roundtrip.rgis");

    let mut project = Project::new("Roundtrip");
    project.map_mut().name = "Main".into();
    project.map_mut().camera.center = Coord::new(12.5, -4.25);
    project.map_mut().camera.res = 1234.5;
    project.map_mut().camera.bearing = 17.0;
    project.map_mut().camera.pitch = 23.0;
    project.folders.push("C:/data".into());

    let mut dataset = Dataset::new("places", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
    dataset.source = Some("C:/data/places.geojson".into());
    dataset.push(Some(Geometry::Point(Coord::new(1.5, 2.5))), vec![Value::Text("one".into())]);
    let id = project.map_mut().add_dataset(dataset);
    let layer = project.map_mut().layer_mut(id).expect("added layer");
    layer.visible = false;
    layer.transparency = 37.0;
    layer.labels.default.field = "name".into();
    layer.labels.default.size = 11.5;

    project.save(&path).expect("save project");
    let json = std::fs::read_to_string(&path).expect("read saved project");
    assert!(json.contains("\"features\""), "small vector features are embedded in the project file");
    assert!(json.contains("C:/data/places.geojson"), "the source reference is kept");

    let loaded = Project::load(&path).expect("load project");
    assert_eq!(loaded.name, "Roundtrip");
    assert_eq!(loaded.folders, ["C:/data"]);
    let map = loaded.map();
    assert_eq!(map.name, "Main");
    assert_eq!((map.camera.center.x, map.camera.center.y), (12.5, -4.25));
    assert_eq!(map.camera.res, 1234.5);
    assert_eq!((map.camera.bearing, map.camera.pitch), (17.0, 23.0));
    let layer = map.layer(id).expect("layer survives the round trip");
    assert_eq!(layer.name, "places");
    assert!(!layer.visible);
    assert_eq!(layer.transparency, 37.0);
    assert_eq!(layer.labels.default.field, "name");
    assert_eq!(layer.labels.default.size, 11.5);
    assert_eq!(layer.dataset.source.as_deref(), Some("C:/data/places.geojson"));
    assert_eq!(layer.dataset.features.len(), 1);
    assert_eq!(layer.dataset.value(0, 0), &Value::Text("one".into()));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn windowed_sources_do_not_embed_their_features_but_keep_the_reference() {
    let dir = temp_dir("windowed");
    let path = dir.join("windowed.rgis");

    let mut project = Project::new("Windowed");
    let mut source = SourceLayer::default();
    source.path = Some(dir.join("missing.fgb").display().to_string());
    project.map_mut().add_streamed("streamed", source);
    project.save(&path).expect("save project");

    let json = std::fs::read_to_string(&path).expect("read saved project");
    let value: serde_json::Value = serde_json::from_str(&json).expect("saved project is JSON");
    let layer = &value["maps"][0]["layers"][0];
    assert_eq!(layer["source"]["path"], serde_json::json!(dir.join("missing.fgb").display().to_string()));
    assert_eq!(layer["dataset"]["features"], serde_json::json!([]), "streamed features are not copied into the project");

    let loaded = Project::load(&path).expect("load project");
    let layer = loaded.map().layer_by_name("streamed").expect("streamed layer is retained");
    assert!(layer.source.as_ref().is_some_and(|s| s.path.as_deref() == Some(dir.join("missing.fgb").to_string_lossy().as_ref())));
    assert!(layer.source.as_ref().and_then(|s| s.error.as_deref()).is_some(), "missing source is reported");
    assert!(layer.dataset.features.is_empty());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn missing_raster_is_unavailable_but_its_layer_is_retained() {
    let dir = temp_dir("raster");
    let path = dir.join("raster.rgis");
    let missing = dir.join("missing.tif");

    let mut project = Project::new("Raster");
    let id = project.map_mut().add_raster("missing", RasterLayer::from_path(tiny_raster(), missing.display().to_string()));
    project.save(&path).expect("save project");
    assert!(path.is_file());

    let loaded = Project::load(&path).expect("load project");
    let layer = loaded.map().layer(id).expect("raster layer is retained");
    let raster = layer.raster.as_ref().expect("raster record is retained");
    assert_eq!(raster.path.as_deref(), Some(missing.to_string_lossy().as_ref()));
    assert!(raster.data.is_none(), "pixels are cleared when the path cannot be read");
    assert!(layer.dataset.features.is_empty());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn saving_a_project_replaces_the_existing_file() {
    // This is a Phase 1 fact, not a product behaviour: today's `Project::save` has no version
    // identifier and writes over an existing file. Phase 2 must add publication above it.
    let dir = temp_dir("overwrite");
    let path = dir.join("overwrite.rgis");

    let first = Project::new("First");
    first.save(&path).expect("save first project");
    let second = Project::new("Second");
    second.save(&path).expect("save second project");

    let loaded = Project::load(&path).expect("load replaced project");
    assert_eq!(loaded.name, "Second");

    std::fs::remove_dir_all(&dir).ok();
}
