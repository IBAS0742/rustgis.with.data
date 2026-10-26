//! A small, self-hosted RustGIS preview backend.
//!
//! The crate deliberately keeps policy out of the UI: project versions are immutable files, source
//! roots and database connections are persisted separately, and every path that comes from a
//! browser is resolved through a configured root. The pure modules here are tested without an HTTP
//! server so the same rules apply to the web app and any future client.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod availability;
pub mod databases;
pub mod error;
pub mod home;
pub mod import;
pub mod pairing;
pub mod ids;
pub mod project_store;
pub mod server;
pub mod sources;

pub use availability::{AvailabilityReport, LayerAvailability, PreparedProject, prepare};
pub use databases::{DatabaseCatalog, DatabaseConnection, DatabaseRegistry, DatabaseView, NewDatabase, RelationCatalog, SchemaCatalog};
pub use error::StoreError;
pub use home::DataHome;
pub use project_store::{ProjectContent, ProjectMeta, ProjectStore, VersionMeta, WorkingCopy};
pub use sources::{FileEntry, FilePage, FolderSource, ReferenceCheck, ReferenceState, ResolvedFile, ScanSummary, SourceRegistry};
