use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Failed to compute signature for {path}: {reason}")]
    Signature { path: PathBuf, reason: String },

    #[error("Unsupported file format: {0}")]
    UnsupportedFormat(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Scan cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
