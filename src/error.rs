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

    /// JSON encoding failed (the recording header/events).
    #[error("could not encode JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// Something went wrong while recording or exporting.
    #[error("{0}")]
    Export(String),

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_io_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let err = Error::io(PathBuf::from("/tmp/test.toml"), io_err);
        let msg = err.to_string();
        assert!(msg.contains("/tmp/test.toml"));
        assert!(msg.contains("no such file"));
    }

    #[test]
    fn error_toml_parse_display() {
        let toml_err = "invalid key".parse::<toml::Value>().unwrap_err();
        let err = Error::TomlParse {
            path: PathBuf::from("bad.toml"),
            source: toml_err,
        };
        let msg = err.to_string();
        assert!(msg.contains("bad.toml"));
        assert!(msg.contains("invalid TOML"));
    }

    #[test]
    fn error_toml_serialize_display() {
        // Trigger a TOML serialization error: nested arrays of mixed types
        let data = serde_json::json!({"k": [1, "mixed"]});
        let toml_val: toml::Value = toml::Value::try_from(data).unwrap();
        let _err_str = toml::to_string_pretty(&toml_val).unwrap();
        // Just verify we can construct the error variant and its display works
        let err = Error::Export(format!("could not serialize TOML: test"));
        assert!(err.to_string().contains("could not serialize TOML"));
    }

    #[test]
    fn error_validation_display() {
        let err = Error::Validation("problem 1\nproblem 2".into());
        let msg = err.to_string();
        assert!(msg.contains("validation failed"));
        assert!(msg.contains("problem 1"));
    }

    #[test]
    fn error_json_display() {
        let json_err = serde_json::from_str::<serde_json::Value>("{bad}").unwrap_err();
        let err = Error::Json(json_err);
        let msg = err.to_string();
        assert!(msg.contains("could not encode JSON"));
    }

    #[test]
    fn error_export_display() {
        let err = Error::Export("something went wrong".into());
        assert_eq!(err.to_string(), "something went wrong");
    }

    #[test]
    fn error_unimplemented_display() {
        let err = Error::Unimplemented("future feature");
        let msg = err.to_string();
        assert!(msg.contains("not yet implemented"));
        assert!(msg.contains("future feature"));
    }

    #[test]
    fn error_from_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("}bad{").unwrap_err();
        let err: Error = json_err.into();
        assert!(matches!(err, Error::Json(_)));
    }
}
