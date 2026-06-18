//! `demo check` — statically validate a demo score.

use std::process::ExitCode;

use crate::cli::CheckArgs;
use crate::error::{Error, Result};

pub fn run(_args: CheckArgs) -> Result<ExitCode> {
    Err(Error::Unimplemented("check"))
}
