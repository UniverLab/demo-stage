//! `demo normalize` — refine a raw macro into a clean demo score.

use crate::cli::NormalizeArgs;
use crate::error::Result;
use crate::model::RawMacro;
use crate::normalize::{normalize, Options};

pub fn run(args: NormalizeArgs) -> Result<()> {
    let raw = RawMacro::load(&args.input)?;

    // Derive a name from the input file (macro.raw.toml → "macro").
    let stem = args
        .input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("demo");
    let name = stem.strip_suffix(".raw").unwrap_or(stem);

    let score = normalize(
        &raw,
        name,
        &Options {
            typing_ms: args.typing_ms,
            salt_ms: args.salt_ms,
            seed: args.seed,
        },
    );
    score.save(&args.output)?;

    println!(
        "normalized {} → {} ({} steps)",
        args.input.display(),
        args.output.display(),
        score.timeline.len()
    );
    Ok(())
}
