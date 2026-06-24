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

/// A resolved reveal request: where, how, and when to open it.
struct Reveal {
    url: String,
    mode: String,
    /// Defer until this substring appears in the output.
    when: Option<String>,
    /// Defer until the current foreground command finishes.
    after: bool,
    hold_ms: Option<u64>,
    scroll: bool,
}

pub fn run(args: OpenArgs) -> Result<()> {
    // Running inside the captured shell (found via the env var, not the cwd)?
    // Then the command's echo + wizard print into the recording — tell the
    // recorder to mute from now, so it can excise them. From a second terminal
    // there's nothing in the captured shell to mute, so skip it.
    let in_session = std::env::var(control::CONTROL_ENV)
        .map(|p| !p.is_empty() && std::path::Path::new(&p).exists())
        .unwrap_or(false);
    if in_session {
        let _ = control::send(serde_json::json!({ "cmd": "open_begin" }));
    }

    let r = resolve(args)?;

    control::send(serde_json::json!({
        "cmd": "open",
        "url": r.url,
        "mode": r.mode,
        "when": r.when,
        "after": r.after,
        "hold": r.hold_ms,
        "scroll": r.scroll,
    }))?;

    let how = if r.scroll {
        format!("{}, scrolling", r.mode)
    } else {
        r.mode.clone()
    };
    if let Some(pat) = &r.when {
        println!("● will open {} ({how}) when output matches {pat:?}", r.url);
    } else if r.after {
        println!(
            "● will open {} ({how}) when the current command finishes",
            r.url
        );
    } else {
        println!("● opening {} ({how})", r.url);
    }
    Ok(())
}

/// Resolve a reveal from flags, or from the wizard when no URL is given.
fn resolve(args: OpenArgs) -> Result<Reveal> {
    let mode = |a: &OpenArgs| {
        if a.split || a.mode == OpenMode::Split {
            "split"
        } else {
            "replace"
        }
        .to_string()
    };

    match &args.url {
        Some(url) if !args.wizard => Ok(Reveal {
            url: url.clone(),
            mode: mode(&args),
            when: args.when.clone(),
            after: args.after,
            hold_ms: args.hold,
            scroll: args.scroll,
        }),
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

fn wizard() -> Result<Reveal> {
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

    let trigger = ask(Select::new(
        "Reveal:",
        vec![
            "now",
            "when the current command finishes",
            "when a line appears in the output",
        ],
    )
    .prompt())?;
    let (when, after) = if trigger.starts_with("when the current") {
        (None, true)
    } else if trigger.starts_with("when a line") {
        let pat = ask(Text::new("Cue line (a substring of the output):").prompt())?;
        let pat = pat.trim();
        ((!pat.is_empty()).then(|| pat.to_string()), false)
    } else {
        (None, false)
    };

    let scroll = ask(Select::new("Scroll the page while shown?", vec!["no", "yes"]).prompt())?
        .starts_with("yes");

    Ok(Reveal {
        url: url.trim().to_string(),
        mode: mode.to_string(),
        when,
        after,
        hold_ms: None,
        scroll,
    })
}
