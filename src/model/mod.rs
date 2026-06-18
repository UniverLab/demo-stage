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
