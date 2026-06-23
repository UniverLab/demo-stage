//! `demo record` — execute a demo score in a PTY and save the result as a
//! recording (an asciinema `.cast`) that `demo export` plays back.
//!
//! This is the repeatable step: re-run it after the app changes and the
//! recording refreshes. Rendering (`export`) never executes — it only replays
//! what `record` (or a raw `capture`) produced.

use crate::cli::RecordArgs;
use crate::error::{Error, Result};
use crate::export::{recording, run, stage};
use crate::model::Score;
use crate::validate::validate;

pub fn run(args: RecordArgs) -> Result<()> {
    let score = Score::load(&args.input)?;

    let problems = validate(&score);
    if !problems.is_empty() {
        return Err(Error::Validation(problems.join("\n")));
    }
    if stage::needs_stage(&score) {
        return Err(Error::Export(
            "multi-pane (browser) demos aren't supported by record yet — \
             only single-terminal scores"
                .to_string(),
        ));
    }

    let rec = run::run_terminal(&score)?;
    let cast = recording::write(&rec, &score)?;
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
