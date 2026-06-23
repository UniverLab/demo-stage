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
pub mod recording;
pub mod run;
pub mod stage;

use std::path::{Path, PathBuf};

use crate::cli::Target;
use crate::error::{Error, Result};
use crate::model::Score;
use crate::validate::validate;

use run::Recording;

/// Render an already-captured `recording` to `target`, returning the path
/// written. Pure playback — it never executes the demo. `score` carries the
/// layout/styling (its timeline is unused here).
pub fn render(rec: &Recording, score: &Score, target: Target) -> Result<PathBuf> {
    let problems = validate(score);
    if !problems.is_empty() {
        return Err(Error::Validation(problems.join("\n")));
    }
    // A multi-pane stage composites the recorded terminal with its browser panes
    // (Chromium) on the pixel targets. The text targets (cast/html) can only carry
    // the terminal stream — browser panes are dropped there.
    let staged = stage::needs_stage(score);
    let (cw, ch, fps) = (
        score.layout.width as usize,
        score.layout.height as usize,
        score.layout.fps.max(1),
    );

    match target {
        Target::Cast => {
            let path = resolve_output(score, "cast");
            write(&path, cast::to_cast(rec)?.as_bytes())?;
            Ok(path)
        }
        Target::Html => {
            let path = resolve_output(score, "html");
            write(&path, html::to_html(rec)?.as_bytes())?;
            Ok(path)
        }
        Target::Gif => {
            let path = resolve_output(score, "gif");
            ensure_parent(&path)?;
            if staged {
                gif::encode(&path, cw, ch, fps, |emit| {
                    stage::render_stage(rec, score, |f| emit(f))
                })?;
            } else {
                gif::write_gif(rec, score, &path)?;
            }
            Ok(path)
        }
        Target::Mp4 => {
            let path = resolve_output(score, "mp4");
            ensure_parent(&path)?;
            if staged {
                mp4::encode(&path, cw, ch, fps, |emit| {
                    stage::render_stage(rec, score, |f| emit(f))
                })?;
            } else {
                mp4::write_mp4(rec, score, &path)?;
            }
            Ok(path)
        }
    }
}

/// Retime a recording by `1/speed` (so `speed = 2.0` plays twice as fast, `0.5`
/// half as fast): every output event, caption and focus, plus the duration.
pub fn scale_recording(rec: &mut Recording, speed: f64) {
    if speed == 1.0 {
        return;
    }
    let scale = |t: f64| t / speed;
    for (t, _) in &mut rec.events {
        *t = scale(*t);
    }
    for (t, _) in &mut rec.captions {
        *t = scale(*t);
    }
    for (t, _) in &mut rec.focuses {
        *t = scale(*t);
    }
    rec.duration = scale(rec.duration);
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

    fn rec() -> Recording {
        Recording {
            cols: 80,
            rows: 24,
            title: "t".into(),
            events: vec![(0.5, "a".into()), (1.0, "b".into())],
            captions: vec![(0.5, "cap".into())],
            focuses: vec![(0.25, "main".into())],
            duration: 1.0,
        }
    }

    #[test]
    fn speed_2x_halves_recorded_timestamps() {
        let mut r = rec();
        scale_recording(&mut r, 2.0);
        assert_eq!(r.events, vec![(0.25, "a".into()), (0.5, "b".into())]);
        assert_eq!(r.captions, vec![(0.25, "cap".into())]);
        assert_eq!(r.focuses, vec![(0.125, "main".into())]);
        assert_eq!(r.duration, 0.5);
    }

    #[test]
    fn speed_half_doubles_recorded_timestamps() {
        let mut r = rec();
        scale_recording(&mut r, 0.5);
        assert_eq!(r.events, vec![(1.0, "a".into()), (2.0, "b".into())]);
        assert_eq!(r.duration, 2.0);
    }

    #[test]
    fn speed_1x_is_a_no_op() {
        let mut r = rec();
        scale_recording(&mut r, 1.0);
        assert_eq!(r.events, rec().events);
    }
}
