//! Phase 5 non-secret persistence and password-required behavior. A live PostGIS server is not
//! required for these checks; catalog queries themselves need external infrastructure.

use std::path::PathBuf;

use rustgis_preview::{DataHome, DatabaseRegistry, NewDatabase, StoreError};

fn temp_home(name: &str) -> (PathBuf, DataHome) {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("rustgis-preview-db-{name}-{}-{nanos}", std::process::id()));
    let home = DataHome::open(&root).expect("data home");
    (root, home)
}

fn add(registry: &mut DatabaseRegistry, password: Option<&str>) -> rustgis_preview::DatabaseView {
    registry
        .add(NewDatabase {
            display_name: Some("Local GIS".into()),
            host: "127.0.0.1".into(),
            port: 5432,
            username: "gis".into(),
            password: password.map(str::to_owned),
            ssl_mode: Some("prefer".into()),
            database: Some("gis".into()),
        })
        .expect("add connection")
}

#[test]
fn password_is_only_in_memory_and_never_written_to_config() {
    let (root, home) = temp_home("secret");
    let mut registry = DatabaseRegistry::open(home.clone()).expect("open");
    let view = add(&mut registry, Some("correct horse battery staple"));
    assert!(!view.password_required);
    let config = std::fs::read_to_string(home.config().join("databases.json")).expect("config file");
    assert!(!config.contains("correct horse battery staple"), "password must never be persisted");
    assert!(!config.contains("password\":\""), "only the password_required flag may be written");
    assert!(config.contains("127.0.0.1"));
    assert!(config.contains("\"password_required\": true"));

    let json = serde_json::to_string(&registry.views()).expect("serialize views");
    assert!(!json.contains("correct horse battery staple"));
    assert!(!json.contains("\"password\""));

    drop(registry);
    let registry = DatabaseRegistry::open(home).expect("reopen");
    let after = registry.views();
    assert_eq!(after.len(), 1);
    assert!(after[0].password_required, "a restart requires the password again");
    let err = registry.databases(&after[0].id).expect_err("no in-memory password after restart");
    assert!(matches!(err, StoreError::PasswordRequired(_)));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn supplying_a_password_clears_the_required_flag_without_changing_the_model() {
    let (root, home) = temp_home("supply");
    let mut registry = DatabaseRegistry::open(home.clone()).expect("open");
    let view = add(&mut registry, Some("first"));
    drop(registry);
    let mut registry = DatabaseRegistry::open(home.clone()).expect("reopen");
    assert!(registry.views()[0].password_required);
    let updated = registry.set_password(&view.id, "second").expect("set password");
    assert!(!updated.password_required);
    let config = std::fs::read_to_string(home.config().join("databases.json")).expect("config");
    assert!(!config.contains("second"));
    assert!(!registry.views()[0].password_required);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn connections_survive_restart_and_can_be_removed() {
    let (root, home) = temp_home("persist");
    let mut registry = DatabaseRegistry::open(home.clone()).expect("open");
    let first = add(&mut registry, None);
    let second = registry
        .add(NewDatabase {
            display_name: None,
            host: "db.example.test".into(),
            port: 5433,
            username: "reader".into(),
            password: None,
            ssl_mode: Some("require".into()),
            database: None,
        })
        .expect("second connection");
    assert_ne!(first.id, second.id);
    drop(registry);

    let mut registry = DatabaseRegistry::open(home).expect("reopen");
    assert_eq!(registry.views().len(), 2);
    registry.remove(&first.id).expect("remove");
    assert_eq!(registry.views().len(), 1);
    assert_eq!(registry.views()[0].display_name, "reader@db.example.test");
    assert!(matches!(registry.remove(&first.id).expect_err("gone"), StoreError::SourceNotFound(_)));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn invalid_connection_settings_are_rejected() {
    let (root, home) = temp_home("invalid");
    let mut registry = DatabaseRegistry::open(home).expect("open");
    for (host, port, user, ssl) in [
        ("", 5432, "gis", "prefer"),
        ("localhost\n", 5432, "gis", "prefer"),
        ("localhost", 0, "gis", "prefer"),
        ("localhost", 5432, "", "prefer"),
        ("localhost", 5432, "gis", "sometimes"),
    ] {
        let err = registry
            .add(NewDatabase {
                display_name: None,
                host: host.into(),
                port,
                username: user.into(),
                password: None,
                ssl_mode: Some(ssl.into()),
                database: None,
            })
            .expect_err("invalid settings");
        assert!(matches!(err, StoreError::InvalidInput(_)), "{err:?}");
    }
    assert!(registry.views().is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn path_like_connection_ids_are_rejected_without_a_lookup() {
    let (root, home) = temp_home("ids");
    let registry = DatabaseRegistry::open(home).expect("open");
    for bad in ["..", "../x", "a/b", "C:\\x"] {
        let err = registry.view_for(bad).expect_err("path-like id");
        assert!(matches!(err, StoreError::InvalidInput(_)));
    }
    std::fs::remove_dir_all(root).ok();
}
