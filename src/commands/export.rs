//! `demo export` — compile a demo score to one or more target formats.

use crate::cli::ExportArgs;
use crate::error::{Error, Result};
use crate::model::Score;

pub fn run(args: ExportArgs) -> Result<()> {
    let score = Score::load(&args.input)?;
    let targets = args.targets.0;

    // A single `-o` path can't name several formats at once.
    if args.output.is_some() && targets.len() > 1 {
        return Err(Error::Export(
            "-o/--output works with a single format; drop it to export several formats \
             (each uses its default name)"
                .to_string(),
        ));
    }

    for target in targets {
        let path = crate::export::export(&score, target, args.output.clone(), args.speed)?;
        println!("exported {} → {}", args.input.display(), path.display());
    }
    Ok(())
}
