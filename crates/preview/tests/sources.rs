//! Phase 3 acceptance at the source-registry boundary.

use std::path::{Path, PathBuf};

use rustgis_preview::{DataHome, ReferenceState, SourceRegistry, StoreError};

fn temp_home(name: &str) -> (PathBuf, DataHome) {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("rustgis-preview-src-{name}-{}-{nanos}", std::process::id()));
    let home = DataHome::open(&root).expect("data home");
    (root, home)
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, text).expect("write fixture");
}

#[test]
fn folders_persist_and_rescan_sees_added_and_deleted_files() {
    let (root, home) = temp_home("persist");
    let data = root.join("data");
    write(&data.join("one.geojson"), "{}");
    write(&data.join("two.tif"), "x");
    write(&data.join("notes.md"), "not data");
    write(&data.join("nested/three.csv"), "x,y\n1,2\n");
    std::fs::create_dir_all(root.join("other")).expect("other");

    let mut registry = SourceRegistry::open(home.clone()).expect("open registry");
    let source = registry.add_folder(data.to_str().expect("utf-8 path"), Some("Data")).expect("add folder");
    let other = registry.add_folder(root.join("other").to_str().expect("utf-8 path"), None).expect("add second folder");
    assert_eq!(registry.folders().len(), 2);
    assert!(!other.display_name.is_empty());

    let page = registry.browse(&source.id, "", 0, 100).expect("browse root");
    assert_eq!(page.total, 3, "directories plus supported files, not notes.md");
    let names: Vec<_> = page.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["nested", "one.geojson", "two.tif"]);
    assert_eq!(page.entries[0].kind, "directory");

    write(&data.join("four.fgb"), "x");
    let page = registry.browse(&source.id, "", 0, 100).expect("browse after add");
    assert_eq!(page.total, 4);
    assert!(page.entries.iter().any(|e| e.name == "four.fgb"));
    std::fs::remove_file(data.join("one.geojson")).expect("delete file");
    let page = registry.browse(&source.id, "", 0, 100).expect("browse after delete");
    assert!(!page.entries.iter().any(|e| e.name == "one.geojson"));

    let scan = registry.rescan(&source.id).expect("rescan");
    assert_eq!(scan.error, None);
    assert_eq!(scan.files, 3);
    assert!(scan.directories >= 1);
    assert_eq!(scan.source.scan_status, "ready");

    drop(registry);
    let registry = SourceRegistry::open(home).expect("reopen registry");
    assert_eq!(registry.folders().len(), 2);
    assert_eq!(registry.folder(&source.id).expect("source persists").display_name, "Data");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn duplicate_roots_are_rejected() {
    let (root, home) = temp_home("duplicate");
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data");
    let mut registry = SourceRegistry::open(home).expect("open");
    registry.add_folder(data.to_str().expect("path"), None).expect("first");
    let err = registry.add_folder(data.to_str().expect("path"), None).expect_err("duplicate");
    assert!(matches!(err, StoreError::InvalidInput(_)));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn traversal_and_absolute_paths_are_rejected() {
    let (root, home) = temp_home("traversal");
    let data = root.join("data");
    write(&data.join("ok.geojson"), "{}");
    write(&root.join("outside/secret.geojson"), "{}");
    let mut registry = SourceRegistry::open(home).expect("open");
    let source = registry.add_folder(data.to_str().expect("path"), None).expect("add");

    for bad in [
        "../outside/secret.geojson",
        "..\\outside\\secret.geojson",
        "/etc/passwd",
        "C:\\Windows\\win.ini",
        "a/../../outside/secret.geojson",
    ] {
        let err = registry.resolve_file(&source.id, bad).expect_err("path must be rejected");
        assert!(matches!(err, StoreError::PathNotAllowed(_) | StoreError::FileNotFound(_)), "{bad}: {err:?}");
    }
    let resolved = registry.resolve_file(&source.id, "ok.geojson").expect("safe file");
    assert!(resolved.path.ends_with("ok.geojson"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pagination_is_bounded_and_does_not_expose_unsupported_files() {
    let (root, home) = temp_home("paging");
    let data = root.join("data");
    for i in 0..15 {
        write(&data.join(format!("{i:02}.geojson")), "{}");
    }
    write(&data.join("not-data.md"), "x");
    let mut registry = SourceRegistry::open(home).expect("open");
    let source = registry.add_folder(data.to_str().expect("path"), None).expect("add");

    let first = registry.browse(&source.id, "", 0, 5).expect("first page");
    assert_eq!(first.total, 15);
    assert_eq!(first.entries.len(), 5);
    assert_eq!(first.entries[0].name, "00.geojson");
    let last = registry.browse(&source.id, "", 2, 5).expect("last page");
    assert_eq!(last.entries.len(), 5);
    assert_eq!(last.entries[4].name, "14.geojson");
    let beyond = registry.browse(&source.id, "", 10, 5).expect("beyond");
    assert!(beyond.entries.is_empty());
    assert!(!first.entries.iter().any(|e| e.name == "not-data.md"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn disabled_sources_and_missing_roots_keep_their_records() {
    let (root, home) = temp_home("missing");
    let data = root.join("data");
    write(&data.join("one.geojson"), "{}");
    let mut registry = SourceRegistry::open(home.clone()).expect("open");
    let source = registry.add_folder(data.to_str().expect("path"), None).expect("add");
    registry.set_enabled(&source.id, false).expect("disable");
    let err = registry.browse(&source.id, "", 0, 20).expect_err("disabled");
    assert!(matches!(err, StoreError::SourceUnavailable(_)));
    registry.set_enabled(&source.id, true).expect("enable");

    std::fs::remove_dir_all(&data).expect("remove root");
    let err = registry.browse(&source.id, "", 0, 20).expect_err("missing root");
    assert!(matches!(err, StoreError::SourceUnavailable(_)));
    let scan = registry.rescan(&source.id).expect("rescan reports error without removing record");
    assert!(scan.error.is_some());
    assert_eq!(scan.source.scan_status, "unavailable");
    assert!(registry.folder(&source.id).is_ok());
    drop(registry);
    let registry = SourceRegistry::open(home).expect("reopen");
    assert!(registry.folder(&source.id).is_ok());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn file_api_urls_are_checked_inside_sources() {
    let (root, home) = temp_home("file-url");
    let data = root.join("data");
    write(&data.join("places.geojson"), "{}");
    write(&data.join("two words.geojson"), "{}");
    let mut registry = SourceRegistry::open(home).expect("open");
    let source = registry.add_folder(data.to_str().expect("path"), None).expect("add");
    let base = format!("http://127.0.0.1:8765/api/files/{}/", source.id);

    let check = registry.check_file_url(&format!("{base}places.geojson?cache=1")).expect("file API URL");
    assert_eq!(check.state, ReferenceState::Available);
    assert_eq!(check.source_id.as_deref(), Some(source.id.as_str()));

    let check = registry.check_file_url(&format!("{base}two%20words.geojson")).expect("file API URL");
    assert_eq!(check.state, ReferenceState::Available);

    let check = registry.check_file_url(&format!("{base}missing.geojson")).expect("file API URL");
    assert_eq!(check.state, ReferenceState::Missing);

    let check = registry.check_file_url(&format!("{base}%2e%2e%2foutside%2fsecret.geojson")).expect("file API URL");
    assert_eq!(check.state, ReferenceState::Outside);

    registry.set_enabled(&source.id, false).expect("disable");
    let check = registry.check_file_url(&format!("{base}places.geojson")).expect("file API URL");
    assert_eq!(check.state, ReferenceState::Disabled);

    assert!(registry.check_file_url("https://example.test/tiles/{z}/{x}/{y}.png").is_none());
    std::fs::remove_dir_all(root).ok();
}

#[cfg(unix)]
#[test]
fn symlinks_that_escape_the_root_are_not_served() {
    let (root, home) = temp_home("symlink");
    let data = root.join("data");
    write(&data.join("inside.geojson"), "{}");
    let outside = root.join("outside");
    write(&outside.join("secret.geojson"), "{}");
    std::os::unix::fs::symlink(outside.join("secret.geojson"), data.join("escape.geojson")).expect("symlink");
    let mut registry = SourceRegistry::open(home).expect("open");
    let source = registry.add_folder(data.to_str().expect("path"), None).expect("add");
    let page = registry.browse(&source.id, "", 0, 20).expect("browse");
    assert!(!page.entries.iter().any(|e| e.name == "escape.geojson"), "symlink entries are not followed");
    let err = registry.resolve_file(&source.id, "escape.geojson").expect_err("escape rejected");
    assert!(matches!(err, StoreError::PathNotAllowed(_)));
    registry.resolve_file(&source.id, "inside.geojson").expect("inside file is served");
    std::fs::remove_dir_all(root).ok();
}
