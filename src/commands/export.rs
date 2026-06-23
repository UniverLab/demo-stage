//! `demo export` — compile a demo score to one or more target formats.

use crate::cli::{all_targets, ExportArgs};
use crate::error::Result;
use crate::model::Score;

pub fn run(args: ExportArgs) -> Result<()> {
    let score = Score::load(&args.input)?;
    // No target given → build every supported format.
    let targets = args.targets.map(|t| t.0).unwrap_or_else(all_targets);

    for target in targets {
        let path = crate::export::export(&score, target, args.speed)?;
        println!("exported {} → {}", args.input.display(), path.display());
    }
    Ok(())
}
