//! `demo export` — render a recording to one or more formats. Pure playback:
//! it replays a recording (a `.cast` from `demo record`, or a raw `capture`)
//! and never executes the demo.

use crate::cli::{all_targets, ExportArgs};
use crate::error::Result;
use crate::export::{recording, render, scale_recording};

pub fn run(args: ExportArgs) -> Result<()> {
    let (mut rec, score, faithful) = recording::read(&args.input)?;
    scale_recording(&mut rec, args.speed);

    // A faithful capture renders the real session as-is — the typing and spacing
    // are exactly as recorded, NOT humanized. Say so, and how to get a clean take.
    if faithful {
        eprintln!(
            "note: rendering a faithful capture — typing and idle are as recorded, \
             not normalized. For humanized typing/spacing, run `demo record` first \
             (re-executes the demo), then export its `.rec`."
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
