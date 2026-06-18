//! `demo export` — compile a demo score to a target format.

use crate::cli::ExportArgs;
use crate::error::Result;
use crate::model::Score;

pub fn run(args: ExportArgs) -> Result<()> {
    let score = Score::load(&args.input)?;
    let path = crate::export::export(&score, args.target, args.output)?;
    println!("exported {} → {}", args.input.display(), path.display());
    Ok(())
}
