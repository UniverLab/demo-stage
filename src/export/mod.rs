//! `demo export` — compile a score to a target format.
//!
//! `cast`/`html` run the score in a PTY and capture text (no external deps).
//! `gif` rasterizes that capture in pure Rust. `mp4` and browser panes need
//! ffmpeg/chromium and are reported as unsupported when those are absent.

pub mod cast;
pub mod html;
pub mod run;

use std::path::{Path, PathBuf};

use crate::cli::Target;
use crate::error::{Error, Result};
use crate::model::Score;
use crate::validate::validate;

/// Export `score` to `target`, returning the path written.
pub fn export(score: &Score, target: Target, output: Option<PathBuf>) -> Result<PathBuf> {
    let problems = validate(score);
    if !problems.is_empty() {
        return Err(Error::Validation(problems.join("\n")));
    }

    match target {
        Target::Cast => {
            let rec = run::run_terminal(score)?;
            let path = resolve_output(score, output, "cast");
            write(&path, cast::to_cast(&rec)?.as_bytes())?;
            Ok(path)
        }
        Target::Html => {
            let rec = run::run_terminal(score)?;
            let path = resolve_output(score, output, "html");
            write(&path, html::to_html(&rec)?.as_bytes())?;
            Ok(path)
        }
        Target::Gif => Err(Error::Export(
            "gif export is not implemented yet".to_string(),
        )),
        Target::Mp4 => Err(Error::Export(
            "mp4 export needs ffmpeg, which is not available here; use cast/html for terminal demos"
                .to_string(),
        )),
    }
}

fn resolve_output(score: &Score, output: Option<PathBuf>, ext: &str) -> PathBuf {
    output.unwrap_or_else(|| {
        score
            .demo
            .output_dir
            .join(format!("{}.{ext}", sanitize(&score.demo.name)))
    })
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
    }
    std::fs::write(path, bytes).map_err(|e| Error::io(path, e))
}
