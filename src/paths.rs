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
        Err(_)
            if shell_dir
                .and_then(|s| std::fs::canonicalize(s).ok())
                .is_some() =>
        {
            let shell = shell_dir
                .and_then(|s| std::fs::canonicalize(s).ok())
                .unwrap();
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

/// Build an absolute `file://` URL for a local file (e.g., from file picker).
/// Unlike `file_url_relative_to_launch`, this always uses the full absolute path,
/// so the demo works correctly from any launch directory.
pub fn file_url_absolute(path: &Path) -> Result<String> {
    let abs =
        std::fs::canonicalize(path).map_err(|e| Error::Export(format!("file not found: {e}")))?;
    if !is_supported_browser_file(&abs) {
        return Err(Error::Export(format!(
            "unsupported file type — supported: {}",
            SUPPORTED_BROWSER_EXTENSIONS.join(", ")
        )));
    }
    // Convert to file:// URL. On Unix, use the path directly; on Windows, the path
    // will have backslashes that need to be forward slashes.
    let path_str = abs
        .to_str()
        .ok_or_else(|| Error::Export("path contains invalid UTF-8".to_string()))?
        .replace('\\', "/");
    
    // On Windows, canonicalize returns `C:\...`, so we need to add the drive letter
    // prefix after `file:///`. On Unix, it's `/path/...`, so `file:///path/...`.
    if cfg!(windows) && path_str.len() > 1 && path_str.chars().nth(1) == Some(':') {
        // Windows: C:\... → file:///C:/...
        Ok(format!("file:///{path_str}"))
    } else {
        // Unix: /path/... → file:///path/...
        Ok(format!("file://{path_str}"))
    }
}

/// Rewrite Windows `file:///X:/…` URLs to native paths when Chromium runs on Unix
/// (WSL). Windows Chrome often copies WSL files as `file:///Z:/home/…`; Chromium
/// in WSL needs `/home/…` or `/mnt/c/…`.
pub fn normalize_windows_file_url(url: &str) -> String {
    let u = url.trim();
    let Some(rest) = u.strip_prefix("file:///") else {
        return u.to_string();
    };
    #[cfg(not(unix))]
    {
        return u.to_string();
    }
    #[cfg(unix)]
    {
        let Some((drive, path)) = rest.split_once(':') else {
            return u.to_string();
        };
        let drive = drive.trim_start_matches('/');
        if drive.len() != 1 || !drive.chars().all(|c| c.is_ascii_alphabetic()) {
            return u.to_string();
        }
        let path = path.trim_start_matches('/');
        if looks_like_unix_root_path(path) {
            let linux = format!("/{path}");
            if Path::new(&linux).exists() {
                return format!("file://{linux}");
            }
        }
        let linux = format!("/mnt/{}/{}", drive.to_ascii_lowercase(), path);
        if Path::new(&linux).exists() {
            return format!("file://{linux}");
        }
        u.to_string()
    }
}

fn looks_like_unix_root_path(path: &str) -> bool {
    matches!(
        path.split('/').next(),
        Some("home" | "tmp" | "var" | "usr" | "opt" | "mnt")
    )
}

/// Normalize a browser-pane URL from wizard/CLI input: repair Windows `file://`
/// paths, canonicalize local files relative to `launch_dir`, or add a protocol.
pub fn repair_browser_url(url: &str, launch_dir: &Path) -> Result<String> {
    let u = url.trim();
    if u.starts_with("file://") {
        let fixed = normalize_windows_file_url(u);
        if looks_like_local_path(&fixed) {
            return local_file_url(&fixed, launch_dir);
        }
        return Ok(fixed);
    }
    if looks_like_local_path(u) {
        return local_file_url(u, launch_dir);
    }
    Ok(normalize_url(u))
}

/// Resolve a browser pane URL. Relative `file://./…` paths are resolved from the
/// process cwd (the demo project directory when re-running).
pub fn resolve_browser_url(url: &str) -> Result<String> {
    let u = url.trim();
    let Some(path_part) = u.strip_prefix("file://") else {
        return Ok(u.to_string());
    };
    if path_part.starts_with('/') {
        let fixed = normalize_windows_file_url(u);
        if fixed != u {
            return Ok(fixed);
        }
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
    let trimmed = path.trim();
    if trimmed.starts_with("file://") {
        let fixed = normalize_windows_file_url(trimmed);
        if fixed != trimmed && fixed.starts_with("file://") {
            let linux = fixed
                .strip_prefix("file://")
                .unwrap_or(&fixed)
                .to_string();
            return file_url_relative_to_launch(Path::new(&linux), launch_dir, None);
        }
    }
    let raw = trimmed.strip_prefix("file://").unwrap_or(trimmed);
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

    #[test]
    fn normalize_windows_file_url_maps_wsl_home_paths() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        if !Path::new(&home).exists() {
            return;
        }
        let tail = home.trim_start_matches('/');
        let win = format!("file:///Z:/{tail}");
        let fixed = normalize_windows_file_url(&win);
        assert_eq!(fixed, format!("file://{home}"));
    }
}
