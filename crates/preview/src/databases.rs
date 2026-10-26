//! PostgreSQL/PostGIS connection configuration and catalog browsing.
//!
//! By explicit product decision (2026-10-25) a password entered when the connection is saved is
//! persisted in `databases.json` and restored on start. Connections saved without a password keep
//! the old behaviour: the secret is held in process memory only and the console asks again after
//! every page refresh. Passwords are tested against the server before they are accepted and are
//! never included in API views, logs or URLs.

use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rustgis_data::source::postgis;
use serde::{Deserialize, Serialize};

use crate::StoreError;
use crate::home::DataHome;
use crate::ids;

const FORMAT: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 4 << 20;

/// Persisted connection settings. By explicit product decision the password given at save
/// time is persisted here too; it is still never returned by any API view, log or URL.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatabaseConnection {
    pub id: String,
    pub display_name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default = "default_ssl")]
    pub ssl_mode: String,
    /// A default database for the connection; individual catalog requests can select another.
    #[serde(default)]
    pub database: Option<String>,
    /// The secret saved when the connection was created. `None` means the operator chose not
    /// to store one; the console then asks for it again on every page load (session-only).
    #[serde(default)]
    pub password: Option<String>,
    /// Legacy flag from the pre-persistence format: this connection authenticated with a
    /// password that was held in memory only.
    #[serde(default)]
    pub password_required: bool,
    pub created_at_ms: u64,
}

/// What the API may return: connection settings plus whether a password is needed right now.
/// The secret itself is never part of any view.
#[derive(Clone, Debug, Serialize)]
pub struct DatabaseView {
    pub id: String,
    pub display_name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub ssl_mode: String,
    pub database: Option<String>,
    pub password_required: bool,
    pub created_at_ms: u64,
}

