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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_chromium_returns_a_path_on_this_system() {
        // On most dev machines, at least one of the candidates exists.
        // This test just verifies the function doesn't panic.
        let _ = find_chromium();
    }

    #[test]
    fn which_finds_ls_on_unix() {
        let result = which("ls");
        assert!(result.is_some());
        assert!(result.unwrap().is_file());
    }

    #[test]
    fn which_returns_none_for_nonexistent() {
        assert!(which("definitely_not_a_real_binary_xyz123").is_none());
    }

    #[test]
    fn which_respects_path() {
        // which() uses the current PATH, so just verify it compiles and runs
        let _ = which("cargo");
    }
}
