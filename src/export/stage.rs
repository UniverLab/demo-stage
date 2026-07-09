//! Multi-scene stage (SPEC §4): run the terminal pane in a PTY, capture each
//! browser pane via headless Chromium, and composite all panes onto the canvas
//! frame by frame. Terminal capture + compositing are verified; the browser
//! capture needs Chromium and is verified outside the sandbox.

use super::run::Recording;
use super::{browser, composite, raster};
use crate::error::{Error, Result};
use crate::model::{Pane, PaneKind, Score, Step};

/// True when a score has more than one pane, any browser pane, or a terminal
/// pane that doesn't fill the canvas (e.g. a capture with an explicit export
/// resolution, where the terminal sits centered on a larger canvas) — all cases
/// that need the compositing stage rather than the single-terminal fast path.
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
    let offset_terminal = score.layout.panes.iter().any(|p| {
        p.kind == PaneKind::Terminal
            && (p.x != 0
                || p.y != 0
                || p.width != score.layout.width
                || p.height != score.layout.height)
    });
    has_browser || terminals > 1 || offset_terminal
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
            crate::fonts::load_emoji(),
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
    let mut scenes: Vec<(&Pane, browser::Scene, f64, Option<f64>)> = Vec::new();
    for pane in score
        .layout
        .panes
        .iter()
        .filter(|p| p.kind == PaneKind::Browser)
    {
        let scrolls = scroll_keyframes_for(score, &pane.id);
        let (reveal_at, hide_at) = pane_window(pane, &rec.focuses);
        scenes.push((pane, browser::capture(pane, scrolls)?, reveal_at, hide_at));
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
        for (pane, scene, reveal_at, hide_at) in &scenes {
            if t < *reveal_at {
                continue; // not revealed yet
            }
            if hide_at.is_some_and(|h| t >= h) {
                continue; // switched away — the pane beneath (terminal) shows again
            }
            // Scene-local progress: 0 at the reveal, 1 at the end of its window —
            // so a scene's scroll keyframes play across the time it's on screen,
            // not across the whole demo (which would mostly be before it opened).
            let window_end = hide_at.unwrap_or(total);
            let span = (window_end - reveal_at).max(1e-6);
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

/// A browser pane's on-screen window `[reveal, hide)`. The recording's focus
/// events are the source of truth when they mention the pane: they sit on the
/// playback clock (and are speed-scaled with the rest of the recording), while
/// the score's `reveal_at`/`hide_at` may still carry times from an earlier
/// capture. The pane reveals at its first focus and hides when focus moves to
/// another pane; the score's window is the fallback for what the events don't
/// say (e.g. a hide recorded as a back-to-terminal reveal with no focus event,
/// or a recording that carries no focus for the pane at all).
fn pane_window(pane: &Pane, focuses: &[(f64, String)]) -> (f64, Option<f64>) {
    match focuses.iter().position(|(_, id)| *id == pane.id) {
        Some(i) => {
            let reveal = focuses[i].0;
            let hide = focuses[i + 1..]
                .iter()
                .find(|(_, id)| *id != pane.id)
                .map(|(t, _)| *t)
                .or(pane.hide_at.filter(|h| *h > reveal));
            (reveal, hide)
        }
        None => (pane.reveal_at.unwrap_or(0.0), pane.hide_at),
    }
}

/// How many scroll keyframes to capture for a browser pane — roughly one per
/// 700 ms of scrolling directed at it (explicitly or via focus), capped.
fn scroll_keyframes_for(score: &Score, pane_id: &str) -> usize {
    let mut focused: Option<&str> = None;
    let mut ms = 0u64;
    for step in &score.timeline {
        match step {
            Step::Focus { pane } => {
                focused = pane.as_deref();
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

        // A terminal centered on a larger canvas (explicit export resolution)
        // needs compositing even without browser panes.
        let centered = score(
            r#"
[demo]
name = "t"
[layout]
width = 1920
height = 1080
  [[layout.panes]]
  id = "main"
  type = "terminal"
  x = 560
  y = 300
  width = 800
  height = 480
"#,
        );
        assert!(needs_stage(&centered));
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

    fn browser_pane(reveal_at: Option<f64>, hide_at: Option<f64>) -> Pane {
        Pane {
            id: "b".into(),
            kind: PaneKind::Browser,
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            font_family: None,
            font_size: None,
            url: Some("https://x".into()),
            theme: None,
            reveal_at,
            hide_at,
        }
    }

    #[test]
    fn focus_events_override_a_stale_pane_window() {
        // The score says 82s (capture clock) but this recording focused the
        // pane at 21.4s, then went back to the terminal at 28s.
        let pane = browser_pane(Some(82.1), None);
        let focuses = vec![(21.4, "b".to_string()), (28.0, "main".to_string())];
        assert_eq!(pane_window(&pane, &focuses), (21.4, Some(28.0)));
    }

    #[test]
    fn score_hide_backs_up_missing_focus_events() {
        // Faithful capture: the hide was recorded as a back-to-terminal reveal
        // (no focus event), so the pane's own hide_at closes the window…
        let pane = browser_pane(Some(1.0), Some(2.0));
        assert_eq!(
            pane_window(&pane, &[(1.0, "b".to_string())]),
            (1.0, Some(2.0))
        );
        // …but a hide that predates the observed reveal is stale — ignored.
        let stale = browser_pane(Some(82.0), Some(15.0));
        assert_eq!(
            pane_window(&stale, &[(21.4, "b".to_string())]),
            (21.4, None)
        );
    }

    #[test]
    fn without_focus_events_the_scores_window_stands() {
        let pane = browser_pane(Some(3.0), Some(9.0));
        assert_eq!(pane_window(&pane, &[]), (3.0, Some(9.0)));
        let bare = browser_pane(None, None);
        assert_eq!(pane_window(&bare, &[]), (0.0, None));
    }
}
