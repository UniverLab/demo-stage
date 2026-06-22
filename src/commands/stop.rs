//! `demo stop` — end the in-progress capture from inside it.
//!
//! `demo record` runs your shell inside a PTY and exports [`STOP_FILE_ENV`] into
//! that shell, pointing at a sentinel file it polls. Running `demo stop` there
//! creates the file, which the recorder notices and uses to end the capture —
//! a friendlier stop than typing `exit` or pressing Ctrl-D mid-demo.

use std::fs;

use crate::error::{Error, Result};

/// Env var the recorder sets on the captured shell, holding the sentinel path.
pub const STOP_FILE_ENV: &str = "DEMO_RECORD_STOPFILE";

pub fn run() -> Result<()> {
    match std::env::var(STOP_FILE_ENV) {
        Ok(path) if !path.is_empty() => {
            fs::write(&path, b"stop").map_err(|e| Error::io(&path, e))?;
            println!("● stopping recording…");
            Ok(())
        }
        _ => Err(Error::Export(
            "`demo stop` only works inside a running `demo record` session".to_string(),
        )),
    }
}
