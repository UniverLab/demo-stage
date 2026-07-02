//! `demo focus` — switch the live capture's view to one or two sources.
//!
//! A *live* control command (like `demo stop`/`demo open`): run it inside the
//! captured shell, or from **another terminal in the same directory** (so it
//! works even while a full-screen TUI owns the captured terminal). It signals the
//! running recorder, which records the view switch at the live moment.
//!
//! `demo focus main` shows just the terminal; `demo focus docs` a browser source
//! full-screen; `demo focus main docs` the two side by side (`--vertical` stacks
//! them). Timing/behaviour flags — `--hold`, `--scroll`, `--when`, `--after` —
//! defer or shape the reveal. With no source on a terminal it runs a wizard.

use std::io::IsTerminal;

use inquire::{MultiSelect, Select};

use crate::cli::FocusArgs;
use crate::commands::control;
use crate::error::{Error, Result};
use crate::model::{Source, SourceKind};

pub fn run(args: FocusArgs) -> Result<()> {
    // Live command: it only makes sense while a capture is running. `find` gives
    // the shared "no capture in progress" error (also used by `demo stop`).
    control::find()?;
    let sources = control::read_sources();

    let (chosen, orientation) = if args.sources.is_empty() {
        if !std::io::stdin().is_terminal() {
            return Err(Error::Export(
                "demo focus needs a source (e.g. `demo focus main`), or a terminal for the wizard"
                    .to_string(),
            ));
        }
        wizard(&sources, &args)?
    } else {
        (args.sources.clone(), orientation_flag(&args))
    };

    if chosen.len() > 2 {
        return Err(Error::Export(
            "demo focus shows at most two sources at once".to_string(),
        ));
    }

    // Resolve each id to a reveal pane (terminal, or a browser source's URL).
    let panes: Vec<serde_json::Value> = chosen
        .iter()
        .map(|id| resolve_pane(id, &sources, args.theme.as_deref()))
        .collect::<Result<_>>()?;

    // In-session, mute this command's echo/wizard from now (from another terminal
    // there's nothing in the captured shell to mute).
    if in_session() {
        let _ = control::send(serde_json::json!({ "cmd": "reveal_begin" }));
    }

    let label = chosen.join(if orientation == "vertical" {
        " / "
    } else {
        " | "
    });
    println!("● focus → {label}");
    control::send(serde_json::json!({
        "cmd": "reveal",
        "panes": panes,
        "orientation": orientation,
        "hold": args.hold.map(|s| (s.max(0.0) * 1000.0) as u64),
        "scroll": args.scroll,
        "when": args.when,
        "after": args.after,
    }))?;
    Ok(())
}

/// Resolve a source id to a reveal-pane JSON object. `main`/`terminal` is the
/// terminal (no URL); anything else must be a browser source from the capture.
fn resolve_pane(
    id: &str,
    sources: &[Source],
    theme_override: Option<&str>,
) -> Result<serde_json::Value> {
    if id == "main" || id == "terminal" {
        return Ok(serde_json::json!({ "id": "main" }));
    }
    match sources.iter().find(|s| s.id == id) {
        Some(s) if s.kind == SourceKind::Browser => Ok(serde_json::json!({
            "id": s.id,
            "url": s.url,
            "theme": theme_override.map(str::to_string).or_else(|| s.theme.clone()),
        })),
        Some(_) => Ok(serde_json::json!({ "id": "main" })), // a terminal source
        None => Err(Error::Export(format!(
            "unknown source '{id}' — configure it in `demo capture`, or use `demo open <url>` for an ad-hoc page{}",
            source_hint(sources)
        ))),
    }
}

/// `--vertical`/`--horizontal` → the orientation string (default horizontal).
fn orientation_flag(args: &FocusArgs) -> String {
    if args.vertical {
        "vertical"
    } else {
        "horizontal"
    }
    .to_string()
}

/// True when run inside the captured shell (found via the env var, not the cwd).
fn in_session() -> bool {
    std::env::var(control::CONTROL_ENV)
        .map(|p| !p.is_empty() && std::path::Path::new(&p).exists())
        .unwrap_or(false)
}

/// A hint listing the configured source ids, for error messages.
fn source_hint(sources: &[Source]) -> String {
    let ids: Vec<&str> = sources.iter().map(|s| s.id.as_str()).collect();
    if ids.is_empty() {
        String::new()
    } else {
        format!(" (sources: {})", ids.join(", "))
    }
}

/// Pick 1–2 sources (and orientation for two) from the capture's sources.
fn wizard(sources: &[Source], args: &FocusArgs) -> Result<(Vec<String>, String)> {
    println!("\n  demo focus — switch the view\n");
    // "main" (the terminal) is always available, plus any browser sources.
    let mut ids: Vec<String> = vec!["main".to_string()];
    ids.extend(
        sources
            .iter()
            .filter(|s| s.kind == SourceKind::Browser)
            .map(|s| s.id.clone()),
    );
    if ids.len() == 1 {
        println!("  (this capture has no browser sources — only the terminal.");
        println!("   configure them when starting `demo capture`, or reveal an ad-hoc page with `demo open <url>`)\n");
    }

    let chosen = ask(MultiSelect::new("Show (pick one or two):", ids)
        .with_help_message("space toggles, enter accepts")
        .prompt())?;
    if chosen.is_empty() {
        return Err(Error::Export("pick at least one source".to_string()));
    }
    if chosen.len() > 2 {
        return Err(Error::Export("pick at most two sources".to_string()));
    }

    let orientation = if chosen.len() == 2 {
        let pick =
            ask(Select::new("Arrange:", vec!["side by side", "stacked (top/bottom)"]).prompt())?;
        if pick.starts_with("stacked") {
            "vertical"
        } else {
            "horizontal"
        }
        .to_string()
    } else {
        orientation_flag(args)
    };
    Ok((chosen, orientation))
}

fn ask<T>(r: std::result::Result<T, inquire::InquireError>) -> Result<T> {
    r.map_err(|e| Error::Export(format!("wizard: {e}")))
}
