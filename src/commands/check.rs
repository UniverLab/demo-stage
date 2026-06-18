//! `demo check` — statically validate a demo score.

use std::process::ExitCode;

use crate::cli::CheckArgs;
use crate::error::Result;
use crate::model::Score;
use crate::validate::validate;

pub fn run(args: CheckArgs) -> Result<ExitCode> {
    let score = Score::load(&args.input)?;
    let problems = validate(&score);
    let path = args.input.display();

    if problems.is_empty() {
        println!("ok: {path} is valid");
        return Ok(ExitCode::SUCCESS);
    }

    eprintln!("{path}: {} problem(s) found:", problems.len());
    for problem in &problems {
        eprintln!("  - {problem}");
    }
    Ok(ExitCode::FAILURE)
}
