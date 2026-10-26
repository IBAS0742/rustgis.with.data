//! Phase B acceptance: pairing an imported layer binds its data in the working copy only,
//! and immutable versions are never rewritten.

use std::path::PathBuf;

use rustgis_preview::pairing::{PairFile, PairRequest};
use rustgis_preview::project_store::{ImportLayerMeta, ImportMeta};
use rustgis_preview::{import, sources::SourceRegistry, DatabaseRegistry, DataHome, ProjectStore};

fn temp_home(name: &str) -> (PathBuf, DataHome) {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("rustgis-preview-pair-{name}-{}-{nanos}", std::process::id()));
    let home = DataHome::open(&root).expect("data home");
    (root, home)
}

const QGS: &str = r#"<qgis projectname="配对测试">
  <projectlayers>
    <maplayer type="vector" visible="1"><id>r</id><layername>道路</layername><provider>ogr</provider>
      <datasource>./roads.geojson</datasource></maplayer>
  </projectlayers>
</qgis>"#;

const GEOJSON: &str = r#"{"type":"FeatureCollection","features":[
  {"type":"Feature","properties":{"name":"r1"},"geometry":{"type":"LineString","coordinates":[[0,0],[1,1]]}}]}"#;

#[test]
fn pairing_a_file_layer_updates_the_working_copy_and_metadata() {
    let (root, home) = temp_home("file");
    std::fs::create_dir_all(root.join("data")).expect("dir");
    let mut sources = SourceRegistry::open(home.clone()).expect("registry");
    sources.add_folder(root.join("data").to_str().expect("utf8"), None).expect("folder");
    let databases = DatabaseRegistry::open(home.clone()).expect("databases");

    let plan = import::parse_project(QGS.as_bytes()).expect("parse");
    // The data file is not there yet: the layer imports unpaired.
    let matches: Vec<_> = plan.layers.iter().map(|l| import::match_layer(l, &sources, &[])).collect();
    assert!(matches!(matches[0], import::MatchStatus::Unpaired { .. }), "before pairing the layer waits");
    let text = import::build_project(&plan, &matches).expect("build");

    let mut store = ProjectStore::open(home.clone()).expect("store");
    let created = store.create_project("配对工程", None, Some(text)).expect("create");
    store
        .set_import_meta(
            &created.project.id,
            ImportMeta {
                app: "qgis".into(),
                file_name: "t.qgs".into(),
                imported_at_ms: 0,
                layers: vec![ImportLayerMeta {
                    name: "道路".into(),
                    kind: "文件".into(),
                    status: "unpaired".into(),
                    qgis_datasource: "./roads.geojson".into(),
                    target: None,
                    style_note: "完整".into(),
                }],
            },
        )
        .expect("import meta");
    let working = store.create_working_copy(&created.project.id, &created.version.id, None).expect("copy");

    // The operator now prepares the data and pairs the layer to it.
    std::fs::write(root.join("data/roads.geojson"), GEOJSON).expect("geojson");
    let request = PairRequest { layer_name: "道路".into(), file: Some(PairFile { path: "roads.geojson".into() }), postgis: None };
    let meta = rustgis_preview::pairing::pair_layer(&mut store, &sources, &databases, &created.project.id, &working.id, &request).expect("pair");

    let entry = &meta.import.as_ref().expect("import meta").layers[0];
    assert_eq!(entry.status, "paired");
    assert!(entry.target.as_deref().is_some_and(|t| t.ends_with("roads.geojson")));

    let copy = store.read_working_copy(&created.project.id, &working.id).expect("working");
    assert!(copy.content.contains("roads.geojson"));
    // The immutable V1 keeps the original unpaired reference.
    let version = store.read_version(&created.project.id, &created.version.id).expect("version");
    assert!(version.content.contains("./roads.geojson"), "the saved version is untouched by pairing");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pairing_refuses_files_outside_the_configured_roots() {
    let (root, home) = temp_home("outside");
    std::fs::create_dir_all(root.join("elsewhere")).expect("dir");
    std::fs::write(root.join("elsewhere/secret.geojson"), GEOJSON).expect("geojson");
    let mut sources = SourceRegistry::open(home.clone()).expect("registry");
    sources.add_folder(root.join("elsewhere/..").join("data").to_str().expect("utf8"), None).ok();
    // No folder actually configured to hold the file: the registry has a root at data/ which
    // does not exist here, so nothing resolves.
    let databases = DatabaseRegistry::open(home.clone()).expect("databases");

    let plan = import::parse_project(QGS.as_bytes()).expect("parse");
    let text = import::build_project(&plan, &[]).expect("build");
    let mut store = ProjectStore::open(home).expect("store");
    let created = store.create_project("越界", None, Some(text)).expect("create");
    let working = store.create_working_copy(&created.project.id, &created.version.id, None).expect("copy");

    let request = PairRequest {
        layer_name: "道路".into(),
        file: Some(PairFile { path: root.join("elsewhere/secret.geojson").to_string_lossy().into_owned() }),
        postgis: None,
    };
    let err = rustgis_preview::pairing::pair_layer(&mut store, &sources, &databases, &created.project.id, &working.id, &request)
        .expect_err("outside every root");
    assert!(matches!(err, rustgis_preview::StoreError::InvalidInput(_)));
    std::fs::remove_dir_all(root).ok();
}
