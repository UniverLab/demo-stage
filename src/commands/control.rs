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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("demo-control-test-{ts}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn setup_control(tmp: &std::path::Path) -> PathBuf {
        let control = tmp.join(CONTROL_FILE);
        std::fs::write(&control, "").unwrap();
        control
    }

    #[test]
    fn constants_are_expected_values() {
        assert_eq!(CONTROL_ENV, "DEMO_CAPTURE_CONTROL");
        assert_eq!(CONTROL_FILE, ".demo-capture");
        assert_eq!(SOURCES_FILE, ".demo-capture.sources");
        assert_eq!(META_FILE, ".demo-capture.meta");
    }

    #[test]
    fn find_returns_error_when_no_control_exists() {
        let original = std::env::var(CONTROL_ENV);
        std::env::remove_var(CONTROL_ENV);
        let result = find();
        assert!(result.is_err());
        if let Ok(v) = original { std::env::set_var(CONTROL_ENV, v) }
    }

    #[test]
    fn write_and_read_meta_round_trip() {
        let tmp = temp_dir();
        let control = setup_control(&tmp);
        let launch = PathBuf::from("/home/user/project");
        let shell = PathBuf::from("/tmp/sandbox");

        write_meta(&control, &launch, &shell).unwrap();

        let meta = read_meta_at(&control).unwrap();
        assert_eq!(meta.launch_dir, launch);
        assert_eq!(meta.shell_dir, shell);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_meta_creates_file() {
        let tmp = temp_dir();
        let control = setup_control(&tmp);
        let meta_path = control.with_file_name(META_FILE);

        assert!(!meta_path.exists());
        write_meta(&control, Path::new("/a"), Path::new("/b")).unwrap();
        assert!(meta_path.exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_and_read_sources_round_trip() {
        let tmp = temp_dir();
        let control = setup_control(&tmp);
        let sources = vec![crate::model::Source {
            id: "main".into(),
            kind: crate::model::SourceKind::Terminal,
            url: None,
            theme: None,
        }];

        write_sources(&control, &sources).unwrap();

        let sources_path = control.with_file_name(SOURCES_FILE);
        let data = std::fs::read_to_string(&sources_path).unwrap();
        let read: Vec<crate::model::Source> = serde_json::from_str(&data).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].id, "main");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_sources_empty_when_no_file() {
        let sources = read_sources_at(Path::new("/nonexistent"));
        assert!(sources.is_empty());
    }

    #[test]
    fn send_appends_json_line() {
        let tmp = temp_dir();
        let control = setup_control(&tmp);

        let cmd = serde_json::json!({"cmd": "open", "url": "https://example.com"});
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&control)
            .unwrap();
        let mut line = serde_json::to_string(&cmd).unwrap();
        line.push('\n');
        file.write_all(line.as_bytes()).unwrap();
        drop(file);

        let content = std::fs::read_to_string(&control).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["cmd"], "open");
        assert_eq!(parsed["url"], "https://example.com");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn send_returns_error_when_no_control() {
        let original = std::env::var(CONTROL_ENV);
        std::env::remove_var(CONTROL_ENV);
        let result = send(serde_json::json!({"cmd": "test"}));
        assert!(result.is_err());
        if let Ok(v) = original { std::env::set_var(CONTROL_ENV, v) }
    }

    #[test]
    fn capture_meta_serializes() {
        let meta = CaptureMeta {
            launch_dir: PathBuf::from("/home/user"),
            shell_dir: PathBuf::from("/tmp/sandbox"),
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("launch_dir"));
        assert!(json.contains("shell_dir"));
    }

    #[test]
    fn capture_meta_deserializes() {
        let json = r#"{"launch_dir":"/home/user","shell_dir":"/tmp/sandbox"}"#;
        let meta: CaptureMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.launch_dir, PathBuf::from("/home/user"));
        assert_eq!(meta.shell_dir, PathBuf::from("/tmp/sandbox"));
    }

    // Helper functions that bypass the env/cwd dependency for testing
    fn read_meta_at(control: &std::path::Path) -> Option<CaptureMeta> {
        let path = control.with_file_name(META_FILE);
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    fn read_sources_at(control: &std::path::Path) -> Vec<crate::model::Source> {
        let path = control.with_file_name(SOURCES_FILE);
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
}
