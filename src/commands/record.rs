//! `demo record` — capture an interactive session into a raw macro.

use crate::cli::RecordArgs;
use crate::error::{Error, Result};

pub fn run(_args: RecordArgs) -> Result<()> {
    Err(Error::Unimplemented("record"))
}
