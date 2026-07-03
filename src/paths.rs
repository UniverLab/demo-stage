//! Local file paths for browser panes: supported extensions, relative `file://`
//! URLs anchored to the capture launch directory, and resolution at export time.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Extensions the browser pane can display (PDF, images, HTML).
pub const SUPPORTED_BROWSER_EXTENSIONS: &[&str] = &["pdf", "png", "html", "htm", "svg"];

pub fn is_supported_browser_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            SUPPORTED_BROWSER_EXTENSIONS
                .iter()
                .any(|s| ext.eq_ignore_ascii_case(s))
        })
}

/// Build a `file://` URL relative to `launch_dir` so re-runs (`record` / `export`)
/// resolve against wherever the demo is launched from. Files outside `launch_dir`
/// but inside `shell_dir` (the isolated capture sandbox) keep their path relative
/// to the shell cwd.
pub fn file_url_relative_to_launch(
    abs: &Path,
    launch_dir: &Path,
    shell_dir: Option<&Path>,
) -> Result<String> {
    let abs =
        std::fs::canonicalize(abs).map_err(|e| Error::Export(format!("file not found: {e}")))?;
    if !is_supported_browser_file(&abs) {
        return Err(Error::Export(format!(
            "unsupported file type — supported: {}",
            SUPPORTED_BROWSER_EXTENSIONS.join(", ")
        )));
    }
    let launch = std::fs::canonicalize(launch_dir)
        .map_err(|e| Error::Export(format!("launch directory not found: {e}")))?;
    let rel = match abs.strip_prefix(&launch) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) if shell_dir
            .and_then(|s| std::fs::canonicalize(s).ok())
            .is_some() =>
        {
            let shell = shell_dir.and_then(|s| std::fs::canonicalize(s).ok()).unwrap();
            abs.strip_prefix(&shell)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| {
                    abs.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| abs.to_string_lossy().to_string())
                })
        }
        Err(_) => abs
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| abs.to_string_lossy().to_string()),
    };
    let rel = rel.trim_start_matches("./");
    Ok(format!("file://./{rel}"))
}

/// Resolve a browser pane URL. Relative `file://./…` paths are resolved from the
/// process cwd (the demo project directory when re-running).
pub fn resolve_browser_url(url: &str) -> Result<String> {
    let u = url.trim();
    let Some(path_part) = u.strip_prefix("file://") else {
        return Ok(u.to_string());
    };
    if path_part.starts_with('/') {
        return Ok(u.to_string());
    }
    let rel = path_part.trim_start_matches("./");
    let cwd = std::env::current_dir().map_err(|e| Error::Export(format!("cwd: {e}")))?;
    let path = cwd.join(rel);
    let abs = std::fs::canonicalize(&path)
        .map_err(|e| Error::Export(format!("file not found for {u}: {e}")))?;
    Ok(format!("file://{}", abs.display()))
}

/// True when `s` looks like a local path rather than a remote URL.
pub fn looks_like_local_path(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("file://")
        || s.starts_with("./")
        || s.starts_with("../")
        || (s.starts_with('/') && !s.starts_with("//"))
        || (!s.contains("://")
            && Path::new(s)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| {
                    SUPPORTED_BROWSER_EXTENSIONS
                        .iter()
                        .any(|s| ext.eq_ignore_ascii_case(s))
                }))
}

/// Canonicalize a local path flag/argument into a launch-relative `file://` URL.
pub fn local_file_url(path: &str, launch_dir: &Path) -> Result<String> {
    let raw = path.trim().strip_prefix("file://").unwrap_or(path.trim());
    let p = PathBuf::from(raw);
    let abs = if p.is_absolute() {
        p
    } else {
        launch_dir.join(p)
    };
    file_url_relative_to_launch(&abs, launch_dir, None)
}

/// Ensure a remote URL has a protocol prefix. Bare domains like `google.com`
/// become `https://google.com`; `file://`, `http://`, `https://` are left as-is.
pub fn normalize_url(url: &str) -> String {
    let u = url.trim();
    if u.contains("://") {
        u.to_string()
    } else if u.starts_with("localhost") || u.starts_with("127.0.0.1") {
        format!("http://{u}")
    } else {
        format!("https://{u}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_supported_extensions() {
        assert!(is_supported_browser_file(Path::new("x.PDF")));
        assert!(is_supported_browser_file(Path::new("page.html")));
        assert!(!is_supported_browser_file(Path::new("readme.md")));
    }

    #[test]
    fn relative_file_url_round_trips_through_resolve() {
        let dir = std::env::temp_dir().join(format!("demo-paths-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("out.pdf");
        fs::write(&file, b"%PDF-1").unwrap();

        let url = file_url_relative_to_launch(&file, &dir, None).unwrap();
        assert_eq!(url, "file://./out.pdf");

        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let resolved = resolve_browser_url(&url).unwrap();
        std::env::set_current_dir(prev).unwrap();
        assert!(resolved.starts_with("file://"));
        assert!(resolved.ends_with("/out.pdf"));
    }

    #[test]
    fn looks_like_local_path_heuristic() {
        assert!(looks_like_local_path("./dist/main.pdf"));
        assert!(looks_like_local_path("file://./out.pdf"));
        assert!(!looks_like_local_path("https://example.com"));
    }
}
