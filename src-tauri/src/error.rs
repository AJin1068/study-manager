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
}

impl serde::Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
