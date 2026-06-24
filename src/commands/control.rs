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
