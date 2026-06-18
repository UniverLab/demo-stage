//! The crate-wide error type.

use std::path::PathBuf;

use thiserror::Error;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong in DemoStage.
#[derive(Debug, Error)]
pub enum Error {
    /// A filesystem operation failed, with the offending path for context.
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A TOML document could not be parsed.
    #[error("{path}: invalid TOML: {source}")]
    TomlParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// A value could not be serialized to TOML.
    #[error("could not serialize TOML: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    /// Static validation of a score found one or more problems.
    #[error("validation failed:\n{0}")]
    Validation(String),

    /// A command (or part of one) is not implemented yet.
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
}

impl Error {
    /// Wrap an [`std::io::Error`] together with the path it occurred on.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}
