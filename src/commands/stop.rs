//! `demo stop` — end the in-progress capture, from inside it or another shell.
//!
//! Signals the running recorder via the control file (see [`super::control`]),
//! so it works both inside the captured session and from another terminal in the
//! same directory.

use crate::commands::control;
use crate::error::Result;

pub fn run() -> Result<()> {
    control::send(serde_json::json!({ "cmd": "stop" }))?;
    println!("● stopping recording…");
    Ok(())
}
