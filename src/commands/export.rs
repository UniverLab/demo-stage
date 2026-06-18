//! `demo export` — compile a demo score to a target format.

use crate::cli::ExportArgs;
use crate::error::{Error, Result};

pub fn run(_args: ExportArgs) -> Result<()> {
    Err(Error::Unimplemented("export"))
}
