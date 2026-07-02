//! `demo export` — compile a score to a target format.
//!
//! `cast`/`html` run the score in a PTY and capture text (no external deps).
//! `gif` rasterizes that capture in pure Rust. `mp4` provisions ffmpeg on first
//! use. Multi-pane scores (with a `browser` pane) composite via the stage, which
//! drives Chromium for browser panes.

pub mod browser;
pub mod composite;
pub mod gif;
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

use run::{progress_bar, progress_clear, Recording};

/// Render an already-captured `recording` to `target`, returning the path
/// written. Pure playback — it never executes the demo. `score` carries the
/// layout/styling (its timeline is unused here).
pub fn render(rec: &Recording, score: &Score, target: Target) -> Result<PathBuf> {
    let problems = validate(score);
    if !problems.is_empty() {
        return Err(Error::Validation(problems.join("\n")));
    }
    let staged = stage::needs_stage(score);
    let fps = score.layout.fps.max(1);
    // Two distinct frame geometries:
    //  - staged: compositing on the canvas → layout.width/height
    //  - single-terminal: the pane grid → cols*cell_w × rows*cell_h
    let (cw, ch) = if staged {
        (score.layout.width as usize, score.layout.height as usize)
    } else {
        let plan = raster::plan(rec, score);
        (plan.width, plan.height)
    };
    let total_frames = (rec.duration * fps as f64).ceil() as usize + 1;

    match target {
        Target::Gif => {
            let path = resolve_output(score, "gif");
            ensure_parent(&path)?;
            if staged {
                let mut n = 0usize;
                gif::encode(&path, cw, ch, fps, |emit| {
                    stage::render_stage(rec, score, |f| {
                        n += 1;
                        progress_bar("exporting gif", n, total_frames);
                        emit(f);
                    })
                })?;
                progress_clear();
            } else {
                let mut n = 0usize;
                gif::encode(&path, cw, ch, fps, |emit| {
                    raster::render_frames(rec, score, |f| {
                        n += 1;
                        progress_bar("exporting gif", n, total_frames);
                        emit(f);
                    })
                    .map(|_| ())
                })?;
                progress_clear();
            }
            Ok(path)
        }
        Target::Mp4 => {
            let path = resolve_output(score, "mp4");
            ensure_parent(&path)?;
            if staged {
                let mut n = 0usize;
                mp4::encode(&path, cw, ch, fps, |emit| {
                    stage::render_stage(rec, score, |f| {
                        n += 1;
                        progress_bar("exporting mp4", n, total_frames);
                        emit(f);
                    })
                })?;
                progress_clear();
            } else {
                let mut n = 0usize;
                mp4::encode(&path, cw, ch, fps, |emit| {
                    raster::render_frames(rec, score, |f| {
                        n += 1;
                        progress_bar("exporting mp4", n, total_frames);
                        emit(f);
                    })
                    .map(|_| ())
                })?;
                progress_clear();
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

/// Retime the layout's pane reveal/hide windows by `1/speed`. They're absolute
/// times on the same clock as the recording, so a `--speed` export must scale
/// them together with [`scale_recording`] — otherwise a pane's window can slide
/// past the (shortened) playback and the pane never shows.
pub fn scale_pane_windows(score: &mut Score, speed: f64) {
    if speed == 1.0 {
        return;
    }
    for pane in &mut score.layout.panes {
        if let Some(t) = &mut pane.reveal_at {
            *t /= speed;
        }
        if let Some(t) = &mut pane.hide_at {
            *t /= speed;
        }
    }
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

    #[test]
    fn speed_scales_pane_windows_with_the_recording() {
        let mut score: Score = toml::from_str(
            r#"
[demo]
name = "t"
[layout]
width = 100
height = 100
  [[layout.panes]]
  id = "main"
  type = "terminal"
  x = 0
  y = 0
  width = 100
  height = 100
  [[layout.panes]]
  id = "b"
  type = "browser"
  x = 0
  y = 0
  width = 100
  height = 100
  url = "https://x"
  reveal_at = 20.0
  hide_at = 30.0
"#,
        )
        .unwrap();
        scale_pane_windows(&mut score, 2.0);
        let b = &score.layout.panes[1];
        assert_eq!(b.reveal_at, Some(10.0));
        assert_eq!(b.hide_at, Some(15.0));
        // The terminal pane has no window — untouched.
        assert_eq!(score.layout.panes[0].reveal_at, None);
    }
}
