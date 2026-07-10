//! IPC between a running `demo capture` and the `demo open` / `demo stop`
//! commands. The recorder writes a **control file** (`.demo-capture`) in its
//! working directory and exports its path in [`CONTROL_ENV`] for the captured
//! shell. Commands append one JSON line to that file; the recorder polls it.
//!
//! Resolving the file by the env var (inside the capture) *or* by the cwd (from
//! another terminal) is what lets you run `demo open` from a separate shell —
//! handy when a full-screen TUI owns the captured terminal.

use std::io::Write;
use std::path::PathBuf;

use crate::error::{Error, Result};

/// Env var the recorder exports into the captured shell, holding the control path.
pub const CONTROL_ENV: &str = "DEMO_CAPTURE_CONTROL";
/// Control file name written in the recorder's working directory.
pub const CONTROL_FILE: &str = ".demo-capture";
/// Sidecar (beside the control file) listing the sources configured at capture
/// start, so `demo focus`/`demo open` wizards can offer them live — the
/// `demo.toml` score isn't written until the capture ends.
pub const SOURCES_FILE: &str = ".demo-capture.sources";
/// Sidecar with capture roots: where `demo capture` was launched and where the
/// shell runs (may differ when using the isolated sandbox).
pub const META_FILE: &str = ".demo-capture.meta";

#[derive(serde::Serialize, serde::Deserialize)]
pub struct CaptureMeta {
    pub launch_dir: PathBuf,
    pub shell_dir: PathBuf,
}

/// Locate the active capture's control file: the env var (inside the capture)
/// first, then `./.demo-capture` (from another terminal in the same directory).
pub fn find() -> Result<PathBuf> {
    if let Ok(p) = std::env::var(CONTROL_ENV) {
        if !p.is_empty() && std::path::Path::new(&p).exists() {
            return Ok(PathBuf::from(p));
        }
    }
    let cwd = PathBuf::from(CONTROL_FILE);
    if cwd.exists() {
        return Ok(cwd);
    }
    Err(Error::Export(
        "no running `demo capture` found — run this inside the capture, or from \
         the directory where `demo capture` is running"
            .to_string(),
    ))
}

/// Sidecar path beside the active control file (the sources list).
fn sources_path() -> Option<PathBuf> {
    let control = find().ok()?;
    Some(control.with_file_name(SOURCES_FILE))
}

/// Write capture roots next to the control file for live wizards.
pub fn write_meta(
    control: &std::path::Path,
    launch_dir: &std::path::Path,
    shell_dir: &std::path::Path,
) -> Result<()> {
    let path = control.with_file_name(META_FILE);
    let meta = CaptureMeta {
        launch_dir: launch_dir.to_path_buf(),
        shell_dir: shell_dir.to_path_buf(),
    };
    let json = serde_json::to_string(&meta)?;
    std::fs::write(&path, json).map_err(|e| Error::io(&path, e))
}

/// Read capture roots from the active capture (if any).
pub fn read_meta() -> Option<CaptureMeta> {
    let control = find().ok()?;
    let path = control.with_file_name(META_FILE);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// Write the sources configured at capture start next to the control file.
pub fn write_sources(control: &std::path::Path, sources: &[crate::model::Source]) -> Result<()> {
    let path = control.with_file_name(SOURCES_FILE);
    let json = serde_json::to_string(sources)?;
    std::fs::write(&path, json).map_err(|e| Error::io(&path, e))
}

/// Read the active capture's sources sidecar (empty if there's no capture).
pub fn read_sources() -> Vec<crate::model::Source> {
    sources_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Append one JSON command line to the active capture's control file.
pub fn send(cmd: serde_json::Value) -> Result<()> {
    let path = find()?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .map_err(|e| Error::io(&path, e))?;
    let mut line = serde_json::to_string(&cmd)?;
    line.push('\n');
    file.write_all(line.as_bytes())
        .map_err(|e| Error::io(&path, e))
}