/// Input for adding a connection. Deliberately not `Debug`: the password must not be logged.
pub struct NewDatabase {
    pub display_name: Option<String>,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub ssl_mode: Option<String>,
    pub database: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GeometryColumnCatalog {
    pub name: String,
    pub srid: i32,
    pub kind: String,
    pub geography: bool,
    pub several: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RelationCatalog {
    pub schema: String,
    pub name: String,
    pub kind: String,
    pub geometry_columns: Vec<GeometryColumnCatalog>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SchemaCatalog {
    pub name: String,
    pub relations: Vec<RelationCatalog>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DatabaseCatalog {
    pub connection_id: String,
    pub database: String,
    pub postgis_available: bool,
    pub schemas: Vec<SchemaCatalog>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default = "format")]
    format: u32,
    #[serde(default)]
    connections: Vec<DatabaseConnection>,
}

fn format() -> u32 {
    FORMAT
}

fn default_ssl() -> String {
    "prefer".to_owned()
}

pub struct DatabaseRegistry {
    home: DataHome,
    connections: Vec<DatabaseConnection>,
    passwords: HashMap<String, String>,
}

impl DatabaseRegistry {
    pub fn open(home: DataHome) -> Result<Self, StoreError> {
        let path = home.config().join("databases.json");
        let connections = match fs::metadata(&path) {
            Ok(meta) if meta.len() > MAX_CONFIG_BYTES => {
                return Err(StoreError::InvalidInput("databases.json is too large".into()));
            }
            Ok(_) => {
                let text = fs::read_to_string(&path)?;
                let file: ConfigFile = serde_json::from_str(&text).map_err(|e| StoreError::InvalidInput(format!("databases.json: {e}")))?;
                if file.format > FORMAT {
                    return Err(StoreError::InvalidInput(format!("databases.json format {} is newer than this server", file.format)));
                }
                file.connections.into_iter().filter(|c| ids::is_component(&c.id)).collect()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        // Persisted passwords become immediately usable again after a restart.
        let passwords = connections.iter().filter_map(|c| c.password.clone().map(|p| (c.id.clone(), p))).collect();
        Ok(DatabaseRegistry { home, connections, passwords })
    }

    pub fn views(&self) -> Vec<DatabaseView> {
        self.connections.iter().map(|c| self.view(c)).collect()
    }

    pub fn view_for(&self, id: &str) -> Result<DatabaseView, StoreError> {
        let c = self.connection(id)?;
        Ok(self.view(c))
    }

    pub fn add(&mut self, input: NewDatabase) -> Result<DatabaseView, StoreError> {
        let host = clean_field(&input.host, "host", 255)?;
        if input.port == 0 {
            return Err(StoreError::InvalidInput("port must be between 1 and 65535".into()));
        }
        let username = clean_field(&input.username, "username", 128)?;
        let database = input.database.as_deref().map(|d| clean_field(d, "database", 128)).transpose()?;
        let ssl_mode = input.ssl_mode.as_deref().unwrap_or("prefer").to_ascii_lowercase();
        if !matches!(ssl_mode.as_str(), "disable" | "prefer" | "require" | "verify-ca" | "verify-full") {
            return Err(StoreError::InvalidInput("ssl_mode must be disable, prefer, require, verify-ca or verify-full".into()));
        }
        let display_name =
            input.display_name.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).unwrap_or_else(|| match &database {
                Some(db) => format!("{db}@{host}"),
                None => format!("{username}@{host}"),
            });
        let display_name = clean_field(&display_name, "display name", 200)?;
        let password = input.password.filter(|p| !p.is_empty());
        // A password given at save time is recorded as-is (product decision). It is verified
        // the first time the connection is used; the operator can override it per session.
        let connection = DatabaseConnection {
            id: ids::new_id("db"),
            display_name,
            host,
            port: input.port,
            username,
            ssl_mode,
            database,
            password,
            password_required: false,
            created_at_ms: now_ms(),
        };
        if let Some(password) = connection.password.clone() {
            self.passwords.insert(connection.id.clone(), password);
        }
        self.connections.push(connection.clone());
        if let Err(e) = self.save() {
            self.connections.retain(|c| c.id != connection.id);
            self.passwords.remove(&connection.id);
            return Err(e);
        }
        Ok(self.view(&connection))
    }

    pub fn remove(&mut self, id: &str) -> Result<(), StoreError> {
        let index = self.connections.iter().position(|c| c.id == id).ok_or_else(|| StoreError::SourceNotFound(id.to_owned()))?;
        let removed = self.connections.remove(index);
        self.passwords.remove(&removed.id);
        self.save()
    }

    /// Supplies the password for a connection that was saved without one. The value is tested
    /// against the server first; on failure nothing is kept and the user may retry. It stays in
    /// memory for this process only, so a page refresh asks again, as designed.
    pub fn set_password(&mut self, id: &str, password: &str) -> Result<DatabaseView, StoreError> {
        let c = (*self.connection(id)?).clone();
        if password.is_empty() {
            return Err(StoreError::InvalidInput("password is empty".into()));
        }
        probe(&c.host, c.port, &c.username, Some(password), &c.ssl_mode, c.database.as_deref())?;
        self.passwords.insert(id.to_owned(), password.to_owned());
        Ok(self.view(&c))
    }

    /// Lists databases this login may connect to. The connection is opened to the configured
    /// default database (or the server's default when none was given).
    pub fn databases(&self, id: &str) -> Result<Vec<String>, StoreError> {
        let conn = self.connect(id, None)?;
        postgis::databases(&conn).map_err(|e| db_error(e.to_string()))
    }

    /// Opens one connection for the selected database, then lists each schema and relation. This
    /// deliberately does not assume that one PostgreSQL connection can see every database.
    pub fn catalog(&self, id: &str, database: &str) -> Result<DatabaseCatalog, StoreError> {
        let database = clean_field(database, "database", 128)?;
        let conn = self.connect(id, Some(&database))?;
        let postgis_available = postgis::has_postgis(&conn).map_err(|e| db_error(e.to_string()))?;
        let schemas = postgis::schemas(&conn).map_err(|e| db_error(e.to_string()))?;
        let mut out = Vec::with_capacity(schemas.len());
        for schema in schemas {
            let relations = postgis::relations(&conn, &schema).map_err(|e| db_error(e.to_string()))?;
            out.push(SchemaCatalog {
                name: schema,
                relations: relations
                    .into_iter()
                    .map(|r| RelationCatalog {
                        schema: r.schema,
                        name: r.name,
                        kind: r.kind,
                        geometry_columns: r
                            .geometry_columns
                            .into_iter()
                            .map(|c| GeometryColumnCatalog {
                                name: c.name,
                                srid: c.srid,
                                kind: c.kind,
                                geography: c.geography,
                                several: c.several,
                            })
                            .collect(),
                    })
                    .collect(),
            });
        }
        Ok(DatabaseCatalog { connection_id: id.to_owned(), database, postgis_available, schemas: out })
    }

    /// Reads a spatial table as bounded GeoJSON for the browser's normal remote-file path. This is
    /// a snapshot, not a live layer: the browser cannot open a PostgreSQL socket, and the preview
    /// deliberately does not expose a database proxy protocol. Re-adding the table refreshes it.
    pub fn geojson(&self, id: &str, database: &str, table: &str, limit: usize) -> Result<Vec<u8>, StoreError> {
        let database = clean_field(database, "database", 128)?;
        let table = clean_field(table, "table", 512)?;
        let conn = self.connect(id, Some(&database))?;
        postgis::geojson(&conn, &table, limit.clamp(1, 50_000)).map_err(|e| db_error(e.to_string()))
    }

    /// Connects to a selected database, or the connection's default when `database` is None.
    /// The session-supplied password wins over the persisted one. Returns only errors whose
    /// text is safe to show to the client (no DSN, no password).
    pub fn connect(&self, id: &str, database: Option<&str>) -> Result<Arc<postgis::Conn>, StoreError> {
        let c = self.connection(id)?;
        let session = self.passwords.get(id).cloned();
        let password = session.as_deref().or(c.password.as_deref());
        if c.password_required && c.password.is_none() && password.is_none() {
            return Err(StoreError::PasswordRequired(format!("connection {} needs its password again", c.display_name)));
        }
        let dsn = dsn_for(c, database)?;
        postgis::connect_with_password(&dsn, password).map_err(|e| db_error(e.to_string()))
    }

    fn connection(&self, id: &str) -> Result<&DatabaseConnection, StoreError> {
        if !ids::is_component(id) {
            return Err(StoreError::InvalidInput(format!("invalid database id: {id}")));
        }
        self.connections.iter().find(|c| c.id == id).ok_or_else(|| StoreError::SourceNotFound(id.to_owned()))
    }

    fn view(&self, c: &DatabaseConnection) -> DatabaseView {
        DatabaseView {
            id: c.id.clone(),
            display_name: c.display_name.clone(),
            host: c.host.clone(),
            port: c.port,
            username: c.username.clone(),
            ssl_mode: c.ssl_mode.clone(),
            database: c.database.clone(),
            // No persisted password and nothing supplied this session: the console asks again.
            password_required: c.password.is_none() && !self.passwords.contains_key(&c.id),
            created_at_ms: c.created_at_ms,
        }
    }

    fn save(&self) -> Result<(), StoreError> {
        let file = ConfigFile { format: FORMAT, connections: self.connections.clone() };
        let bytes = serde_json::to_vec_pretty(&file)?;
        rustgis_data::io::write_atomic(&self.home.config().join("databases.json"), &bytes)?;
        Ok(())
    }
}

fn dsn_for(c: &DatabaseConnection, database: Option<&str>) -> Result<String, StoreError> {
    let database = database.or(c.database.as_deref());
    let mut dsn =
        format!("host={} port={} user={} sslmode={}", libpq_value(&c.host), c.port, libpq_value(&c.username), libpq_value(&c.ssl_mode));
    if let Some(database) = database {
        let database = clean_field(database, "database", 128)?;
        dsn.push_str(&format!(" dbname={}", libpq_value(&database)));
    }
    Ok(dsn)
}

/// Verifies credentials by opening a real connection and issuing the cheapest useful query
/// (listing databases). Used before a password is accepted, so stored connections are known
/// to work. Errors are sanitised like every other database failure.
fn probe(
    host: &str,
    port: u16,
    username: &str,
    password: Option<&str>,
    ssl_mode: &str,
    database: Option<&str>,
) -> Result<(), StoreError> {
    let mut dsn = format!("host={} port={} user={} sslmode={}", libpq_value(host), port, libpq_value(username), libpq_value(ssl_mode));
    if let Some(database) = database {
        let database = clean_field(database, "database", 128)?;
        dsn.push_str(&format!(" dbname={}", libpq_value(&database)));
    }
    let conn = postgis::connect_with_password(&dsn, password).map_err(|e| db_error(e.to_string()))?;
    postgis::databases(&conn).map_err(|e| db_error(e.to_string())).map(|_| ())
}

/// Quotes a value for a libpq key/value connection string. No password ever enters this function.
fn libpq_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\\' || ch == '\'' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('\'');
    out
}

fn clean_field(value: &str, name: &str, max: usize) -> Result<String, StoreError> {
    if value.chars().any(char::is_control) || value.len() > max {
        return Err(StoreError::InvalidInput(format!("{name} is empty, too long or contains control characters")));
    }
    let value = value.trim();
    if value.is_empty() {
        return Err(StoreError::InvalidInput(format!("{name} is empty, too long or contains control characters")));
    }
    Ok(value.to_owned())
}

fn db_error(message: String) -> StoreError {
    // Postgres errors name the user/host/database, never the password. Strip URL-ish credentials
    // anyway, so a future DSN form cannot leak one.
    StoreError::DatabaseUnavailable(strip_url_credentials(&message))
}

fn strip_url_credentials(message: &str) -> String {
    let Some(scheme) = message.find("://") else {
        return message.to_owned();
    };
    let authority_start = scheme + 3;
    let rest = &message[authority_start..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let Some(at) = authority.rfind('@') else {
        return message.to_owned();
    };
    let userinfo = &authority[..at];
    let Some(colon) = userinfo.rfind(':') else {
        return message.to_owned();
    };
    let mut out = String::with_capacity(message.len());
    out.push_str(&message[..authority_start]);
    out.push_str(&userinfo[..colon]);
    out.push_str(":***");
    out.push_str(&rest[at..]);
    out
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(host: &str, database: Option<&str>) -> DatabaseConnection {
        DatabaseConnection {
            id: "db_test".into(),
            display_name: "test".into(),
            host: host.into(),
            port: 5432,
            username: "gis user".into(),
            ssl_mode: "require".into(),
            database: database.map(str::to_owned),
            password: None,
            password_required: true,
            created_at_ms: 0,
        }
    }

    #[test]
    fn dsn_quotes_values_and_never_contains_a_password() {
        let dsn = dsn_for(&connection("2001:db8::1", Some("gis")), None).expect("dsn");
        assert_eq!(dsn, "host='2001:db8::1' port=5432 user='gis user' sslmode='require' dbname='gis'");
        assert!(!dsn.contains("password"));
        let selected = dsn_for(&connection("localhost", Some("gis")), Some("other")).expect("dsn");
        assert!(selected.ends_with("dbname='other'"));
    }

    #[test]
    fn url_userinfo_is_redacted_in_error_text() {
        assert_eq!(
            strip_url_credentials("could not connect to postgresql://gis:secret@example.test:5432/gis"),
            "could not connect to postgresql://gis:***@example.test:5432/gis"
        );
        assert_eq!(strip_url_credentials("authentication failed for user gis"), "authentication failed for user gis");
        assert_eq!(strip_url_credentials("http://example.test/a:b"), "http://example.test/a:b");
    }
}
