//! Phase 2 acceptance at the storage boundary: immutable versions, mutable working copies,
//! restart recovery, and conflict/no-clobber behaviour.

use std::path::PathBuf;

use rustgis_preview::{DataHome, ProjectStore, StoreError};

fn temp_home(name: &str) -> (PathBuf, DataHome) {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("rustgis-preview-{name}-{}-{nanos}", std::process::id()));
    let home = DataHome::open(&root).expect("open temporary data home");
    (root, home)
}

fn changed_name(text: &str, name: &str) -> String {
    let mut value: serde_json::Value = serde_json::from_str(text).expect("project is JSON");
    value["name"] = serde_json::Value::String(name.to_owned());
    serde_json::to_string(&value).expect("serialize changed project")
}

#[test]
fn first_version_survives_restart() {
    let (root, home) = temp_home("restart");
    let mut store = ProjectStore::open(home.clone()).expect("open store");
    let created = store.create_project("Empty", None).expect("create project");
    assert!(created.read_only);
    assert_eq!(created.project.versions.len(), 1);
    let project_id = created.project.id.clone();
    let version_id = created.version.id.clone();
    let first_text = created.content.clone();
    assert!(first_text.contains("\"name\":\"Empty\""), "empty project content is generated");

    drop(store);
    let store = ProjectStore::open(home).expect("reopen store");
    let projects = store.projects();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id, project_id);
    let versions = store.versions(&project_id).expect("list versions");
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].id, version_id);
    let reopened = store.read_version(&project_id, &version_id).expect("read version");
    assert_eq!(reopened.content, first_text);
    assert!(reopened.read_only);
    assert_eq!(reopened.project.name, "Empty");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn working_copy_saves_a_new_version_without_changing_its_parent() {
    let (root, home) = temp_home("copy");
    let mut store = ProjectStore::open(home.clone()).expect("open store");
    let created = store.create_project("Original", None).expect("create project");
    let project_id = created.project.id.clone();
    let v1 = created.version.id.clone();
    let v1_text = created.content.clone();

    let working = store.create_working_copy(&project_id, &v1).expect("copy version");
    assert!(!working.read_only);
    assert_eq!(working.base_version.as_deref(), Some(v1.as_str()));

    let changed = changed_name(&working.content, "Edited");
    let v2 = store.save_working_copy(&project_id, &working.id, &changed).expect("save working copy");
    assert_ne!(v2.id, v1);
    assert_eq!(v2.parent.as_deref(), Some(v1.as_str()));
    assert!(v2.sha256.as_deref().is_some_and(|h| h.len() == 64));

    let old = store.read_version(&project_id, &v1).expect("read parent");
    assert_eq!(old.content, v1_text, "saving a copy must not change V1");
    assert!(old.content.contains("\"name\":\"Original\""));
    let new = store.read_version(&project_id, &v2.id).expect("read child");
    assert!(new.content.contains("\"name\":\"Edited\""));
    let versions = store.versions(&project_id).expect("list versions");
    assert_eq!(versions.len(), 2);

    drop(store);
    let store = ProjectStore::open(home).expect("reopen store");
    let reopened = store.read_version(&project_id, &v2.id).expect("read child after restart");
    assert!(reopened.content.contains("\"name\":\"Edited\""));
    let working = store.read_working_copy(&project_id, &working.id).expect("working copy survives restart");
    assert!(working.content.contains("\"name\":\"Edited\""));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn publishing_an_existing_version_id_is_refused_and_does_not_replace_it() {
    let (root, home) = temp_home("conflict");
    let mut store = ProjectStore::open(home).expect("open store");
    let created = store.create_project("Conflict", None).expect("create project");
    let project_id = created.project.id.clone();
    let v1 = created.version.id.clone();

    let second_text = changed_name(&created.content, "Other");
    let err = store.publish_version_with_id(&project_id, &v1, &second_text, None).expect_err("existing version must not be replaceable");
    assert!(matches!(err, StoreError::VersionExists(ref id) if id == &v1));
    let still = store.read_version(&project_id, &v1).expect("read original");
    assert!(still.content.contains("\"name\":\"Conflict\""));
    assert_eq!(store.versions(&project_id).expect("versions").len(), 1);

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn interrupted_temporary_files_are_not_listed_and_are_cleaned() {
    let (root, home) = temp_home("interrupted");
    let mut store = ProjectStore::open(home.clone()).expect("open store");
    let created = store.create_project("Interrupted", None).expect("create project");
    let project_id = created.project.id.clone();
    drop(store);

    let versions = home.projects().join(&project_id).join("versions");
    std::fs::write(versions.join(".tmp-interrupted"), b"{\"half\":").expect("write temp");
    std::fs::write(versions.join("not-a-version.partial"), b"junk").expect("write partial");
    let store = ProjectStore::open(home.clone()).expect("reopen store");
    let versions = store.versions(&project_id).expect("list versions");
    assert_eq!(versions.len(), 1);
    assert!(!versions.iter().any(|v| v.id.contains("tmp") || v.id.contains("partial")));
    assert!(!home.projects().join(&project_id).join("versions/.tmp-interrupted").exists());
    assert!(home.projects().join(&project_id).join("versions/not-a-version.partial").exists());

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn publishing_leaves_no_temporary_links() {
    let (root, home) = temp_home("temp-links");
    let mut store = ProjectStore::open(home.clone()).expect("open store");
    let created = store.create_project("Clean", None).expect("create project");
    let project_id = created.project.id.clone();
    let working = store.create_working_copy(&project_id, &created.version.id).expect("copy version");
    let changed = changed_name(&working.content, "Clean edit");
    store.save_working_copy(&project_id, &working.id, &changed).expect("save working copy");

    let versions = home.projects().join(&project_id).join("versions");
    let leftovers: Vec<_> = std::fs::read_dir(versions)
        .expect("versions directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "temporary hard links were left behind: {leftovers:?}");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn invalid_project_text_is_rejected_before_a_version_is_published() {
    let (root, home) = temp_home("invalid");
    let mut store = ProjectStore::open(home.clone()).expect("open store");
    let err = store.create_project("Invalid", Some("{\"not\":\"a project\"}".into())).expect_err("invalid project");
    assert!(matches!(err, StoreError::InvalidProject(_)));
    assert!(store.projects().is_empty());
    assert!(std::fs::read_dir(home.projects()).expect("projects directory").next().is_none());

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn ids_that_are_paths_are_rejected() {
    let (root, home) = temp_home("traversal");
    let store = ProjectStore::open(home).expect("open store");
    for bad in ["..", "../x", "a/b", "C:\\data"] {
        let err = store.read_version(bad, "v").expect_err("path-like project id");
        assert!(matches!(err, StoreError::InvalidInput(_)));
    }
    std::fs::remove_dir_all(root).ok();
}
