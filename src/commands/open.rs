//! `demo open` — reveal a browser scene in the running capture.
//!
//! Run it inside the capture, or **from another terminal in the same directory**
//! (so it works even while a full-screen TUI owns the captured shell). It signals
//! the recorder (see [`super::control`]); the reveal is baked into the recording
//! and composited at `export` time.

use crate::cli::{OpenArgs, OpenMode};
use crate::commands::control;
use crate::error::Result;

pub fn run(args: OpenArgs) -> Result<()> {
    let mode = if args.split {
        "split"
    } else {
        match args.mode {
            OpenMode::Replace => "replace",
            OpenMode::Split => "split",
        }
    };

    control::send(serde_json::json!({
        "cmd": "open",
        "url": args.url,
        "mode": mode,
        "when": args.when,
    }))?;

    match &args.when {
        Some(pat) => println!(
            "● will open {} ({mode}) when output matches {pat:?}",
            args.url
        ),
        None => println!("● opening {} ({mode})", args.url),
    }
    Ok(())
}
