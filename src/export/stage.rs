//! Multi-scene stage (SPEC §4): run the terminal pane in a PTY, capture each
//! browser pane via headless Chromium, and composite all panes onto the canvas
//! frame by frame. Terminal capture + compositing are verified; the browser
//! capture needs Chromium and is verified outside the sandbox.

use super::run::Recording;
use super::{browser, composite, raster};
use crate::error::{Error, Result};
use crate::model::{Pane, PaneKind, Score, Step};

/// True when a score has more than one pane (or any browser pane) and therefore
/// needs the compositing stage rather than the single-terminal fast path.
pub fn needs_stage(score: &Score) -> bool {
    let has_browser = score
        .layout
        .panes
        .iter()
        .any(|p| p.kind == PaneKind::Browser);
    let terminals = score
        .layout
        .panes
        .iter()
        .filter(|p| p.kind == PaneKind::Terminal)
        .count();
    has_browser || terminals > 1
}

/// Composite a multi-pane score from an already-captured terminal `rec`,
/// emitting each composited canvas frame. Pure playback — the terminal pane comes
/// from `rec` (never re-run); browser panes are captured here via Chromium.
pub fn render_stage(rec: &Recording, score: &Score, mut on_frame: impl FnMut(&[u8])) -> Result<()> {
    let canvas_w = score.layout.width as usize;
    let canvas_h = score.layout.height as usize;
    let bg = score
        .layout
        .background
        .as_deref()
        .and_then(raster::parse_hex)
        .unwrap_or([11, 15, 20]);

    let term_pane = score
        .layout
        .panes
        .iter()
        .find(|p| p.kind == PaneKind::Terminal)
        .ok_or_else(|| Error::Export("a multi-scene stage needs a terminal pane".to_string()))?;

    // Captions are drawn on the composited canvas, so keep them out of the
    // terminal sub-frames (render the terminal from a captions-free copy).
    let font_name = score
        .layout
        .font_family
        .as_deref()
        .unwrap_or(crate::fonts::DEFAULT_FONT);
    let mut caption = if rec.captions.is_empty() {
        None
    } else {
        Some(raster::CaptionOverlay::new(
            rec.captions.clone(),
            20.0,
            font_name,
        )?)
    };
    let mut term_rec = rec.clone();
    term_rec.captions.clear();
    let mut term_src = raster::FrameSource::new(&term_rec, score)?;
    let (tw, th) = term_src.dims();
    let n = term_src.n_frames();
    let fps = score.layout.fps.max(1) as f64;

    // Browser panes captured up front (Chromium). Each reveals at the moment it
    // is first focused (recorded during the terminal run) — so it "opens" exactly
    // when the demo focuses it, e.g. once a server is up or a PDF has compiled.
    let mut scenes: Vec<(&Pane, browser::Scene, f64)> = Vec::new();
    for pane in score
        .layout
        .panes
        .iter()
        .filter(|p| p.kind == PaneKind::Browser)
    {
        let scrolls = scroll_keyframes_for(score, &pane.id);
        let reveal_at = rec
            .focuses
            .iter()
            .find(|(_, id)| *id == pane.id)
            .map(|(t, _)| *t)
            .unwrap_or(0.0);
        scenes.push((pane, browser::capture(pane, scrolls)?, reveal_at));
    }

    let total = n as f64 / fps;
    for i in 0..n {
        let t = i as f64 / fps;
        let term_frame = term_src.next_frame().unwrap_or_default();

        let mut layers = vec![composite::Layer {
            x: term_pane.x as usize,
            y: term_pane.y as usize,
            w: tw,
            h: th,
            rgba: &term_frame,
        }];
        for (pane, scene, reveal_at) in &scenes {
            if t < *reveal_at {
                continue; // not revealed yet
            }
            // Scene-local progress: 0 at the reveal, 1 at the end — so a scene's
            // scroll keyframes play across the window it's actually on screen,
            // not across the whole demo (which would mostly be before it opened).
            let span = (total - reveal_at).max(1e-6);
            let progress = ((t - reveal_at) / span).clamp(0.0, 1.0);
            layers.push(composite::Layer {
                x: pane.x as usize,
                y: pane.y as usize,
                w: scene.width,
                h: scene.height,
                rgba: scene.frame_at(progress),
            });
        }
        let mut canvas = composite::composite(canvas_w, canvas_h, bg, &layers);
        if let Some(caption) = &mut caption {
            caption.draw(&mut canvas, canvas_w, canvas_h, i as f64 / fps);
        }
        on_frame(&canvas);
    }
    Ok(())
}

/// How many scroll keyframes to capture for a browser pane — roughly one per
/// 700 ms of scrolling directed at it (explicitly or via focus), capped.
fn scroll_keyframes_for(score: &Score, pane_id: &str) -> usize {
    let mut focused: Option<&str> = None;
    let mut ms = 0u64;
    for step in &score.timeline {
        match step {
            Step::Focus { pane, scene } => {
                focused = if let Some(p) = pane {
                    Some(p.as_str())
                } else {
                    scene.as_deref()
                };
            }
            Step::Scroll {
                duration_ms, pane, ..
            } => {
                if pane.as_deref().or(focused) == Some(pane_id) {
                    ms += duration_ms;
                }
            }
            _ => {}
        }
    }
    ((ms / 700) as usize).clamp(0, 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(toml_str: &str) -> Score {
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn detects_when_a_stage_is_needed() {
        let single = score(
            r#"
[demo]
name = "t"
[layout]
width = 100
height = 100
  [[layout.panes]]
  id = "c"
  type = "terminal"
  x = 0
  y = 0
  width = 100
  height = 100
"#,
        );
        assert!(!needs_stage(&single));

        let multi = score(
            r#"
[demo]
name = "t"
[layout]
width = 200
height = 100
  [[layout.panes]]
  id = "c"
  type = "terminal"
  x = 0
  y = 0
  width = 100
  height = 100
  [[layout.panes]]
  id = "p"
  type = "browser"
  x = 100
  y = 0
  width = 100
  height = 100
  url = "file:///x.pdf"
"#,
        );
        assert!(needs_stage(&multi));
    }

    #[test]
    fn counts_scroll_keyframes_for_the_focused_browser() {
        let s = score(
            r#"
[demo]
name = "t"
[layout]
width = 200
height = 100
  [[layout.panes]]
  id = "c"
  type = "terminal"
  x = 0
  y = 0
  width = 100
  height = 100
  [[layout.panes]]
  id = "p"
  type = "browser"
  x = 100
  y = 0
  width = 100
  height = 100
  url = "file:///x.pdf"
[[timeline]]
action = "focus"
pane = "p"
[[timeline]]
action = "scroll"
direction = "down"
duration_ms = 2100
"#,
        );
        // 2100ms / 700 = 3 keyframes for "p", none for "c".
        assert_eq!(scroll_keyframes_for(&s, "p"), 3);
        assert_eq!(scroll_keyframes_for(&s, "c"), 0);
    }
}
