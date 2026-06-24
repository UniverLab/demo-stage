//! `demo open` — reveal a browser scene in the running capture.
//!
//! Run it inside the capture, or **from another terminal in the same directory**
//! (so it works even while a full-screen TUI owns the captured shell). It signals
//! the recorder (see [`super::control`]); the reveal is baked into the recording
//! and composited at `export` time.
//!
//! With a URL + flags it's non-interactive; with no URL (on a terminal) it runs a
//! small wizard. Running it from a second terminal keeps the prompts out of the
//! recording.

use std::io::IsTerminal;

use inquire::{Select, Text};

use crate::cli::{OpenArgs, OpenMode};
use crate::commands::control;
use crate::error::{Error, Result};

pub fn run(args: OpenArgs) -> Result<()> {
    let (url, mode, when) = resolve(args)?;

    control::send(serde_json::json!({
        "cmd": "open",
        "url": url,
        "mode": mode,
        "when": when,
    }))?;

    match &when {
        Some(pat) => println!("● will open {url} ({mode}) when output matches {pat:?}"),
        None => println!("● opening {url} ({mode})"),
    }
    Ok(())
}

/// Resolve (url, mode, when) from flags, or from the wizard when no URL is given.
fn resolve(args: OpenArgs) -> Result<(String, String, Option<String>)> {
    let mode = |a: &OpenArgs| {
        if a.split || a.mode == OpenMode::Split {
            "split"
        } else {
            "replace"
        }
        .to_string()
    };

    match &args.url {
        Some(url) if !args.wizard => Ok((url.clone(), mode(&args), args.when.clone())),
        _ => {
            if !std::io::stdin().is_terminal() {
                return Err(Error::Export(
                    "demo open needs a URL (or a terminal for the wizard)".to_string(),
                ));
            }
            wizard()
        }
    }
}

fn ask<T>(r: std::result::Result<T, inquire::InquireError>) -> Result<T> {
    r.map_err(|e| Error::Export(format!("wizard: {e}")))
}

fn wizard() -> Result<(String, String, Option<String>)> {
    println!("\n  demo open — reveal a browser scene\n");

    let url = ask(Text::new("URL:")
        .with_help_message("a repo page, a file:// PDF, http://localhost…")
        .prompt())?;

    let mode = ask(Select::new(
        "Show as:",
        vec![
            "replace — full screen (scene swap)",
            "split — beside the terminal",
        ],
    )
    .prompt())?;
    let mode = if mode.starts_with("split") {
        "split"
    } else {
        "replace"
    };

    let trigger =
        ask(Select::new("Reveal:", vec!["now", "when a line appears in the output"]).prompt())?;
    let when = if trigger.starts_with("when") {
        let pat = ask(Text::new("Cue line (a substring of the output):").prompt())?;
        let pat = pat.trim();
        (!pat.is_empty()).then(|| pat.to_string())
    } else {
        None
    };

    Ok((url.trim().to_string(), mode.to_string(), when))
}
