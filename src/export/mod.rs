//! `demo export` — compile a score to a target format.
//!
//! `cast`/`html` run the score in a PTY and capture text (no external deps).
//! `gif` rasterizes that capture in pure Rust. `mp4` provisions ffmpeg on first
//! use. Multi-pane scores (with a `browser` pane) composite via the stage, which
//! drives Chromium for browser panes.

pub mod browser;
pub mod cast;
pub mod composite;
pub mod gif;
pub mod html;
pub mod mp4;
pub mod provision;
pub mod raster;
pub mod run;
pub mod stage;

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
        Target::Gif => {
            let path = resolve_output(score, output, "gif");
            ensure_parent(&path)?;
            if stage::needs_stage(score) {
                let (w, h, fps) = canvas_dims(score);
                gif::encode(&path, w, h, fps, |emit| {
                    stage::render_stage(score, |f| emit(f))
                })?;
            } else {
                let rec = run::run_terminal(score)?;
                gif::write_gif(&rec, score, &path)?;
            }
            Ok(path)
        }
        Target::Mp4 => {
            let path = resolve_output(score, output, "mp4");
            ensure_parent(&path)?;
            if stage::needs_stage(score) {
                let (w, h, fps) = canvas_dims(score);
                mp4::encode(&path, w, h, fps, |emit| {
                    stage::render_stage(score, |f| emit(f))
                })?;
            } else {
                let rec = run::run_terminal(score)?;
                mp4::write_mp4(&rec, score, &path)?;
            }
            Ok(path)
        }
    }
}

/// Output canvas size for a multi-pane stage render.
fn canvas_dims(score: &Score) -> (usize, usize, u32) {
    (
        score.layout.width as usize,
        score.layout.height as usize,
        score.layout.fps.max(1),
    )
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn sanitize_replaces_unsafe_chars() {
        assert_eq!(sanitize("my demo!"), "my-demo-");
        assert_eq!(sanitize("ok_name-1"), "ok_name-1");
        assert_eq!(sanitize("a/b\\c"), "a-b-c");
    }
}
