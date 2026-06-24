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
    let score = Score::load(&args.input)?;

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
