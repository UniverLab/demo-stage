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
use crate::model::{Score, Step};
use crate::validate::validate;

/// Export `score` to `target` at `speed` (1.0 = recorded pace), returning the
/// path written.
pub fn export(score: &Score, target: Target, speed: f64) -> Result<PathBuf> {
    let problems = validate(score);
    if !problems.is_empty() {
        return Err(Error::Validation(problems.join("\n")));
    }

    // Apply the speed multiplier once, up front, so every render path (PTY text
    // and stage composite) sees the same scaled timing.
    let scaled;
    let score = if speed == 1.0 {
        score
    } else {
        scaled = scale_timing(score.clone(), speed);
        &scaled
    };

    match target {
        Target::Cast => {
            let rec = run::run_terminal(score)?;
            let path = resolve_output(score, "cast");
            write(&path, cast::to_cast(&rec)?.as_bytes())?;
            Ok(path)
        }
        Target::Html => {
            let rec = run::run_terminal(score)?;
            let path = resolve_output(score, "html");
            write(&path, html::to_html(&rec)?.as_bytes())?;
            Ok(path)
        }
        Target::Gif => {
            let path = resolve_output(score, "gif");
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
            let path = resolve_output(score, "mp4");
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

/// Scale every time-based value in the score by `1/speed` (so `speed = 2.0`
/// plays twice as fast, `0.5` half as fast): humanized typing and the
/// fixed-duration `wait`/`scroll` steps. Output-driven `wait_for_stdout` steps
/// are left alone — they pace off real program output, not the clock.
fn scale_timing(mut score: Score, speed: f64) -> Score {
    let scale = |ms: u64| ((ms as f64) / speed).round() as u64;
    if let Some(typing) = score.typing.as_mut() {
        typing.base_ms = scale(typing.base_ms);
        typing.salt_ms = scale(typing.salt_ms);
    }
    for step in &mut score.timeline {
        match step {
            Step::Wait { duration_ms } | Step::Scroll { duration_ms, .. } => {
                *duration_ms = scale(*duration_ms);
            }
            _ => {}
        }
    }
    score
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

fn resolve_output(score: &Score, ext: &str) -> PathBuf {
    score
        .demo
        .output_dir
        .join(format!("{}.{ext}", sanitize(&score.demo.name)))
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
    use super::*;

    #[test]
    fn sanitize_replaces_unsafe_chars() {
        assert_eq!(sanitize("my demo!"), "my-demo-");
        assert_eq!(sanitize("ok_name-1"), "ok_name-1");
        assert_eq!(sanitize("a/b\\c"), "a-b-c");
    }

    fn score() -> Score {
        toml::from_str(
            r#"
[demo]
name = "t"
[typing]
base_ms = 80
salt_ms = 20
[[timeline]]
action = "wait"
duration_ms = 1000
[[timeline]]
action = "wait_for_stdout"
match = "done"
[layout]
width = 100
height = 100
  [[layout.panes]]
  id = "m"
  type = "terminal"
  x = 0
  y = 0
  width = 100
  height = 100
"#,
        )
        .unwrap()
    }

    fn first_wait(s: &Score) -> u64 {
        s.timeline
            .iter()
            .find_map(|step| match step {
                Step::Wait { duration_ms } => Some(*duration_ms),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn speed_2x_halves_typing_and_waits() {
        let s = scale_timing(score(), 2.0);
        let typing = s.typing.as_ref().unwrap();
        assert_eq!(typing.base_ms, 40);
        assert_eq!(typing.salt_ms, 10);
        assert_eq!(first_wait(&s), 500);
    }

    #[test]
    fn speed_half_doubles_durations() {
        let s = scale_timing(score(), 0.5);
        assert_eq!(s.typing.as_ref().unwrap().base_ms, 160);
        assert_eq!(first_wait(&s), 2000);
    }

    #[test]
    fn speed_leaves_wait_for_stdout_alone() {
        // Output-driven waits have no clock duration to scale; they must survive.
        let s = scale_timing(score(), 3.0);
        assert!(s
            .timeline
            .iter()
            .any(|step| matches!(step, Step::WaitForStdout { .. })));
    }
}
