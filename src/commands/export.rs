//! `demo export` — render a recording to one or more formats. Pure playback:
//! it replays a recording (a `.cast` from `demo record`, or a raw `capture`)
//! and never executes the demo.

use crate::cli::{all_targets, ExportArgs};
use crate::error::Result;
use crate::export::{recording, render, scale_recording};

pub fn run(args: ExportArgs) -> Result<()> {
    let (mut rec, score, faithful) = recording::read(&args.input)?;

    // A faithful capture renders the real session as-is — typing and spacing are
    // exactly as recorded, NOT humanized. Require `--force` to render one, so the
    // clean path (`demo record`) is the default; but keep it possible, since
    // interactive / side-effecting demos (ghScaff, secrets) can't be re-executed.
    if faithful && !args.force {
        return Err(crate::error::Error::Export(format!(
            "{} is a faithful capture — its typing/idle are as recorded, not \
             humanized. Run `demo record` first for a clean, re-humanized take, \
             then export its `.rec`. Or pass `--force` to render this capture \
             as-is (needed for interactive/side-effecting demos that can't be \
             re-executed).",
            args.input.display()
        )));
    }

    scale_recording(&mut rec, args.speed);

    if faithful {
        eprintln!(
            "note: rendering a faithful capture as-is (--force) — typing/idle are \
             as recorded; `demo record` would re-humanize them."
        );
    }

    // No target given → build every supported format.
    let targets = args.targets.map(|t| t.0).unwrap_or_else(all_targets);
    for target in targets {
        let path = render(&rec, &score, target)?;
        println!("exported {} → {}", args.input.display(), path.display());
    }
    Ok(())
}
