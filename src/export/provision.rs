//! Lazy provisioning of heavy export dependencies, tectonic-style: check the
//! system first, and on a miss notify the user and fetch a managed copy that
//! lives in a cache — instead of failing and asking them to install it by hand.

use std::path::PathBuf;

use crate::error::{Error, Result};

/// Ensure an `ffmpeg` binary is available (for the `mp4` target). Uses a system
/// install if present; otherwise downloads a managed static build on first use.
pub fn ensure_ffmpeg() -> Result<()> {
    if ffmpeg_sidecar::command::ffmpeg_is_installed() {
        return Ok(());
    }
    eprintln!("demo: ffmpeg not found — fetching a managed copy (one time, ~mid-tens of MB)…");
    ffmpeg_sidecar::download::auto_download().map_err(|e| {
        Error::Export(format!(
            "ffmpeg auto-download failed: {e}. Install ffmpeg manually and retry."
        ))
    })?;
    eprintln!("demo: ffmpeg ready.");
    Ok(())
}

/// Locate a Chromium/Chrome install (needed for `browser` panes). Returns the
/// path if found. Auto-provisioning of a managed Chromium is planned; until the
/// browser compositor lands, callers report this as a clear, actionable error.
pub fn find_chromium() -> Option<PathBuf> {
    const CANDIDATES: [&str; 5] = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
    ];
    CANDIDATES.iter().find_map(|name| which(name))
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}
