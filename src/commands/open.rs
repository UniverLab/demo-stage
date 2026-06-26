//! `demo open` — reveal a browser scene in the running capture.
//!
//! Run it inside the capture, or **from another terminal in the same directory**
//! (so it works even while a full-screen TUI owns the captured shell). It signals
//! the recorder (see [`super::control`]); the reveal is baked into the recording
//! and composited at `export` time.
//!
//! With a URL + flags it's non-interactive; with no URL (on a terminal) it runs a
//! small wizard. Running it from a second terminal keeps the prompts out of the
//! recording. `--view` instead opens a real (headed) browser you drive yourself
//! and records it until you close the window.

use std::io::IsTerminal;
use std::time::{SystemTime, UNIX_EPOCH};

use inquire::{Select, Text};

use crate::cli::{OpenArgs, OpenMode};
use crate::commands::control;
use crate::error::{Error, Result};

/// Simple file path autocomplete for the open wizard.
#[derive(Clone)]
struct FilePathCompleter;

impl inquire::autocompletion::Autocomplete for FilePathCompleter {
    fn get_suggestions(
        &mut self,
        input: &str,
    ) -> std::result::Result<Vec<String>, inquire::CustomUserError> {
        let input = if input.is_empty() { "./" } else { input };
        let (dir, prefix) = if input.ends_with('/') {
            (input.to_string(), "")
        } else {
            let p = std::path::Path::new(input);
            let dir = p.parent().unwrap_or(std::path::Path::new("."));
            let prefix = p.file_name().and_then(|f| f.to_str()).unwrap_or("");
            (dir.to_string_lossy().to_string(), prefix)
        };
        let entries = std::fs::read_dir(&dir).ok();
        let mut results = Vec::new();
        if let Some(entries) = entries {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(prefix) {
                    let full = if dir == "./" || dir == "." {
                        name.clone()
                    } else {
                        format!("{}/{}", dir.trim_end_matches('/'), name)
                    };
                    let full = if entry.path().is_dir() {
                        format!("{full}/")
                    } else {
                        full
                    };
                    results.push(full);
                }
            }
        }
        results.sort();
        Ok(results)
    }

    fn get_completion(
        &mut self,
        input: &str,
        suggestion: Option<String>,
    ) -> std::result::Result<Option<String>, inquire::CustomUserError> {
        Ok(suggestion.or_else(|| {
            let mut suggestions = self.get_suggestions(input).ok()?;
            if suggestions.len() == 1 {
                Some(suggestions.remove(0))
            } else {
                None
            }
        }))
    }
}

/// Monospace cell size assumed by the renderer (matches `export::recording`), so a
/// `--view` recording is sized to the terminal canvas it'll be composited onto.
const CELL_W: u32 = 10;
const CELL_H: u32 = 20;

/// A resolved reveal request: where, how, and when to open it.
struct Reveal {
    url: String,
    name: Option<String>,
    mode: String,
    /// Defer until this substring appears in the output.
    when: Option<String>,
    /// Defer until the current foreground command finishes.
    after: bool,
    hold_ms: Option<u64>,
    scroll: bool,
    /// Open a headed browser, navigate live, and record until the window closes.
    view: bool,
    /// Emulated colour scheme (`light`/`dark`), or `None` for the page default.
    theme: Option<String>,
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

    if r.view {
        return run_view(&r);
    }

    // Print the confirmation BEFORE signalling the recorder: in-session, the
    // control command closes the mute span, so a line printed after it would leak
    // into the demo. Printed before, it's still inside the muted span (excised).
    let how = if r.scroll {
        format!("{}, scrolling", r.mode)
    } else if let Some(ms) = r.hold_ms {
        format!("{}, hold {ms}ms", r.mode)
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

    control::send(serde_json::json!({
        "cmd": "open",
        "url": r.url,
        "name": r.name,
        "mode": r.mode,
        "when": r.when,
        "after": r.after,
        "hold": r.hold_ms,
        "scroll": r.scroll,
        "theme": r.theme,
    }))?;
    Ok(())
}

/// `--view`: open a headed browser, record the user's session to a frames dir,
/// then reveal that pre-recorded scene (no headless Chromium needed at export).
fn run_view(r: &Reveal) -> Result<()> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let (w, h) = (cols as u32 * CELL_W, rows as u32 * CELL_H);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let dir = format!("demo-scenes/scene-{ts}");

