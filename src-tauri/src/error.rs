use crate::platform::PlatformError;

/// Errors that cross the Tauri command boundary. Serialized as a plain string for the UI.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("platform error: {0}")]
    Platform(#[from] PlatformError),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("internal state error: {0}")]
    Internal(String),
    /// The port has no usable configuration (or is in an unimplemented mode). The UI shows an
    /// input card for it; local features keep working.
    #[error("not configured: {port}")]
    NotConfigured { port: &'static str },
    /// A server-link failure with a stable code; never carries URLs, paths, bodies, or tokens.
    #[error("remote error: {0}")]
    Remote(String),
}

impl serde::Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
