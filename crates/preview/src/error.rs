//! Errors shared by storage, source and database APIs.

/// A failure with enough structure for an HTTP layer to choose a status code without matching on
/// error text.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("project not found: {0}")]
    ProjectNotFound(String),
    #[error("version not found: {0}")]
    VersionNotFound(String),
    #[error("working copy not found: {0}")]
    WorkingCopyNotFound(String),
    #[error("source not found: {0}")]
    SourceNotFound(String),
    #[error("source unavailable: {0}")]
    SourceUnavailable(String),
    #[error("path is not inside a configured source: {0}")]
    PathNotAllowed(String),
    #[error("file not found: {0}")]
    FileNotFound(String),
    #[error("not a directory: {0}")]
    NotADirectory(String),
    #[error("database password required: {0}")]
    PasswordRequired(String),
    #[error("database unavailable: {0}")]
    DatabaseUnavailable(String),
    #[error("version already exists: {0}")]
    VersionExists(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("not a RustGIS project: {0}")]
    InvalidProject(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl StoreError {
    /// A stable, short code for API responses.
    pub fn code(&self) -> &'static str {
        match self {
            StoreError::ProjectNotFound(_) => "project_not_found",
            StoreError::VersionNotFound(_) => "version_not_found",
            StoreError::WorkingCopyNotFound(_) => "working_copy_not_found",
            StoreError::SourceNotFound(_) => "source_not_found",
            StoreError::SourceUnavailable(_) => "source_unavailable",
            StoreError::PathNotAllowed(_) => "path_not_allowed",
            StoreError::FileNotFound(_) => "file_not_found",
            StoreError::NotADirectory(_) => "not_a_directory",
            StoreError::PasswordRequired(_) => "password_required",
            StoreError::DatabaseUnavailable(_) => "database_unavailable",
            StoreError::VersionExists(_) => "version_exists",
            StoreError::InvalidInput(_) => "invalid_input",
            StoreError::InvalidProject(_) => "invalid_project",
            StoreError::Io(_) => "io_error",
            StoreError::Json(_) => "invalid_json",
        }
    }

    /// The HTTP status the API should return for this error.
    pub fn status(&self) -> u16 {
        match self {
            StoreError::ProjectNotFound(_) | StoreError::VersionNotFound(_) | StoreError::WorkingCopyNotFound(_) => 404,
            StoreError::SourceNotFound(_) | StoreError::FileNotFound(_) => 404,
            StoreError::PathNotAllowed(_) => 403,
            StoreError::PasswordRequired(_) => 401,
            StoreError::DatabaseUnavailable(_) => 503,
            StoreError::SourceUnavailable(_) | StoreError::NotADirectory(_) => 409,
            StoreError::VersionExists(_) => 409,
            StoreError::InvalidInput(_) | StoreError::InvalidProject(_) | StoreError::Json(_) => 400,
            StoreError::Io(_) => 500,
        }
    }
}