    let n = crate::export::browser::record_view(
        &r.url,
        w,
        h,
        r.theme.as_deref(),
        std::path::Path::new(&dir),
    )?;
    if n == 0 {
        return Err(Error::Export(
            "no frames were recorded (the browser window closed before anything rendered)"
                .to_string(),
        ));
    }
    let hold_ms = (n as u64 * 1000) / crate::export::browser::VIEW_FPS as u64;

    // Confirm before signalling (see `run`): keeps the line out of the recording.
    println!("● recorded {n} frames → {dir} ({hold_ms} ms scene)");
    control::send(serde_json::json!({
        "cmd": "open",
        "url": crate::model::view_frames_url(&dir),
        "mode": "replace",
        "when": serde_json::Value::Null,
        "after": false,
        "hold": hold_ms,
        "scroll": false,
    }))?;
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
            url: normalize_url(url),
            name: None,
            mode: mode(&args),
            when: args.when.clone(),
            after: args.after,
            hold_ms: args.hold,
            scroll: args.scroll,
            view: args.view,
            theme: args.theme.map(|t| t.as_str().to_string()),
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

    let source = ask(Select::new(
        "Source:",
        vec!["URL (web page, localhost)", "Local file (PDF, PNG, HTML)"],
    )
    .prompt())?;

    let url = if source.starts_with("Local") {
        let path = ask(inquire::Text::new("File path:")
            .with_help_message("relative or absolute path to a local file")
            .with_autocomplete(FilePathCompleter)
            .prompt())?;
        let abs = std::fs::canonicalize(path.trim())
            .map_err(|e| Error::Export(format!("file not found: {e}")))?;
        format!("file://{}", abs.display())
    } else {
        let raw = ask(Text::new("URL:")
            .with_help_message("a repo page, http://localhost…")
            .prompt())?;
        normalize_url(&raw)
    };

    let scene_name = ask(inquire::Text::new("Scene name:")
        .with_help_message("identifier for this scene (e.g. 'browser', 'preview')")
        .with_default("browser")
        .prompt())?;
    let scene_name = scene_name.trim().to_string();

    let theme = ask(Select::new("Browser theme:", vec!["default", "light", "dark"]).prompt())?;
    let theme = match theme {
        "light" => Some("light".to_string()),
        "dark" => Some("dark".to_string()),
        _ => None,
    };

    // How to present it — a static hold, a scroll, or an interactive view. These
    // are mutually exclusive, so they're one question.
    let behavior = ask(Select::new(
        "Show it as:",
        vec![
            "Static — hold for a few seconds",
            "Scroll the page down (pan)",
            "Interactive — open a real browser, navigate, record until you close it",
        ],
    )
    .prompt())?;
    let view = behavior.starts_with("Interactive");
    let scroll = behavior.starts_with("Scroll");
    let hold_ms = if behavior.starts_with("Static") {
        // Reject non-numbers instead of silently defaulting (a stray letter would
        // otherwise pass through as the default).
        let secs = ask(Text::new("Hold for how many seconds?")
            .with_default("6")
            .with_validator(|s: &str| {
                let s = s.trim();
                match s.parse::<f64>() {
                    Ok(n) if n > 0.0 => Ok(inquire::validator::Validation::Valid),
                    _ => Ok(inquire::validator::Validation::Invalid(
                        "enter a positive number of seconds (e.g. 6)".into(),
                    )),
                }
            })
            .prompt())?;
        let secs: f64 = secs.trim().parse().unwrap_or(6.0);
        Some((secs.max(0.5) * 1000.0) as u64)
    } else {
        None
    };

    // An interactive view always takes over the whole frame and opens immediately.
    if view {
        return Ok(Reveal {
            url: normalize_url(url.trim()),
            name: Some(scene_name.clone()),
            mode: "replace".to_string(),
            when: None,
            after: false,
            hold_ms: None,
            scroll: false,
            view: true,
            theme,
        });
    }

    let mode = ask(Select::new(
        "Place it:",
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

    Ok(Reveal {
        url: normalize_url(url.trim()),
        name: Some(scene_name),
        mode: mode.to_string(),
        when,
        after,
        hold_ms,
        scroll,
        view: false,
        theme,
    })
}

/// Ensure a URL has a protocol prefix. Bare domains like `google.com` become
/// `https://google.com`; `file://`, `http://`, `https://` are left as-is.
fn normalize_url(url: &str) -> String {
    let u = url.trim();
    if u.contains("://") {
        u.to_string()
    } else if u.starts_with("localhost") || u.starts_with("127.0.0.1") {
        format!("http://{u}")
    } else {
        format!("https://{u}")
    }
}
