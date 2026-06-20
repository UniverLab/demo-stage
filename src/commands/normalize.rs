//! `demo normalize` — refine a raw macro into a clean demo score.

use std::path::PathBuf;

use crate::cli::NormalizeArgs;
use crate::error::Result;
use crate::model::{RawMacro, Score};
use crate::normalize::{merge_into_stage, normalize, Options};

pub fn run(args: NormalizeArgs) -> Result<()> {
    let raw = RawMacro::load(&args.input)?;
    let opts = Options {
        typing_ms: args.typing_ms,
        salt_ms: args.salt_ms,
        seed: args.seed,
    };

    // `--stage` wins; otherwise honour a stage stamped by `record --into`.
    let stage_path = args
        .stage
        .clone()
        .or_else(|| raw.meta.stage.as_deref().map(PathBuf::from));

    let score = if let Some(path) = &stage_path {
        let stage = Score::load(path)?;
        merge_into_stage(stage, &raw, &opts)
    } else {
        // Derive a name from the input file (macro.raw.toml → "macro").
        let stem = args
            .input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("demo");
        let name = stem.strip_suffix(".raw").unwrap_or(stem);
        normalize(&raw, name, &opts)
    };
    score.save(&args.output)?;

    match &stage_path {
        Some(p) => println!(
            "normalized {} into stage {} → {} ({} steps)",
            args.input.display(),
            p.display(),
            args.output.display(),
            score.timeline.len()
        ),
        None => println!(
            "normalized {} → {} ({} steps)",
            args.input.display(),
            args.output.display(),
            score.timeline.len()
        ),
    }
    Ok(())
}
