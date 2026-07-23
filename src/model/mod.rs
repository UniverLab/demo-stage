//! Data model for the DemoStage DSL.
//!
//! Two documents flow through the pipeline:
//! - [`RawMacro`] (`macro.raw.toml`) — the low-level capture from `demo record`.
//! - [`Score`] (`demo.toml`) — the clean, declarative result of `demo normalize`,
//!   consumed by `check` and `export`.

mod demo;
mod macro_raw;

pub use demo::*;
pub use macro_raw::*;

use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{Error, Result};

/// URL scheme marking a browser scene backed by pre-recorded frames from an
/// interactive `demo open --view` session (rather than a live URL captured at
/// export). The text after it is the frames directory.
pub const VIEW_FRAMES_SCHEME: &str = "viewframes:";

/// Build the `viewframes:<dir>` pointer stored in a view scene's `url`.
pub fn view_frames_url(dir: &str) -> String {
    format!("{VIEW_FRAMES_SCHEME}{dir}")
}

/// If `url` is a `viewframes:<dir>` pointer, return its frames directory.
pub fn view_frames_dir(url: &str) -> Option<&str> {
    url.strip_prefix(VIEW_FRAMES_SCHEME)
}

/// Read and parse a TOML document, attaching the path to any error.
pub(crate) fn load_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    toml::from_str(&text).map_err(|source| Error::TomlParse {
        path: path.to_path_buf(),
        source,
    })
}

/// Serialize a value to a pretty TOML string.
pub(crate) fn to_toml_string<T: Serialize>(value: &T) -> Result<String> {
    Ok(toml::to_string_pretty(value)?)
}

/// Serialize a value to TOML and write it to `path`.
pub(crate) fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    std::fs::write(path, to_toml_string(value)?).map_err(|e| Error::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn view_frames_scheme_round_trips() {
        let url = view_frames_url("demo-scenes/scene-42");
        assert_eq!(url, "viewframes:demo-scenes/scene-42");
        assert_eq!(view_frames_dir(&url), Some("demo-scenes/scene-42"));
        // A normal URL is not a view-frames pointer.
        assert_eq!(view_frames_dir("https://example.com"), None);
    }

    #[test]
    fn view_frames_dir_none_for_regular_url() {
        assert_eq!(view_frames_dir("http://example.com"), None);
        assert_eq!(view_frames_dir("file:///tmp/test.pdf"), None);
    }

    #[test]
    fn view_frames_dir_empty_dir() {
        assert_eq!(view_frames_dir("viewframes:"), Some(""));
    }

    #[test]
    fn to_toml_string_produces_valid_toml() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct S {
            name: String,
            count: u32,
        }
        let s = S {
            name: "test".into(),
            count: 42,
        };
        let t = to_toml_string(&s).unwrap();
        let parsed: S = toml::from_str(&t).unwrap();
        assert_eq!(parsed, s);
    }

    #[test]
    fn load_toml_round_trips_with_write_toml() {
        let dir = std::env::temp_dir().join("model-toml-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.toml");

        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct S {
            name: String,
            value: u32,
        }
        let original = S {
            name: "hello".into(),
            value: 99,
        };
        write_toml(&path, &original).unwrap();

        let loaded: S = load_toml(&path).unwrap();
        assert_eq!(loaded, original);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_toml_error_on_missing_file() {
        let dir = std::env::temp_dir().join("model-toml-missing-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nope.toml");

        #[derive(serde::Deserialize, Debug)]
        struct S {
            name: String,
        }

        let err = load_toml::<S>(&path).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("nope.toml"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_toml_error_on_invalid_content() {
        let dir = std::env::temp_dir().join("model-toml-invalid-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.toml");
        fs::write(&path, "this is not valid {{{ toml").unwrap();

        #[derive(serde::Deserialize, Debug)]
        struct S {
            name: String,
        }

        let err = load_toml::<S>(&path).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("bad.toml"));
        let _ = fs::remove_dir_all(&dir);
    }
}
