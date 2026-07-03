//! `demo record` — execute a demo score in a PTY and save the result as a
//! recording (a `.rec`) that `demo export` plays back.
//!
//! This is the repeatable step: re-run it after the app changes and the
//! recording refreshes. Rendering (`export`) never executes — it only replays
//! what `record` (or a raw `capture`) produced.

use crate::cli::RecordArgs;
use crate::error::{Error, Result};
use crate::export::{recording, run, stage};
use crate::model::{PaneKind, Score};
use crate::validate::validate;

pub fn run(args: RecordArgs) -> Result<()> {
    let mut score = Score::load(&args.input)?;

    let problems = validate(&score);
    if !problems.is_empty() {
        return Err(Error::Validation(problems.join("\n")));
    }

    // Record only the terminal pane's session. For a multi-pane stage, `export`
    // composites the browser panes (Chromium) around this recording at render time.
    let rec = if stage::needs_stage(&score) {
        let term = score
            .layout
            .panes
            .iter()
            .find(|p| p.kind == PaneKind::Terminal)
            .ok_or_else(|| Error::Export("a stage needs a terminal pane".to_string()))?;
        run::run_with_pane(&score, term)?
    } else {
        run::run_terminal(&score)?
    };
    // A `record` run replays the timeline on its own clock, so the browser
    // windows inherited from the capture no longer line up — re-anchor them to
    // when this run's `focus` steps actually fired.
    anchor_browser_windows(&mut score, &rec.focuses);

    // A `record` run is normalized (re-executed clean script), not faithful.
    let cast = recording::write(&rec, &score, false)?;
    if let Some(parent) = args.output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
    }
    std::fs::write(&args.output, cast).map_err(|e| Error::io(&args.output, e))?;

    println!(
        "recorded {} → {} (next: demo export)",
        args.input.display(),
        args.output.display()
    );
    Ok(())
}

/// Re-anchor each browser pane's `[reveal_at, hide_at)` window to this run's
/// clock. A score produced by `demo capture` carries windows on the *capture*
/// clock, but `record` re-executes the timeline with normalized timing — a
/// capture-time reveal (say 82s) can sit past the whole re-recorded playback
/// (say 31s), so the pane would never show in the export. The moments the
/// run's `focus` steps actually fired (`focuses`) are the truth: a pane
/// reveals at its first focus and hides when focus moves elsewhere.
fn anchor_browser_windows(score: &mut Score, focuses: &[(f64, String)]) {
    for pane in score
        .layout
        .panes
        .iter_mut()
        .filter(|p| p.kind == PaneKind::Browser)
    {
        match focuses.iter().position(|(_, id)| *id == pane.id) {
            Some(i) => {
                pane.reveal_at = Some(focuses[i].0);
                pane.hide_at = focuses[i + 1..]
                    .iter()
                    .find(|(_, id)| *id != pane.id)
                    .map(|(t, _)| *t);
            }
            None => {
                // Never focused in this run (e.g. its focus step was edited
                // out) — keep it off screen rather than at a stale time.
                pane.reveal_at = None;
                pane.hide_at = Some(0.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score_with_browser() -> Score {
        toml::from_str(
            r#"
[demo]
name = "t"
[layout]
width = 1440
height = 1080
  [[layout.panes]]
  id = "main"
  type = "terminal"
  x = 0
  y = 0
  width = 1440
  height = 1080
  [[layout.panes]]
  id = "github-r1"
  type = "browser"
  x = 0
  y = 0
  width = 1440
  height = 1080
  url = "https://example.com"
  reveal_at = 82.104
"#,
        )
        .unwrap()
    }

    #[test]
    fn anchors_a_stale_capture_window_to_the_runs_focus() {
        // The capture said 82.1s, but this run focused the pane at 21.4s.
        let mut score = score_with_browser();
        let focuses = vec![(0.12, "main".to_string()), (21.4, "github-r1".to_string())];
        anchor_browser_windows(&mut score, &focuses);
        let pane = &score.layout.panes[1];
        assert_eq!(pane.reveal_at, Some(21.4));
        assert_eq!(pane.hide_at, None);
    }

    #[test]
    fn focusing_back_to_the_terminal_closes_the_window() {
        let mut score = score_with_browser();
        let focuses = vec![(5.0, "github-r1".to_string()), (12.0, "main".to_string())];
        anchor_browser_windows(&mut score, &focuses);
        let pane = &score.layout.panes[1];
        assert_eq!(pane.reveal_at, Some(5.0));
        assert_eq!(pane.hide_at, Some(12.0));
    }

    #[test]
    fn a_pane_never_focused_stays_off_screen() {
        // Its focus step was edited out — don't show it at the stale capture time.
        let mut score = score_with_browser();
        anchor_browser_windows(&mut score, &[(0.1, "main".to_string())]);
        let pane = &score.layout.panes[1];
        assert_eq!(pane.reveal_at, None);
        assert_eq!(pane.hide_at, Some(0.0));
    }
}
