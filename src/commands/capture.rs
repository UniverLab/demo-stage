//! `demo capture` — record a live interactive session into a raw macro, then
//! normalize it into a clean demo score.
//!
//! Spawns a shell in a PTY and bridges it to the real terminal (raw mode):
//! local stdin → PTY (recorded as `input` events), PTY → local stdout
//! (recorded as `output` events), each timestamped. The capture ends when the
//! shell exits or after `idle_timeout_ms` with no terminal output (SPEC §3.3).

use std::io::{IsTerminal, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::cli::CaptureArgs;
use crate::commands::control;
use crate::error::{Error, Result};
use crate::export::local_server::{self, LocalServer};
use crate::export::recording;
use crate::export::run::{is_zsh, sh_single_quote};
use crate::file_picker::{pick_local_file, BrowseRoots};
use crate::model::{DemoMeta, Orientation, RawEvent, RawMacro, RawMeta, RevealPane, Score};
use crate::normalize::{merge_into_stage, normalize, Options};
use crate::paths::{file_url_absolute, repair_browser_url};

/// Marker the captured shell echoes once it's at our forced prompt — recording
/// starts after it, so the prompt-setup chatter is discarded. Assembled by the
/// shell so the typed command doesn't itself match (only the printed output).
const PROMPT_READY: &str = "demostage_capture_ready";

/// A resolved reveal requested during capture (via `demo focus` or `demo open`):
/// the 1–2 panes to show and how they're arranged, plus hold/scroll. The
/// `--when`/`--after` deferral is handled at the control layer, so by the time a
/// `Reveal` is recorded it fires *now*.
#[derive(Clone)]
struct Reveal {
    panes: Vec<crate::model::RevealPane>,
    orientation: crate::model::Orientation,
    hold_ms: Option<u64>,
    scroll: bool,
}

impl Reveal {
    /// Turn this reveal into the recorded event at time `t_ms`.
    fn to_event(&self, t_ms: u64) -> RawEvent {
        RawEvent::Reveal {
            t_ms,
            panes: self.panes.clone(),
            orientation: self.orientation,
            hold_ms: self.hold_ms,
            scroll: self.scroll,
        }
    }
    /// One-line summary for the debug log.
    fn summary(&self) -> String {
        let ids: Vec<&str> = self.panes.iter().map(|p| p.id.as_str()).collect();
        format!("{:?} ({:?})", ids, self.orientation)
    }
}

/// Reveals armed by `--when <pat>`, each with its cue pattern.
type PendingWhen = Arc<Mutex<Vec<(Reveal, String)>>>;
/// Reveals armed by `--after`, fired when the running command finishes.
type PendingAfter = Arc<Mutex<Vec<Reveal>>>;

/// How long the output must stay quiet, after a command produced output, before
/// an `--after` reveal fires (i.e. the shell is back at the prompt).
const AFTER_QUIET_MS: u64 = 800;

/// When a `demo open` wizard's start has to be inferred from its `open_begin`
/// marker (input detection missed the typed command), back-date the excision span
/// by this much so the wizard's first lines (already printed) are still removed.
const OPEN_BEGIN_BACKDATE_MS: u64 = 800;

fn ms(t0: Instant) -> u64 {
    t0.elapsed().as_millis() as u64
}

/// If muting has been on for more than 90 seconds, close the stranded mute span,
/// emit a diagnostic, and return true. Otherwise return false.
fn maybe_close_safety_valve(
    muting: &AtomicBool,
    mute_since: &Mutex<Instant>,
    mute_start: &Mutex<Option<u64>>,
    mute_spans: &Mutex<Vec<(u64, u64)>>,
    t0: Instant,
    debug: Option<&DebugLog>,
) -> bool {
    if !muting.load(Ordering::SeqCst) {
        return false;
    }
    if mute_since.lock().unwrap().elapsed() <= Duration::from_secs(90) {
        return false;
    }
    muting.store(false, Ordering::SeqCst);
    if let Some(start) = mute_start.lock().unwrap().take() {
        mute_spans.lock().unwrap().push((start, ms(t0)));
    }
    eprintln!(
        "⚠ safety valve: mute span closed after 90s — a meta-command (demo focus/open) failed to report back"
    );
    if let Some(d) = debug {
        d.note("safety valve: 90s mute span closed — meta-command did not report back");
    }
    true
}

/// Track the current terminal line and detect a secret prompt at each line
/// boundary (`\r` redraw or `\n`) — crucially BEFORE the boundary clears the line.
/// `inquire` emits the prompt immediately followed by `\r` (`Vault passphrase:\r…`),
/// so checking only at the chunk end (after the `\r` cleared it) missed it and the
/// secret leaked. Latches `sensitive` and records the prompt label when found.
/// When a completed non-secret line is seen, sets `secret_prompt_cleared` to
/// signal the input thread that the dedup guard can be cleared. Only set on
/// completed lines (at `\n`/`\r`), not on partial lines at chunk boundaries,
/// to avoid spurious clears when a prompt label is split across PTY reads.
fn track_and_detect(
    line: &mut String,
    text: &str,
    sensitive: &AtomicBool,
    secret_prompt: &Mutex<Option<String>>,
    secret_prompt_cleared: &AtomicBool,
) {
    let detect_secret = |line: &str| {
        if is_secret_prompt(line) {
            sensitive.store(true, Ordering::SeqCst);
            *secret_prompt.lock().unwrap() = Some(clean_prompt(line));
        }
    };
    let mark_cleared = |line: &str| {
        if !is_secret_prompt(line) && !line.is_empty() {
            secret_prompt_cleared.store(true, Ordering::SeqCst);
        }
    };
    for ch in text.chars() {
        match ch {
            '\n' | '\r' => {
                detect_secret(line);
                mark_cleared(line);
                line.clear();
            }
            c if c.is_control() => {}
            c => {
                // Full-screen TUIs paint without newlines, so the "line" can grow
                // without bound. A real secret prompt is short — past the cap this
                // is screen paint, not a prompt; stop growing (is_secret_prompt
                // rejects anything this long anyway).
                if line.len() < MAX_PROMPT_LINE {
                    line.push(c);
                }
            }
        }
    }
    // The prompt may sit at the chunk end with no trailing newline yet.
    // Only detect secrets here; don't set the cleared flag on partial lines.
    detect_secret(line);
}

/// Longest a terminal line can be and still count as a secret prompt. Real
/// prompts ("Vault passphrase:") are far shorter; anything bigger is a TUI
/// repainting the screen without newlines.
const MAX_PROMPT_LINE: usize = 256;

/// Tidy a captured prompt line into a label for `demo record` to show: drop ANSI
/// CSI residue left after the ESC was stripped (`[..m` colours, `[?25h` cursor
/// codes, etc.) and the leading prompt glyphs (`? > ◆ ●`), leaving e.g.
/// `Vault passphrase:`.
fn clean_prompt(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '[' {
            // A CSI residue (ESC already stripped): `[` then params `0-9;?` then a
            // letter final byte. Drop it; otherwise keep the literal `[`.
            let mut params = String::new();
            let mut final_letter = None;
            while let Some(&n) = chars.peek() {
                if n.is_ascii_digit() || n == ';' || n == '?' {
                    params.push(n);
                    chars.next();
                } else if n.is_ascii_alphabetic() {
                    final_letter = Some(n);
                    chars.next();
                    break;
                } else {
                    break;
                }
            }
            if final_letter.is_some() {
                continue;
            }
            out.push('[');
            out.push_str(&params);
        } else {
            out.push(c);
        }
    }
    out.trim()
        .trim_start_matches(['?', '>', '◆', '●', '*', ' '])
        .trim()
        .to_string()
}

/// Decide whether a submitted secret prompt should produce a `Secret` event.
/// Returns true when the caller must record one.
///
/// `prompt_left_screen` is true when the output thread has observed a completed
/// non-secret line since the last submission, meaning the prompt has left the
/// screen and the dedup guard should be cleared before checking.
///
/// The dedup covers only redraws of the prompt currently being answered: once
/// `prompt_left_screen` is true the guard is cleared, so the same prompt text
/// detected later records a new event. Two consecutive detections of the same
/// prompt with no submission in between still collapse into one.
fn secret_step_on_submit(
    last_secret_prompt: &mut Option<String>,
    prompt_left_screen: bool,
    prompt: &str,
) -> bool {
    if prompt_left_screen {
        *last_secret_prompt = None;
    }
    let is_dup = last_secret_prompt.as_ref().map(|s| s.as_str()) == Some(prompt);
    if !is_dup {
        *last_secret_prompt = Some(prompt.to_string());
        true
    } else {
        false
    }
}

/// Heuristic: does this line look like a program prompting for a secret? Matches
/// a secret keyword on a line that ends like a prompt (`:` or `?`), so a typed
/// command that merely mentions "password" is not mistaken for a prompt. Long
/// lines are rejected outright — they're TUI screen paint, not prompts.
fn is_secret_prompt(line: &str) -> bool {
    if line.len() > 200 {
        return false;
    }
    let lower = line.to_ascii_lowercase();
    let trimmed = lower.trim_end();
    if !(trimmed.ends_with(':') || trimmed.ends_with('?')) {
        return false;
    }
    const HINTS: [&str; 10] = [
        "password",
        "passphrase",
        "passcode",
        "secret",
        "[sudo]",
        "verification code",
        "token",
        "api key",
        "access key",
        "credential",
    ];
    HINTS.iter().any(|h| trimmed.contains(h))
}

/// A timestamped diagnostic log written when `record --debug` is set. Both I/O
/// threads write to it, so it lives behind a mutex.
struct DebugLog {
    file: Mutex<std::fs::File>,
    t0: Instant,
}

impl DebugLog {
    fn create(path: &std::path::Path, t0: Instant) -> Result<Self> {
        let file = std::fs::File::create(path).map_err(|e| Error::io(path, e))?;
        Ok(DebugLog {
            file: Mutex::new(file),
            t0,
        })
    }

    /// Write one timestamped line.
    fn note(&self, msg: &str) {
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "[+{:>8}ms] {msg}", self.t0.elapsed().as_millis());
            let _ = f.flush();
        }
    }

    /// Log a byte chunk in both escaped-text and hex form — the escaped form
    /// makes control bytes (arrows = `\u{1b}[A`, etc.) visible at a glance.
    fn chunk(&self, dir: &str, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        let hex: String = bytes.iter().map(|b| format!("{b:02x} ")).collect();
        self.note(&format!(
            "{dir} {:>4}B  repr={text:?}  hex=[{}]",
            bytes.len(),
            hex.trim_end()
        ));
    }
}

/// Escape a user-entered label for safe embedding in a bash `PS1`: a literal
/// backslash must be doubled, else bash reads it as a prompt escape (`\d` = date,
/// `\w` = cwd, …) — e.g. a PowerShell path `C:\Users` would otherwise mangle.
fn ps1_text(s: &str) -> String {
    s.replace('\\', "\\\\")
}

/// Build a prompt string with coloured segments for the detected shell.
fn colored(shell: &str, ansi: &str, zsh_col: &str, text: &str) -> String {
    if is_zsh(shell) {
        format!("%B%F{{{zsh_col}}}{text}%f%b")
    } else {
        format!("\\[\\e[{ansi}m\\]{text}\\[\\e[0m\\]")
    }
}

/// Default prompt for the given shell.
pub fn default_prompt(shell: &str) -> String {
    let user = colored(shell, "1;32", "green", "user@demo");
    let path = colored(shell, "1;34", "blue", "~");
    format!("{user}:{path}$ ")
}

/// Decide the captured shell's prompt: `--keep-prompt` keeps yours, `--prompt`
/// forces a given `PS1`, and with neither a quick wizard offers ready-made styles
/// (you edit only the label text; colours are chosen for you). Returns
/// `(force_prompt, ps1)`.
fn choose_prompt(args: &CaptureArgs, shell: &str) -> Result<(bool, String)> {
    if args.keep_prompt {
        return Ok((false, default_prompt(shell)));
    }
    if let Some(p) = &args.prompt {
        return Ok((true, p.clone()));
    }

    let style = inquire::Select::new(
        "Prompt style for this demo:",
        vec![
            "Linux        user@host:~$",
            "macOS        user@host ~ %",
            "PowerShell   PS path>",
            "Minimal      ❯",
            "Keep my real prompt",
        ],
    )
    .prompt()
    .map_err(|e| Error::Export(format!("prompt wizard: {e}")))?;

    // Ask one editable text with a sensible default (blank keeps the default).
    let ask = |q: &str, default: &str| -> Result<String> {
        let v = inquire::Text::new(q)
            .with_default(default)
            .prompt()
            .map_err(|e| Error::Export(format!("prompt wizard: {e}")))?;
        let v = v.trim();
        Ok(if v.is_empty() {
            default.to_string()
        } else {
            v.to_string()
        })
    };

    // Colours are baked into each template; the user only fills the text.
    let ps1 = if style.starts_with("Linux") {
        let l = ps1_text(&ask("Text (user@host):", "user@demo")?);
        let user = colored(shell, "1;32", "green", &l);
        let path = colored(shell, "1;34", "blue", "~");
        format!("{user}:{path}$ ")
    } else if style.starts_with("macOS") {
        let l = ps1_text(&ask("Text (user@host):", "user@mac")?);
        let user = colored(shell, "1;36", "cyan", &l);
        format!("{user} ~ % ")
    } else if style.starts_with("PowerShell") {
        let p = ps1_text(&ask("Path:", "C:\\Users\\demo")?);
        let path = colored(shell, "1;36", "cyan", &p);
        format!("PS {path}> ")
    } else if style.starts_with("Minimal") {
        let s = ps1_text(&ask("Symbol:", "❯")?);
        let sym = colored(shell, "1;32", "green", &s);
        format!("{sym} ")
    } else {
        return Ok((false, default_prompt(shell)));
    };
    Ok((true, ps1))
}

/// Choose the export font. `--font` skips the wizard; otherwise a picker
/// offers the bundled options.
fn choose_font(args: &CaptureArgs) -> Result<String> {
    if let Some(f) = &args.font {
        return Ok(f.clone());
    }
    if !std::io::stdin().is_terminal() {
        return Ok(crate::fonts::DEFAULT_FONT.to_string());
    }
    let choice = inquire::Select::new("Export font:", crate::fonts::FONT_NAMES.to_vec())
        .prompt()
        .map_err(|e| Error::Export(format!("font wizard: {e}")))?;
    Ok(crate::fonts::parse_font_name(choice).to_string())
}

/// Legacy named resolution presets, accepted as `--resolution` aliases. Each
/// maps onto an aspect-ratio × quality pair under the new scheme (e.g.
/// `landscape` = `16:9` × `fullhd`, `standard` = `16:9` × `hd`).
const RESOLUTIONS: [(&str, u32, u32); 4] = [
    ("landscape", 1920, 1080),
    ("portrait", 1080, 1920),
    ("square", 1080, 1080),
    ("standard", 1280, 720),
];

/// Aspect ratios offered at capture. `a:b` means width:height = a:b; the canvas
/// is scaled so its short side matches the quality base.
const ASPECTS: [(&str, u32, u32); 4] = [
    ("16:9", 16, 9),
    ("9:16", 9, 16),
    ("4:3", 4, 3),
    ("1:1", 1, 1),
];

/// Quality tiers — the short side of the canvas, in pixels.
const QUALITIES: [(&str, u32); 2] = [("fullhd", 1080), ("hd", 720)];

/// Default frame rate (matches `[layout] fps` default in the score model).
const DEFAULT_FPS: u32 = 15;

/// Permitted frame rates for the exported gif/mp4.
const FPS_CHOICES: [u32; 3] = [15, 24, 30];

/// Compute the canvas `(width, height)` for an aspect ratio + quality. The
/// short side is the quality base; the long side scales by the ratio. Every
/// combination lands on integer pixels (1080 and 720 are divisible by 9, 3, 1).
fn canvas_from_aspect_quality(aspect: &str, quality: &str) -> Result<(u32, u32)> {
    let av = aspect.trim().to_ascii_lowercase();
    let &(_, a, b) = ASPECTS
        .iter()
        .find(|(name, ..)| *name == av)
        .ok_or_else(|| {
            Error::Export(format!(
                "invalid aspect '{aspect}' — try 16:9, 9:16, 4:3, or 1:1"
            ))
        })?;
    let qv = quality.trim().to_ascii_lowercase();
    let base = QUALITIES
        .iter()
        .find(|(name, _)| *name == qv)
        .map(|(_, b)| *b)
        .ok_or_else(|| Error::Export(format!("invalid quality '{quality}' — try fullhd or hd")))?;
    let short = a.min(b);
    Ok((a * base / short, b * base / short))
}

/// Parse a `--resolution` value: a legacy preset name, `WxH`, or `auto` (→
/// `None`, meaning the canvas derives from the terminal size).
fn parse_resolution(s: &str) -> Result<Option<(u32, u32)>> {
    let v = s.trim().to_ascii_lowercase();
    if v == "auto" {
        return Ok(None);
    }
    if let Some(&(_, w, h)) = RESOLUTIONS.iter().find(|(name, ..)| *name == v) {
        return Ok(Some((w, h)));
    }
    if let Some((w, h)) = v.split_once(['x', '×']) {
        if let (Ok(w), Ok(h)) = (w.trim().parse::<u32>(), h.trim().parse::<u32>()) {
            if w > 0 && h > 0 {
                return Ok(Some((w, h)));
            }
        }
    }
    Err(Error::Export(format!(
        "invalid resolution '{s}' — try a WxH pair (e.g. 1600x900) or auto; or use --aspect/--quality"
    )))
}

/// Parse a `--fps` value: must be one of 15, 24, 30.
fn parse_fps(s: &str) -> Result<u32> {
    let n: u32 = s
        .trim()
        .parse()
        .map_err(|_| Error::Export(format!("invalid fps '{s}' — try 15, 24, or 30")))?;
    if FPS_CHOICES.contains(&n) {
        Ok(n)
    } else {
        Err(Error::Export(format!(
            "unsupported fps {n} — try 15, 24, or 30"
        )))
    }
}

/// Canvas for the export: `--resolution` (explicit/auto) wins, else
/// `--aspect`×`--quality`, else a wizard. `None` = auto (derive from the
/// terminal size) — also the non-interactive default.
fn choose_canvas(args: &CaptureArgs) -> Result<Option<(u32, u32)>> {
    if let Some(r) = &args.resolution {
        return parse_resolution(r);
    }
    if let Some(a) = &args.aspect {
        let q = args.quality.as_deref().unwrap_or("fullhd");
        return Ok(Some(canvas_from_aspect_quality(a, q)?));
    }
    if let Some(q) = &args.quality {
        return Ok(Some(canvas_from_aspect_quality("16:9", q)?));
    }
    if !std::io::stdin().is_terminal() {
        return Ok(None);
    }
    let choice = inquire::Select::new(
        "Aspect ratio:",
        vec![
            "16:9   (widescreen)",
            "9:16   (portrait / vertical)",
            "4:3    (classic)",
            "1:1    (square)",
            "Auto   (derive from terminal size)",
            "Custom (enter WxH)",
        ],
    )
    .prompt()
    .map_err(|e| Error::Export(format!("aspect wizard: {e}")))?;

    let av = choice.to_ascii_lowercase();
    if av.starts_with("auto") {
        return Ok(None);
    }
    if av.starts_with("custom") {
        let w = inquire::Text::new("width:")
            .with_default("1920")
            .prompt()
            .map_err(|e| Error::Export(format!("aspect wizard: {e}")))?;
        let h = inquire::Text::new("height:")
            .with_default("1080")
            .prompt()
            .map_err(|e| Error::Export(format!("aspect wizard: {e}")))?;
        return parse_resolution(&format!("{}x{}", w.trim(), h.trim()));
    }
    let ratio = av.split_whitespace().next().unwrap_or(&av);
    let quality = inquire::Select::new("Quality:", vec!["FullHD  (1080p)", "HD      (720p)"])
        .prompt()
        .map_err(|e| Error::Export(format!("quality wizard: {e}")))?;
    let q = if quality.to_ascii_lowercase().starts_with("full") {
        "fullhd"
    } else {
        "hd"
    };
    canvas_from_aspect_quality(ratio, q).map(Some)
}

/// Frame rate for the export: `--fps`, else a wizard. Defaults to
/// [`DEFAULT_FPS`] when non-interactive.
fn choose_fps(args: &CaptureArgs) -> Result<u32> {
    if let Some(f) = args.fps {
        return parse_fps(&f.to_string());
    }
    if !std::io::stdin().is_terminal() {
        return Ok(DEFAULT_FPS);
    }
    let choice = inquire::Select::new("Frame rate:", vec!["15 fps", "24 fps", "30 fps"])
        .prompt()
        .map_err(|e| Error::Export(format!("fps wizard: {e}")))?;
    choice
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<u32>()
        .map_err(|e| Error::Export(format!("fps wizard: {e}")))
}

/// Ask if the user wants to add browser sources, loop through adding them.
fn choose_sources(
    launch_dir: &std::path::Path,
    shell_dir: &std::path::Path,
) -> Result<(Vec<crate::model::Source>, Vec<LocalServer>)> {
    if !std::io::stdin().is_terminal() {
        return Ok((vec![], vec![]));
    }
    let add = inquire::Confirm::new("Add browser sources? (repo pages, docs, localhost)")
        .with_default(false)
        .prompt()
        .map_err(|e| Error::Export(format!("source wizard: {e}")))?;
    if !add {
        return Ok((vec![], vec![]));
    }
    let mut sources = vec![crate::model::Source {
        id: "main".to_string(),
        kind: crate::model::SourceKind::Terminal,
        url: None,
        theme: None,
    }];
    let mut servers = Vec::new();
    let roots = BrowseRoots {
        launch_dir: launch_dir.to_path_buf(),
        shell_dir: shell_dir.to_path_buf(),
    };
    loop {
        let id = inquire::Text::new("Source ID:")
            .with_help_message("unique name (e.g. 'github', 'docs', 'preview')")
            .prompt()
            .map_err(|e| Error::Export(format!("source wizard: {e}")))?;
        let id = id.trim().to_string();
        if id.is_empty() {
            break;
        }
        let source_kind = inquire::Select::new(
            "Source:",
            vec!["URL (web page, localhost)", "Local file (PDF, PNG, HTML)"],
        )
        .prompt()
        .map_err(|e| Error::Export(format!("source wizard: {e}")))?;
        let url = if source_kind.starts_with("Local") {
            // Store the durable file:// URL — the score outlives this session, and
            // a wizard server's port dies with it (export serves/renders on its
            // own). The live server below only backs `demo focus`/`open` previews
            // during the capture itself.
            let path = pick_local_file(&roots, false)?;
            match local_server::serve_local_file(&path) {
                Ok((live_url, server)) => {
                    servers.push(server);
                    eprintln!("● live preview served on {live_url}");
                }
                Err(e) => eprintln!("demo: live preview server failed ({e}) — continuing"),
            }
            file_url_absolute(&path)?
        } else {
            let raw = inquire::Text::new("URL:")
                .with_help_message("https://github.com/..., http://localhost:3000")
                .prompt()
                .map_err(|e| Error::Export(format!("source wizard: {e}")))?;
            repair_browser_url(raw.trim(), launch_dir)?
        };
        let theme = inquire::Select::new(
            "Browser theme:",
            vec!["default (page preference)", "light", "dark"],
        )
        .prompt()
        .map_err(|e| Error::Export(format!("source wizard: {e}")))?;
        let theme = match theme {
            "light" => Some("light".to_string()),
            "dark" => Some("dark".to_string()),
            _ => None,
        };
        sources.push(crate::model::Source {
            id,
            kind: crate::model::SourceKind::Browser,
            url: Some(url),
            theme,
        });
        let more = inquire::Confirm::new("Add another source?")
            .with_default(false)
            .prompt()
            .map_err(|e| Error::Export(format!("source wizard: {e}")))?;
        if !more {
            break;
        }
    }
    Ok((sources, servers))
}

/// Decode PTY bytes to text across read boundaries. `pending` holds bytes left
/// over from a previous chunk that ended mid-sequence; new `bytes` are appended,
/// the longest valid UTF-8 prefix is returned, and any incomplete trailing
/// sequence is kept in `pending` for the next call. Genuinely invalid bytes are
/// replaced with `U+FFFD` so a bad byte can't stall the stream. Without this, a
/// multi-byte glyph split across two reads (dense braille from `mapscii`) would
/// be corrupted in the recording even though the live terminal looks right.
fn decode_streaming(pending: &mut Vec<u8>, bytes: &[u8]) -> String {
    pending.extend_from_slice(bytes);
    let mut out = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(s) => {
                out.push_str(s);
                pending.clear();
                break;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                // SAFETY: `valid_up_to` is the length of a checked valid prefix.
                out.push_str(unsafe { std::str::from_utf8_unchecked(&pending[..valid]) });
                match e.error_len() {
                    // An invalid byte (not merely incomplete): emit a replacement
                    // and skip past it, then keep decoding the remainder.
                    Some(bad) => {
                        out.push('\u{FFFD}');
                        pending.drain(..valid + bad);
                    }
                    // Incomplete sequence at the end: hold it for the next read.
                    None => {
                        pending.drain(..valid);
                        break;
                    }
                }
            }
        }
    }
    out
}

/// Length in bytes of a UTF-8 sequence given its leading byte. A stray
/// continuation byte (`0x80..=0xbf`, not a valid lead) is treated as length 1.
fn utf8_len(b: u8) -> usize {
    if b < 0xc0 {
        1
    } else if b < 0xe0 {
        2
    } else if b < 0xf0 {
        3
    } else {
        4
    }
}

/// What routing a chunk of keystrokes implies for the recorder. Input is passed
/// straight through to the PTY (so the shell echoes it — no hidden commands); we
/// only *watch* the typed line to know when a demo meta-command needs muting or
/// when an `--after` reveal should be armed.
struct RouteOutcome {
    to_pty: Vec<u8>,
    /// A `demo open`/`demo stop`/`demo focus` was just entered — its echo and any
    /// wizard/confirmation must be excised from the recording.
    mute_command: bool,
}

/// Forward a keystroke chunk to the PTY and track the current command line so the
/// caller can mute demo meta-commands and arm `--after` reveals. Everything typed
/// reaches the shell verbatim (and is echoed by it) — control now lives in the
/// top-level `demo stop`/`demo focus`/`demo open` commands, not hidden keystrokes.
fn route_input_chunk(
    chunk: &[u8],
    cmd_line: &mut String,
    cmd_start: &mut Option<u64>,
    now: u64,
) -> RouteOutcome {
    let mut to_pty: Vec<u8> = Vec::with_capacity(chunk.len());
    let mut mute_command = false;
    let mut i = 0;
    let n = chunk.len();
    // Mark the start of a fresh command line at its first printable char, so a
    // muted meta-command span covers the whole echoed line, not just its Enter.
    let mark_start = |cmd_line: &String, cmd_start: &mut Option<u64>| {
        if cmd_line.is_empty() {
            *cmd_start = Some(now);
        }
    };
    while i < n {
        let b = chunk[i];
        if b == b'\r' || b == b'\n' {
            to_pty.push(b);
            let t = cmd_line.trim_start();
            if is_meta_command(t) {
                mute_command = true;
            }
            cmd_line.clear();
            *cmd_start = None;
            i += 1;
            continue;
        }
        if b == 0x7f {
            to_pty.push(b);
            cmd_line.pop();
            i += 1;
            continue;
        }
        if b < 0x20 {
            to_pty.push(b);
            i += 1;
            continue;
        }
        if b >= 0x80 {
            let seq_len = utf8_len(b);
            let end = (i + seq_len).min(n);
            to_pty.extend_from_slice(&chunk[i..end]);
            if let Ok(s) = std::str::from_utf8(&chunk[i..end]) {
                if let Some(ch) = s.chars().next() {
                    mark_start(cmd_line, cmd_start);
                    cmd_line.push(ch);
                }
            }
            i = end;
            continue;
        }
        mark_start(cmd_line, cmd_start);
        to_pty.push(b);
        cmd_line.push(b as char);
        i += 1;
    }
    RouteOutcome {
        to_pty,
        mute_command,
    }
}

/// Does this typed line invoke a `demo` control command whose echo + wizard must
/// stay out of the recording?
fn is_meta_command(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("demo open") || t.starts_with("demo stop") || t.starts_with("demo focus")
}

enum CaptureWorkdir {
    Current(std::path::PathBuf),
    Temp(std::path::PathBuf),
}

impl CaptureWorkdir {
    fn path(&self) -> &std::path::Path {
        match self {
            Self::Current(path) | Self::Temp(path) => path,
        }
    }
}

impl Drop for CaptureWorkdir {
    fn drop(&mut self) {
        let Self::Temp(path) = self else {
            return;
        };
        if let Err(e) = std::fs::remove_dir_all(path.as_path()) {
            eprintln!(
                "Warning: failed to clean temporary directory {}: {e}",
                path.display()
            );
        }
    }
}

fn setup_workdir(use_here: bool) -> Result<CaptureWorkdir> {
    if use_here {
        let cwd = std::env::current_dir()
            .map_err(|e| Error::Export(format!("failed to get current directory: {e}")))?;
        return Ok(CaptureWorkdir::Current(cwd));
    }

    let temp_base = std::env::temp_dir();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    for attempt in 0..100 {
        let temp_dir = temp_base.join(format!("demo-{timestamp}-{pid}-{attempt}"));
        match std::fs::create_dir(&temp_dir) {
            Ok(()) => return Ok(CaptureWorkdir::Temp(temp_dir)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(Error::Export(format!(
                    "failed to create temporary directory {}: {e}",
                    temp_dir.display()
                )))
            }
        }
    }

    Err(Error::Export(
        "failed to create a unique temporary directory".to_string(),
    ))
}

pub fn run(args: CaptureArgs) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::Export(
            "capture needs an interactive terminal (stdin/stdout is not a TTY)".to_string(),
        ));
    }

    println!("{}\n", crate::BANNER);

    let shell = args
        .shell
        .clone()
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/bash".to_string());
    // A detached/odd terminal can report 0×0 — that would produce a degenerate
    // recording (and a divide-by-zero deeper in the renderer), so fall back.
    let (cols, rows) = match size() {
        Ok((c, r)) if c > 0 && r > 0 => (c, r),
        _ => (80, 24),
    };

    let pair = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| Error::Export(format!("openpty: {e}")))?;
    // Control file in the cwd: `demo open` / `demo stop` — run inside the capture
    // OR from another terminal in this directory — append commands here; the
    // watchdog reads them. The env var lets the captured shell find it directly.
    let control_path = std::path::PathBuf::from(control::CONTROL_FILE);
    std::fs::File::create(&control_path).map_err(|e| Error::io(&control_path, e))?;
    let control_abs = std::fs::canonicalize(&control_path).unwrap_or_else(|_| control_path.clone());
    let launch_dir = std::env::current_dir()
        .map_err(|e| Error::Export(format!("failed to get launch directory: {e}")))?;
    let work_dir = setup_workdir(args.here)?;
    control::write_meta(&control_abs, &launch_dir, work_dir.path())?;
    if !args.here {
        println!("Working directory: {}\n", work_dir.path().display());
    }

    let mut command = CommandBuilder::new(&shell);
    command.cwd(work_dir.path());
    command.env(control::CONTROL_ENV, &control_abs);

    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| Error::Export(format!("spawn {shell}: {e}")))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| Error::Export(format!("pty reader: {e}")))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| Error::Export(format!("pty writer: {e}")))?;

    let events: Arc<Mutex<Vec<RawEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let last_activity = Arc::new(Mutex::new(Instant::now()));
    let shell_exited = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    // Set while the program is showing a password/passphrase prompt: input typed
    // during it is forwarded to the PTY but NEVER recorded.
    let sensitive = Arc::new(AtomicBool::new(false));
    // The text of the secret prompt currently showing (e.g. `Vault passphrase:`),
    // captured so a `Secret` event can record WHICH secret was entered — never the
    // value. Set by the output thread, consumed by the input thread on Enter.
    let secret_prompt: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    // Set by the output thread when a non-secret line is seen after a secret prompt
    // was detected. The input thread reads this on Enter to decide whether to clear
    // the dedup guard: if true, the prompt has left the screen and the same prompt
    // text detected later should record a new Secret event.
    let secret_prompt_cleared = Arc::new(AtomicBool::new(false));
    // Browser reveals armed by `demo open --when <pat>`, fired by the output
    // thread when the pattern appears.
    let pending_opens: PendingWhen = Arc::new(Mutex::new(Vec::new()));
    // Reveals armed by `demo open --after`: fired when the current foreground command
    // finishes (produces output and then goes quiet, back at the prompt). `after_running`
    // tracks that such a command is in flight; `after_last_out` its last output.
    let after_opens: PendingAfter = Arc::new(Mutex::new(Vec::new()));
    let after_running = Arc::new(AtomicBool::new(false));
    let after_last_out = Arc::new(Mutex::new(Instant::now()));
    // True while a control command (`demo open` / `demo stop`) typed inside the
    // capture is running: its output (wizard, confirmation) is NOT recorded, so
    // it never leaks into the demo. Set when the command is typed, cleared when
    // the recorder receives it (or a safety timeout).
    let muting = Arc::new(AtomicBool::new(false));
    let mute_since = Arc::new(Mutex::new(Instant::now()));
    // Recorded meta-command spans `(start_ms, end_ms)` for the finished demo:
    // each `demo open` (its echo + in-session wizard) is excised in post, so the
    // result is clean even if the live mute raced. `mute_start` holds the open
    // span's start until its control command arrives and closes it.
    let mute_spans: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
    let mute_start: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
    // Force a clean prompt (unless --keep-prompt): recording doesn't begin until
    // the shell echoes a readiness marker, so its rc/PS1-setup chatter is dropped.
    // With neither flag a quick wizard asks how to set it before recording.
    let (force_prompt, forced_ps1) = choose_prompt(&args, &shell)?;
    let resolution = choose_canvas(&args)?;
    let fps = choose_fps(&args)?;
    let font_family = choose_font(&args)?;
    let (sources, _local_servers) = choose_sources(&launch_dir, work_dir.path())?;
    // Publish the sources beside the control file so `demo focus`/`demo open` can
    // list them live (the score isn't written until the capture ends).
    // `_local_servers` must stay alive for the capture duration — they serve
    // local files (PDF, PNG, HTML) via HTTP so Chromium can access them.
    let _ = control::write_sources(&control_abs, &sources);
    let ready = Arc::new(AtomicBool::new(!force_prompt));
    let t0 = Instant::now();

    // Optional diagnostic log (`--debug`): every chunk in/out with hex, plus
    // lifecycle notes, written next to the raw macro.
    let debug: Option<Arc<DebugLog>> = if args.debug {
        let mut path = args.rec.clone().into_os_string();
        path.push(".debug.log");
        let path = std::path::PathBuf::from(path);
        let log = Arc::new(DebugLog::create(&path, t0)?);
        log.note(&format!(
            "capture start — shell={shell} cols={cols} rows={rows} idle_timeout_ms={} control={}",
            args.idle_timeout_ms,
            control_abs.display()
        ));
        println!("  (debug log → {})", path.display());
        Some(log)
    } else {
        None
    };

    println!("● capturing — run your demo, then `demo stop` (or `exit` / Ctrl-D) to stop");
    println!("  during capture: `demo focus <source>` and `demo open <url>` — here or from another terminal in this directory");
    if args.idle_timeout_ms > 0 {
        println!(
            "  (auto-stops after {} ms with no output)",
            args.idle_timeout_ms
        );
    }
    println!();

    enable_raw_mode().map_err(|e| Error::Export(format!("raw mode: {e}")))?;

    // Pre-roll: force the prompt over the rc files, then echo the readiness marker.
    // The output thread discards everything until it sees the marker, so none of
    // this (nor a leaked `user@host`) lands in the recording.
    if force_prompt {
        let var = if is_zsh(&shell) { "PROMPT" } else { "PS1" };
        let _ = writeln!(writer, "{var}={}; clear", sh_single_quote(&forced_ps1));
        let _ = writeln!(writer, "printf 'demostage_capture_%s\\n' ready");
        let _ = writer.flush();
    }

    // PTY → stdout, recorded as output events.
    let out_handle = {
        let events = events.clone();
        let last = last_activity.clone();
        let exited = shell_exited.clone();
        let sensitive = sensitive.clone();
        let secret_prompt = secret_prompt.clone();
        let secret_prompt_cleared = secret_prompt_cleared.clone();
        let debug = debug.clone();
        let pending_opens = pending_opens.clone();
        let muting = muting.clone();
        let ready = ready.clone();
        let after_running = after_running.clone();
        let after_last_out = after_last_out.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut stdout = std::io::stdout();
            let mut line = String::new();
            // Rolling recent output, for matching `demo open --when` cues across
            // chunk/line boundaries (a per-line check misses a cue then a newline).
            let mut recent = String::new();
            // Pre-roll buffer: output before the readiness marker (prompt setup).
            let mut pre = String::new();
            // Trailing bytes of an incomplete UTF-8 sequence, carried to the next
            // read: a PTY read can split a multi-byte glyph (e.g. braille from
            // mapscii) across the 4 KiB boundary, and decoding each chunk in
            // isolation would corrupt it into replacement chars in the recording.
            let mut pending: Vec<u8> = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    // A signal (e.g. SIGWINCH on resize) interrupts the blocking
                    // read; that is not the shell exiting — retry, don't stop.
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                    Ok(n) => {
                        let _ = stdout.write_all(&buf[..n]);
                        let _ = stdout.flush();
                        *last.lock().unwrap() = Instant::now();
                        if let Some(d) = &debug {
                            d.chunk("OUT", &buf[..n]);
                        }
                        // Decode across the read boundary: keep any incomplete
                        // trailing sequence in `pending` for the next chunk.
                        let text = decode_streaming(&mut pending, &buf[..n]);
                        // Detect the secret prompt at each line boundary and LATCH
                        // the flag (only set here; the input thread clears it on
                        // Enter), so the masked redraws can't unset it mid-secret.
                        track_and_detect(
                            &mut line,
                            &text,
                            &sensitive,
                            &secret_prompt,
                            &secret_prompt_cleared,
                        );
                        // Pre-roll: discard prompt-setup output until the readiness
                        // marker; record only what follows it (the clean prompt).
                        if !ready.load(Ordering::SeqCst) {
                            pre.push_str(&text);
                            if let Some(idx) = pre.find(PROMPT_READY) {
                                let after = pre[idx + PROMPT_READY.len()..].to_string();
                                pre.clear();
                                ready.store(true, Ordering::SeqCst);
                                if !after.is_empty() {
                                    recent.push_str(&after);
                                    events.lock().unwrap().push(RawEvent::Output {
                                        t_ms: ms(t0),
                                        data: after,
                                    });
                                }
                            }
                            continue;
                        }
                        // While a typed `demo open`/`demo stop` is running, drop its
                        // output entirely (no record, no cue-matching) so it never
                        // appears in the demo.
                        if muting.load(Ordering::SeqCst) {
                            continue;
                        }
                        let now = ms(t0);
                        recent.push_str(&text);
                        // An `--after` command is in flight → note this output, so
                        // the watchdog can fire once the stream goes quiet again.
                        if after_running.load(Ordering::SeqCst) {
                            *after_last_out.lock().unwrap() = Instant::now();
                        }
                        // Fire any `--when <pat>` reveal whose cue just appeared.
                        {
                            let mut pend = pending_opens.lock().unwrap();
                            if !pend.is_empty() {
                                let mut evs = events.lock().unwrap();
                                pend.retain(|(r, pat)| {
                                    if cue_matches(&recent, pat) {
                                        evs.push(r.to_event(now));
                                        false
                                    } else {
                                        true
                                    }
                                });
                            }
                        }
                        if recent.len() > 8192 {
                            recent.clear();
                        }
                        events.lock().unwrap().push(RawEvent::Output {
                            t_ms: now,
                            data: text,
                        });
                    }
                }
            }
            exited.store(true, Ordering::SeqCst);
        })
    };

    // stdin → PTY, recorded as input events. (Detached: stdin reads block; the
    // process exits once recording stops, which tears this down.)
    {
        let events = events.clone();
        let last = last_activity.clone();
        let stop = stop.clone();
        let sensitive = sensitive.clone();
        let secret_prompt = secret_prompt.clone();
        let secret_prompt_cleared = secret_prompt_cleared.clone();
        let debug = debug.clone();
        let muting = muting.clone();
        let mute_since = mute_since.clone();
        let mute_start = mute_start.clone();
        let ready = ready.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let mut stdin = std::io::stdin();
            let mut cmd_line = String::new();
            // When the current input line started (first char), so a `demo open`/
            // `demo stop`/`demo focus` span can be excised from the command's echo,
            // not just from the Enter that follows it.
            let mut cmd_start: Option<u64> = None;
            let mut last_secret_prompt: Option<String> = None;
            while !stop.load(Ordering::SeqCst) {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                    Ok(n) => {
                        *last.lock().unwrap() = Instant::now();
                        // Don't record input during the prompt-setup pre-roll.
                        if !ready.load(Ordering::SeqCst) {
                            continue;
                        }
                        let saved_cmd_start = cmd_start;
                        let outcome =
                            route_input_chunk(&buf[..n], &mut cmd_line, &mut cmd_start, ms(t0));
                        if outcome.mute_command {
                            *mute_since.lock().unwrap() = Instant::now();
                            muting.store(true, Ordering::SeqCst);
                            let mut s = mute_start.lock().unwrap();
                            if s.is_none() {
                                *s = Some(saved_cmd_start.unwrap_or_else(|| ms(t0)));
                            }
                        }
                        if !outcome.to_pty.is_empty() {
                            if writer.write_all(&outcome.to_pty).is_err() {
                                break;
                            }
                            let _ = writer.flush();
                        }
                        // Secret prompt active → forward the keystrokes but do NOT
                        // record them; clear once the prompt is answered (Enter).
                        let masked = sensitive.load(Ordering::SeqCst);
                        if let Some(d) = &debug {
                            if masked {
                                d.note(&format!("IN* {n} bytes (redacted — secret prompt)"));
                            } else {
                                d.chunk("IN ", &outcome.to_pty);
                            }
                        }
                        if masked {
                            if outcome.to_pty.iter().any(|b| *b == b'\r' || *b == b'\n') {
                                sensitive.store(false, Ordering::SeqCst);
                                let prompt_left_screen =
                                    secret_prompt_cleared.swap(false, Ordering::SeqCst);
                                let prompt =
                                    secret_prompt.lock().unwrap().take().unwrap_or_default();
                                if secret_step_on_submit(
                                    &mut last_secret_prompt,
                                    prompt_left_screen,
                                    &prompt,
                                ) {
                                    events.lock().unwrap().push(RawEvent::Secret {
                                        t_ms: ms(t0),
                                        prompt,
                                    });
                                }
                            }
                            continue;
                        }
                        // Don't record input while a meta-command (demo open/stop)
                        // is running — its wizard answers must not enter the demo.
                        if muting.load(Ordering::SeqCst) {
                            continue;
                        }
                        if outcome.to_pty.is_empty() {
                            continue;
                        }
                        events.lock().unwrap().push(RawEvent::Input {
                            t_ms: ms(t0),
                            bytes: String::from_utf8_lossy(&outcome.to_pty).into_owned(),
                        });
                    }
                }
            }
        });
    }

    // Watchdog: read control commands (`demo open` / `demo stop`), stop on shell
    // exit or idle timeout.
    let idle = args.idle_timeout_ms;
    let mut control_read = 0u64;
    let reason = loop {
        if shell_exited.load(Ordering::SeqCst) {
            break "reader closed (shell exited)";
        }
        if matches!(child.try_wait(), Ok(Some(_))) {
            break "shell process exited";
        }
        if let Some(r) = read_control(
            &control_abs,
            &mut control_read,
            &events,
            &pending_opens,
            &after_opens,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            debug.as_deref(),
        ) {
            break r;
        }
        // An `--after` command has finished (produced output, then went quiet) →
        // fire its reveals now, at the moment the shell returned to the prompt.
        if after_running.load(Ordering::SeqCst)
            && after_last_out.lock().unwrap().elapsed() > Duration::from_millis(AFTER_QUIET_MS)
        {
            let drained: Vec<Reveal> = after_opens.lock().unwrap().drain(..).collect();
            let now = ms(t0);
            let mut evs = events.lock().unwrap();
            for r in drained {
                if let Some(d) = &debug {
                    d.note(&format!("reveal (after): {}", r.summary()));
                }
                evs.push(r.to_event(now));
            }
            after_running.store(false, Ordering::SeqCst);
        }
        // Safety: an abandoned `demo open` (wizard cancelled, command never sent)
        // shouldn't mute the rest of the demo forever.
        maybe_close_safety_valve(
            &muting,
            &mute_since,
            &mute_start,
            &mute_spans,
            t0,
            debug.as_deref(),
        );
        // Safety: if the readiness marker never arrives (odd shell), start
        // recording anyway rather than capturing nothing.
        if !ready.load(Ordering::SeqCst) && t0.elapsed() > Duration::from_secs(4) {
            ready.store(true, Ordering::SeqCst);
        }
        if idle > 0 && last_activity.lock().unwrap().elapsed() > Duration::from_millis(idle) {
            break "idle timeout";
        }
        thread::sleep(Duration::from_millis(100));
    };
    // Final drain: the watchdog checks exit conditions *before* read_control,
    // so a control line written in the last ≤100 ms before shell exit would
    // otherwise be lost. One extra read picks it up without waiting.
    let _ = read_control(
        &control_abs,
        &mut control_read,
        &events,
        &pending_opens,
        &after_opens,
        &after_running,
        &after_last_out,
        &muting,
        &mute_start,
        &mute_spans,
        t0,
        debug.as_deref(),
    );
    if let Some(d) = &debug {
        d.note(&format!("stopping — reason: {reason}"));
    }

    stop.store(true, Ordering::SeqCst);
    let _ = child.kill();
    let _ = child.wait();
    let _ = out_handle.join();
    let _ = disable_raw_mode();
    let _ = std::fs::remove_file(&control_abs);
    let _ = std::fs::remove_file(control_abs.with_file_name(control::SOURCES_FILE));
    let _ = std::fs::remove_file(control_abs.with_file_name(control::META_FILE));

    let drain_summary = {
        let mut evs = events.lock().unwrap();
        let raw_for_cutoff = crate::model::RawMacro {
            meta: crate::model::RawMeta {
                shell: String::new(),
                cols: 0,
                rows: 0,
                idle_timeout_ms: 0,
                resolution: None,
                fps: None,
                stage: None,
                mute_spans: Vec::new(),
            },
            events: evs.clone(),
        };
        let drain_ts = recording::stop_cutoff_ms(&raw_for_cutoff)
            .map(|c| c.saturating_sub(1))
            .unwrap_or_else(|| {
                evs.iter()
                    .filter_map(|e| match e {
                        RawEvent::Output { t_ms, .. } => Some(*t_ms),
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0)
            });
        drain_remaining_reveals(&after_opens, &pending_opens, &mut evs, drain_ts)
    };
    for pat in &drain_summary.when_unmatched {
        eprintln!(
            "warning: --when cue {pat:?} never matched during capture; reveal appended at end"
        );
    }
    for summary in &drain_summary.after_summaries {
        eprintln!(
            "warning: --after reveal {summary} never fired during capture; reveal appended at end"
        );
    }

    let events = events.lock().unwrap().clone();
    let mut mute_spans = mute_spans.lock().unwrap().clone();
    // Close a span still open at stop time (e.g. a wizard cut short by `demo stop`).
    if let Some(start) = mute_start.lock().unwrap().take() {
        mute_spans.push((start, ms(t0)));
    }
    let raw = RawMacro {
        meta: RawMeta {
            shell,
            cols,
            rows,
            idle_timeout_ms: idle,
            resolution,
            fps: (fps != DEFAULT_FPS).then_some(fps),
            stage: args.into.as_ref().map(|p| p.display().to_string()),
            mute_spans,
        },
        events,
    };
    // Keep the raw macro only if the user asked for it (it's a pure intermediate
    // — by default a capture leaves behind just the recording).
    if let Some(raw_path) = &args.output {
        raw.save(raw_path)?;
    }
    if let Some(d) = &debug {
        let (ins, outs) = raw.events.iter().fold((0, 0), |(i, o), e| match e {
            RawEvent::Input { .. } => (i + 1, o),
            RawEvent::Output { .. } => (i, o + 1),
            RawEvent::Reveal { .. } | RawEvent::Secret { .. } => (i, o),
        });
        d.note(&format!(
            "recorded {} events ({ins} input, {outs} output)",
            raw.events.len(),
        ));
    }

    // The recording (.rec) of what actually happened, so `demo export` plays back
    // the real session out of the box — no re-execution, which is what breaks
    // interactive/secret/side-effecting tools. This is the one file a capture
    // always leaves behind.
    let cast_path = args.rec.clone();
    let name = cast_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("demo");
    println!(
        "recorded {} events → {}",
        raw.events.len(),
        cast_path.display()
    );

    // Without a normalize pass there is no score — the recording stays faithful.
    if args.no_normalize {
        write_faithful_cast(&raw, None, &cast_path)?;
        println!(
            "next: demo export {}   (renders the live capture)",
            cast_path.display()
        );
        return Ok(());
    }

    // Normalize in memory into a clean score: it carries the demo name/meta for
    // the recording, and is the source `demo record` re-runs — written to disk
    // only when asked (`--score`, or a `--into` stage, which defaults it).
    let opts = Options {
        typing_ms: 80,
        salt_ms: 15,
        seed: None,
    };
    let mut score = match &args.into {
        Some(path) => merge_into_stage(Score::load(path)?, &raw, &opts),
        None => {
            let normalized = normalize(&raw, name, &opts);
            // Preserve sources from an existing score file (defined before capture).
            if !args.no_score && args.normalized_output.exists() {
                if let Ok(existing) = Score::load(&args.normalized_output) {
                    if !existing.sources.is_empty() {
                        let mut score = normalized;
                        score.sources = existing.sources;
                        score
                    } else {
                        normalized
                    }
                } else {
                    normalized
                }
            } else {
                normalized
            }
        }
    };
    // Persist the prompt the demo was captured with, so `demo record` reproduces
    // it instead of falling back to the built-in default.
    if force_prompt {
        score.demo.prompt = Some(forced_ps1.clone());
    }
    // Store the chosen font in the layout so `demo export` uses it.
    score.layout.font_family = Some(font_family);
    // Store wizard-selected sources (skip if already set from --into).
    if score.sources.is_empty() && !sources.is_empty() {
        score.sources = sources;
    }
    let score_path = if args.no_score {
        None
    } else {
        Some(args.normalized_output.clone())
    };
    if let Some(p) = &score_path {
        score.save(p)?;
    }
    write_faithful_cast(&raw, Some(&score), &cast_path)?;

    match &score_path {
        Some(p) => println!(
            "score → {}   |   next: demo export {}  (or `demo record` to re-run)",
            p.display(),
            cast_path.display()
        ),
        None => println!(
            "next: demo export {}   (--no-score set, so `demo record` won't have a score)",
            cast_path.display()
        ),
    }

    // Offer to run `demo edit` for quick timeline refinement.
    if let Some(p) = &score_path {
        if std::io::stdin().is_terminal() {
            let run_direct = inquire::Confirm::new("Run demo edit to refine the timeline?")
                .with_default(false)
                .prompt()
                .unwrap_or(false);
            if run_direct {
                super::edit::run(crate::cli::EditArgs { input: p.clone() })?;
            }
        }
    }
    Ok(())
}

/// Parse a `reveal` control message into a [`Reveal`] — its panes, orientation,
/// hold and scroll. Returns `None` if it carries no panes.
fn parse_reveal(v: &serde_json::Value) -> Option<Reveal> {
    let panes_json = v.get("panes")?.as_array()?;
    let mut panes = Vec::new();
    for p in panes_json {
        let id = p
            .get("id")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("main")
            .to_string();
        let url = p
            .get("url")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let theme = p
            .get("theme")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        panes.push(RevealPane { id, url, theme });
    }
    if panes.is_empty() {
        return None;
    }
    let orientation = match v.get("orientation").and_then(|o| o.as_str()) {
        Some("vertical") => Orientation::Vertical,
        _ => Orientation::Horizontal,
    };
    let hold_ms = v.get("hold").and_then(|h| h.as_u64());
    let scroll = v.get("scroll").and_then(|s| s.as_bool()).unwrap_or(false);
    Some(Reveal {
        panes,
        orientation,
        hold_ms,
        scroll,
    })
}

/// Does the recent output match a `--when` cue? A `re:` prefix is a regular
/// expression; otherwise it's a plain substring match.
fn cue_matches(recent: &str, pattern: &str) -> bool {
    if let Some(rx) = pattern.strip_prefix("re:") {
        match regex::Regex::new(rx) {
            Ok(re) => re.is_match(recent),
            // A bad pattern never matches (rather than firing spuriously).
            Err(_) => false,
        }
    } else {
        recent.contains(pattern)
    }
}

/// Summary of what the shutdown drain resolved from the pending queues.
struct DrainSummary {
    after_summaries: Vec<String>,
    when_unmatched: Vec<String>,
}

/// Drain any remaining `--after` and `--when` queues at shutdown, emitting
/// their reveals as events so they are recorded rather than silently dropped.
/// Returns a summary of what was fired and which `--when` cues never matched.
fn drain_remaining_reveals(
    after_opens: &PendingAfter,
    pending_opens: &PendingWhen,
    events: &mut Vec<RawEvent>,
    now: u64,
) -> DrainSummary {
    let after_remaining: Vec<Reveal> = after_opens.lock().unwrap().drain(..).collect();
    let mut after_summaries = Vec::new();
    for r in after_remaining {
        after_summaries.push(r.summary());
        events.push(r.to_event(now));
    }
    let mut when_unmatched = Vec::new();
    let pending: Vec<(Reveal, String)> = pending_opens.lock().unwrap().drain(..).collect();
    for (r, pat) in pending {
        when_unmatched.push(pat.clone());
        events.push(r.to_event(now));
    }
    DrainSummary {
        after_summaries,
        when_unmatched,
    }
}

/// Read any new control-file commands (`demo focus`/`demo open`/`demo stop`).
/// Records immediate reveals, arms `--when`/`--after` reveals, and returns
/// `Some(reason)` on stop.
#[allow(clippy::too_many_arguments)]
fn read_control(
    path: &std::path::Path,
    read: &mut u64,
    events: &Arc<Mutex<Vec<RawEvent>>>,
    pending: &PendingWhen,
    after: &PendingAfter,
    after_running: &AtomicBool,
    after_last_out: &Mutex<Instant>,
    muting: &Arc<AtomicBool>,
    mute_start: &Arc<Mutex<Option<u64>>>,
    mute_spans: &Arc<Mutex<Vec<(u64, u64)>>>,
    t0: Instant,
    debug: Option<&DebugLog>,
) -> Option<&'static str> {
    let data = std::fs::read(path).ok()?;
    if data.len() as u64 <= *read {
        return None;
    }
    let new = String::from_utf8_lossy(&data[*read as usize..]).into_owned();
    *read = data.len() as u64;

    let mut stop = None;
    for line in new.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        match v.get("cmd").and_then(|c| c.as_str()) {
            // A `demo focus`/`demo open` is starting in the captured shell → mute
            // its echo/wizard. If input detection didn't already mark the start,
            // fall back to a little before now so its first output is still excised.
            Some("reveal_begin") => {
                muting.store(true, Ordering::SeqCst);
                let mut s = mute_start.lock().unwrap();
                if s.is_none() {
                    *s = Some(ms(t0).saturating_sub(OPEN_BEGIN_BACKDATE_MS));
                }
            }
            Some("stop") => {
                muting.store(false, Ordering::SeqCst);
                stop = Some("demo stop");
            }
            Some("reveal_cancel") => {
                // A meta-command failed/was cancelled → close the mute span
                // without recording a reveal (the command leaves no trace).
                muting.store(false, Ordering::SeqCst);
                let mute_span_start = mute_start.lock().unwrap().take();
                if let Some(start) = mute_span_start {
                    mute_spans.lock().unwrap().push((start, ms(t0)));
                }
                if let Some(d) = debug {
                    d.note("reveal_cancel — meta-command failed or was cancelled");
                }
            }
            Some("reveal") => {
                // The command finished → stop muting and close its excision span.
                muting.store(false, Ordering::SeqCst);
                let mute_span_start = mute_start.lock().unwrap().take();
                if let Some(start) = mute_span_start {
                    mute_spans.lock().unwrap().push((start, ms(t0)));
                }
                let Some(reveal) = parse_reveal(&v) else {
                    continue;
                };
                let when = v
                    .get("when")
                    .and_then(|w| w.as_str())
                    .filter(|s| !s.is_empty());
                let after_flag = v.get("after").and_then(|a| a.as_bool()).unwrap_or(false);
                if let Some(pat) = when {
                    if let Some(d) = debug {
                        d.note(&format!("reveal armed: {} when {pat:?}", reveal.summary()));
                    }
                    pending.lock().unwrap().push((reveal, pat.into()));
                } else if after_flag {
                    if let Some(d) = debug {
                        d.note(&format!("reveal armed: {} after command", reveal.summary()));
                    }
                    after.lock().unwrap().push(reveal);
                    after_running.store(true, Ordering::SeqCst);
                    *after_last_out.lock().unwrap() = Instant::now();
                } else {
                    // Immediate reveal: use current time (when command finished)
                    if let Some(d) = debug {
                        d.note(&format!("reveal now: {}", reveal.summary()));
                    }
                    events.lock().unwrap().push(reveal.to_event(ms(t0)));
                }
            }
            _ => {}
        }
    }
    stop
}

/// Write a faithful recording of the captured session (its real output) to
/// `path`, so `demo export` can play it back without re-executing anything.
fn write_faithful_cast(
    raw: &RawMacro,
    score: Option<&Score>,
    path: &std::path::Path,
) -> Result<()> {
    let name = score.map(|s| s.demo.name.as_str()).unwrap_or("demo");
    // The layout comes from the capture's `demo open` scenes (terminal + browser
    // panes); the demo meta/typing come from the normalized score. The timeline
    // carries only browser-scroll steps (it isn't executed — playback is faithful).
    let (rec, mut layout, timeline) = recording::from_raw(raw, name);
    // The reveal-built layout has no styling of its own — the font chosen in the
    // capture wizard lives on the score's layout.
    if let Some(s) = score {
        layout.font_family = s.layout.font_family.clone();
    }
    let final_score = Score {
        demo: score.map(|s| s.demo.clone()).unwrap_or_else(|| DemoMeta {
            name: name.to_string(),
            output_dir: "./dist".into(),
            prompt: None,
        }),
        env: None,
        typing: score.and_then(|s| s.typing.clone()),
        sources: score.map(|s| s.sources.clone()).unwrap_or_default(),
        layout,
        timeline,
    };
    let cast = recording::write(&rec, &final_score, true)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
    }
    std::fs::write(path, cast).map_err(|e| Error::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    #[test]
    fn parses_resolution_presets_and_custom_sizes() {
        assert_eq!(parse_resolution("landscape").unwrap(), Some((1920, 1080)));
        assert_eq!(parse_resolution("Portrait").unwrap(), Some((1080, 1920)));
        assert_eq!(parse_resolution("square").unwrap(), Some((1080, 1080)));
        assert_eq!(parse_resolution("standard").unwrap(), Some((1280, 720)));
        assert_eq!(parse_resolution("1600x900").unwrap(), Some((1600, 900)));
        assert_eq!(parse_resolution("1600×900").unwrap(), Some((1600, 900)));
        assert_eq!(parse_resolution("auto").unwrap(), None);
        assert!(parse_resolution("0x100").is_err());
        assert!(parse_resolution("huge").is_err());
    }

    #[test]
    fn canvas_from_aspect_and_quality_covers_all_combos() {
        // FullHD (short side 1080).
        assert_eq!(
            canvas_from_aspect_quality("16:9", "fullhd").unwrap(),
            (1920, 1080)
        );
        assert_eq!(
            canvas_from_aspect_quality("9:16", "fullhd").unwrap(),
            (1080, 1920)
        );
        assert_eq!(
            canvas_from_aspect_quality("4:3", "fullhd").unwrap(),
            (1440, 1080)
        );
        assert_eq!(
            canvas_from_aspect_quality("1:1", "fullhd").unwrap(),
            (1080, 1080)
        );
        // HD (short side 720).
        assert_eq!(
            canvas_from_aspect_quality("16:9", "hd").unwrap(),
            (1280, 720)
        );
        assert_eq!(
            canvas_from_aspect_quality("9:16", "hd").unwrap(),
            (720, 1280)
        );
        assert_eq!(canvas_from_aspect_quality("4:3", "hd").unwrap(), (960, 720));
        assert_eq!(canvas_from_aspect_quality("1:1", "hd").unwrap(), (720, 720));
        // Case-insensitive.
        assert_eq!(
            canvas_from_aspect_quality("16:9", "FullHD").unwrap(),
            (1920, 1080)
        );
        assert_eq!(canvas_from_aspect_quality("1:1", "HD").unwrap(), (720, 720));
        // The legacy presets map onto the new scheme exactly.
        assert_eq!(
            canvas_from_aspect_quality("16:9", "fullhd").unwrap(),
            parse_resolution("landscape").unwrap().unwrap()
        );
        assert_eq!(
            canvas_from_aspect_quality("16:9", "hd").unwrap(),
            parse_resolution("standard").unwrap().unwrap()
        );
    }

    #[test]
    fn canvas_rejects_unknown_aspect_or_quality() {
        assert!(canvas_from_aspect_quality("3:2", "fullhd").is_err());
        assert!(canvas_from_aspect_quality("16:9", "4k").is_err());
    }

    #[test]
    fn parse_fps_accepts_15_24_30_and_rejects_others() {
        assert_eq!(parse_fps("15").unwrap(), 15);
        assert_eq!(parse_fps("24").unwrap(), 24);
        assert_eq!(parse_fps("30").unwrap(), 30);
        assert!(parse_fps("60").is_err());
        assert!(parse_fps("smooth").is_err());
    }

    #[test]
    fn workdir_here_uses_current_directory_without_cleanup() {
        let cwd = std::env::current_dir().unwrap();
        let workdir = setup_workdir(true).unwrap();

        assert_eq!(workdir.path(), cwd.as_path());
    }

    #[test]
    fn workdir_default_creates_and_cleans_temporary_directory() {
        let path = {
            let workdir = setup_workdir(false).unwrap();
            let path = workdir.path().to_path_buf();
            assert!(path.is_dir());
            path
        };

        assert!(!path.exists());
    }

    #[test]
    fn detects_secret_at_a_carriage_return_boundary() {
        // inquire emits the prompt immediately followed by `\r` and cursor codes —
        // detection must fire on the prompt BEFORE the `\r` clears the line.
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut line = String::new();
        // `\x1b` (ESC) is a control char; `[?25h` is the residue after it.
        track_and_detect(
            &mut line,
            " Vault passphrase:\r\x1b[?25h",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        assert!(
            sensitive.load(Ordering::SeqCst),
            "should latch on the prompt"
        );
        assert_eq!(
            secret_prompt.lock().unwrap().as_deref(),
            Some("Vault passphrase:")
        );
    }

    #[test]
    fn clean_prompt_strips_colour_and_cursor_codes() {
        // After ESC is stripped, both `[38;5;10m` colour and `[?25l` cursor codes
        // remain — clean_prompt removes them and the leading glyph.
        assert_eq!(
            clean_prompt("[?25h[?25l> [38;5;10mVault passphrase:[39m"),
            "Vault passphrase:"
        );
    }

    #[test]
    fn flags_secret_prompts_only() {
        assert!(is_secret_prompt(
            "Enter passphrase for key '/home/u/.ssh/id_ed25519':"
        ));
        assert!(is_secret_prompt("Password: "));
        assert!(is_secret_prompt("[sudo] password for jheison:"));
        assert!(is_secret_prompt("Vault passphrase?"));
        assert!(!is_secret_prompt("$ echo my secret plan"));
        assert!(!is_secret_prompt("Cloning into 'repo'..."));
        assert!(!is_secret_prompt("Refreshing access token cache"));
        assert!(!is_secret_prompt("Vault passphrase: ***"));
    }

    #[test]
    fn tui_screen_paint_never_matches_as_a_secret_prompt() {
        // A full-screen TUI (opencode, vim, …) repaints without newlines, so the
        // tracked "line" is huge even if it happens to contain "token …:". That
        // must never latch the secret redactor (it used to capture ~450KB of
        // screen paint as the prompt label).
        let huge = format!("{} tokens used - Context:", "x".repeat(5000));
        assert!(!is_secret_prompt(&huge));

        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut line = String::new();
        track_and_detect(
            &mut line,
            &huge,
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        assert!(!sensitive.load(Ordering::SeqCst));
        assert!(secret_prompt.lock().unwrap().is_none());
        // And the tracker's memory stays bounded while the paint streams on.
        assert!(line.len() <= MAX_PROMPT_LINE);
    }

    #[test]
    fn decode_streaming_reassembles_a_split_braille_glyph() {
        // U+2839 (⠹) is 3 bytes: e2 a0 b9. Split it across two reads, as a PTY
        // can at a 4 KiB boundary, and it must reassemble — not corrupt.
        let glyph = "⠹";
        let bytes = glyph.as_bytes();
        let mut pending = Vec::new();
        let first = decode_streaming(&mut pending, &bytes[..2]);
        assert_eq!(first, "", "an incomplete sequence yields nothing yet");
        let second = decode_streaming(&mut pending, &bytes[2..]);
        assert_eq!(second, glyph, "the rest completes the glyph intact");
        assert!(pending.is_empty());
    }

    #[test]
    fn decode_streaming_replaces_a_truly_invalid_byte() {
        // A lone 0xFF is invalid UTF-8 — it must become U+FFFD, not stall.
        let mut pending = Vec::new();
        let out = decode_streaming(&mut pending, &[b'a', 0xff, b'b']);
        assert_eq!(out, "a\u{FFFD}b");
        assert!(pending.is_empty());
    }

    /// Route a sequence of read chunks, returning the bytes forwarded to the PTY
    /// and whether a `demo` meta-command was seen (so its echo gets muted).
    fn route(input: &[&[u8]]) -> (Vec<u8>, bool) {
        let mut cmd_line = String::new();
        let mut cmd_start: Option<u64> = None;
        let mut to_pty = Vec::new();
        let mut mute = false;
        for chunk in input {
            let o = route_input_chunk(chunk, &mut cmd_line, &mut cmd_start, 0);
            to_pty.extend(o.to_pty);
            mute |= o.mute_command;
        }
        (to_pty, mute)
    }

    #[test]
    fn everything_typed_reaches_the_pty_and_is_echoed() {
        // No hidden commands anymore: whatever you type is forwarded verbatim so
        // the shell echoes it. A leading `/` is just a normal character now.
        let (to_pty, mute) = route(&[b"/stop\r"]);
        assert_eq!(to_pty, b"/stop\r");
        assert!(!mute);
    }

    #[test]
    fn regular_command_still_works() {
        let (to_pty, mute) = route(&[b"ls -la\n"]);
        assert_eq!(to_pty, b"ls -la\n");
        assert!(!mute);
    }

    #[test]
    fn meta_command_is_flagged_for_muting() {
        // `demo focus`/`demo open`/`demo stop` typed in-session must mute so their
        // echo and wizard never reach the recording — even split across reads.
        assert!(route(&[b"demo focus fill-main\n"]).1);
        assert!(route(&[b"demo ", b"open ", b"github.com\r"]).1);
        assert!(route(&[b"demo stop\n"]).1);
        assert!(
            !route(&[b"demodocs\n"]).1,
            "a lookalike command must not mute"
        );
    }

    #[test]
    fn backspace_is_forwarded_and_erases_cmd_line() {
        let (to_pty, _) = route(&[b"ab\x7f\n"]);
        assert_eq!(to_pty, b"ab\x7f\n");
    }

    #[test]
    fn utf8_round_trip_through_routing() {
        let (to_pty, _) = route(&["héllo\n".as_bytes()]);
        assert_eq!(to_pty, "héllo\n".as_bytes());
    }

    #[test]
    fn utf8_len_returns_correct_sequence_lengths() {
        assert_eq!(utf8_len(0x00), 1);
        assert_eq!(utf8_len(0x7f), 1);
        assert_eq!(utf8_len(0x80), 1);
        assert_eq!(utf8_len(0xbf), 1);
        assert_eq!(utf8_len(0xc0), 2);
        assert_eq!(utf8_len(0xdf), 2);
        assert_eq!(utf8_len(0xe0), 3);
        assert_eq!(utf8_len(0xef), 3);
        assert_eq!(utf8_len(0xf0), 4);
        assert_eq!(utf8_len(0xf4), 4);
    }

    #[test]
    fn ps1_text_doubles_backslashes() {
        assert_eq!(ps1_text(r"C:\Users"), r"C:\\Users");
        assert_eq!(ps1_text("no backslash"), "no backslash");
        assert_eq!(ps1_text(""), "");
    }

    #[test]
    fn is_meta_command_matches_demo_stop_open_focus() {
        assert!(is_meta_command("demo stop"));
        assert!(is_meta_command("demo open http://example.com"));
        assert!(is_meta_command("demo focus main"));
        assert!(is_meta_command("  demo stop"));
        assert!(!is_meta_command("echo demo stop"));
        assert!(!is_meta_command("ls"));
        assert!(!is_meta_command(""));
    }

    #[test]
    fn default_prompt_contains_user_at_demo_and_dollar() {
        let bash_prompt = default_prompt("/bin/bash");
        assert!(bash_prompt.contains("user@demo"));
        assert!(bash_prompt.contains("$ "));

        let zsh_prompt = default_prompt("/bin/zsh");
        assert!(zsh_prompt.contains("user@demo"));
        assert!(zsh_prompt.contains("$ "));
    }

    #[test]
    fn cue_matches_plain_substring() {
        assert!(cue_matches("Report generated successfully.", "Report"));
        assert!(!cue_matches("Report generated", "Error"));
    }

    #[test]
    fn cue_matches_regex_with_prefix() {
        assert!(cue_matches("done in 123ms", "re:\\d+ms"));
        assert!(!cue_matches("no numbers", "re:\\d+ms"));
        assert!(!cue_matches("anything", "re:[invalid"));
    }

    #[test]
    fn reveal_summary_format() {
        let r = Reveal {
            panes: vec![
                RevealPane {
                    id: "main".into(),
                    url: None,
                    theme: None,
                },
                RevealPane {
                    id: "docs".into(),
                    url: Some("http://x.com".into()),
                    theme: None,
                },
            ],
            orientation: Orientation::Vertical,
            hold_ms: None,
            scroll: false,
        };
        let s = r.summary();
        assert!(s.contains("main"));
        assert!(s.contains("docs"));
        assert!(s.contains("Vertical"));
    }

    #[test]
    fn reveal_to_event_produces_correct_timestamp() {
        let r = Reveal {
            panes: vec![RevealPane::terminal()],
            orientation: Orientation::Horizontal,
            hold_ms: Some(5000),
            scroll: true,
        };
        let ev = r.to_event(1234);
        match ev {
            RawEvent::Reveal {
                t_ms,
                panes,
                orientation,
                hold_ms,
                scroll,
            } => {
                assert_eq!(t_ms, 1234);
                assert_eq!(panes.len(), 1);
                assert_eq!(orientation, Orientation::Horizontal);
                assert_eq!(hold_ms, Some(5000));
                assert!(scroll);
            }
            _ => panic!("expected Reveal event"),
        }
    }

    #[test]
    fn parse_reveal_parses_json() {
        let v = serde_json::json!({
            "cmd": "reveal",
            "panes": [{"id": "main"}, {"id": "web", "url": "https://x.com", "theme": "dark"}],
            "orientation": "vertical",
            "hold": 3000,
            "scroll": true,
        });
        let r = parse_reveal(&v).unwrap();
        assert_eq!(r.panes.len(), 2);
        assert_eq!(r.orientation, Orientation::Vertical);
        assert_eq!(r.hold_ms, Some(3000));
        assert!(r.scroll);
        assert_eq!(r.panes[1].theme.as_deref(), Some("dark"));
    }

    #[test]
    fn parse_reveal_returns_none_for_empty_panes() {
        let v = serde_json::json!({"cmd": "reveal", "panes": []});
        assert!(parse_reveal(&v).is_none());
    }

    #[test]
    fn decode_streaming_valid_utf8_passthrough() {
        let mut pending = Vec::new();
        let out = decode_streaming(&mut pending, "hello".as_bytes());
        assert_eq!(out, "hello");
        assert!(pending.is_empty());
    }

    #[test]
    fn decode_streaming_empty_input() {
        let mut pending = Vec::new();
        let out = decode_streaming(&mut pending, &[]);
        assert_eq!(out, "");
        assert!(pending.is_empty());
    }

    #[test]
    fn decode_streaming_only_incomplete() {
        let mut pending = Vec::new();
        // 2-byte lead (0xc2) without continuation
        let out = decode_streaming(&mut pending, &[0xc2]);
        assert_eq!(out, "");
        assert_eq!(pending, vec![0xc2]);
        // Now complete it
        let out2 = decode_streaming(&mut pending, &[0xa9]);
        assert_eq!(out2, "\u{00a9}"); // ©
        assert!(pending.is_empty());
    }

    #[test]
    fn colored_bash_produces_ansi_codes() {
        let result = colored("/bin/bash", "\\[\\e[32m\\]", "", "test");
        assert!(result.contains("32m"));
        assert!(result.contains("test"));
    }

    #[test]
    fn colored_zsh_uses_colon_syntax() {
        let result = colored("/bin/zsh", "", "%F{green}", "test");
        assert!(result.contains("%F{green}"));
        assert!(result.contains("test"));
    }

    #[test]
    fn parse_fps_valid() {
        assert_eq!(parse_fps("15").unwrap(), 15);
        assert_eq!(parse_fps("24").unwrap(), 24);
        assert_eq!(parse_fps("30").unwrap(), 30);
    }

    #[test]
    fn parse_fps_invalid() {
        assert!(parse_fps("60").is_err());
        assert!(parse_fps("abc").is_err());
        assert!(parse_fps("0").is_err());
    }

    #[test]
    fn is_meta_command_edge_cases() {
        assert!(is_meta_command("demo stop"));
        assert!(is_meta_command("demo open https://example.com"));
        assert!(is_meta_command("demo focus main"));
        assert!(!is_meta_command("echo demo stop"));
        assert!(!is_meta_command("ls demo"));
        assert!(!is_meta_command(""));
        assert!(!is_meta_command("demo"));
        assert!(!is_meta_command("demo "));
    }

    #[test]
    fn clean_prompt_basic() {
        assert_eq!(clean_prompt("Password:"), "Password:");
        assert_eq!(clean_prompt("  Password:  "), "Password:");
    }

    #[test]
    fn ps1_text_various() {
        assert_eq!(ps1_text("no special chars"), "no special chars");
        assert_eq!(ps1_text("one\\slash"), "one\\\\slash");
        assert_eq!(ps1_text("\\n"), "\\\\n");
    }

    #[test]
    fn utf8_len_ascii() {
        assert_eq!(utf8_len(b'A'), 1);
        assert_eq!(utf8_len(b' '), 1);
        assert_eq!(utf8_len(b'0'), 1);
    }

    #[test]
    fn decode_streaming_multiple_chunks() {
        let mut pending = Vec::new();
        let out1 = decode_streaming(&mut pending, "hel".as_bytes());
        assert_eq!(out1, "hel");
        let out2 = decode_streaming(&mut pending, "lo\n".as_bytes());
        assert_eq!(out2, "lo\n");
        assert!(pending.is_empty());
    }

    #[test]
    fn cue_matches_empty_pattern() {
        assert!(cue_matches("anything", ""));
    }

    #[test]
    fn reveal_summary_with_theme() {
        let r = Reveal {
            panes: vec![RevealPane {
                id: "web".into(),
                url: Some("https://x.com".into()),
                theme: Some("dark".into()),
            }],
            orientation: Orientation::Horizontal,
            hold_ms: Some(3000),
            scroll: false,
        };
        let s = r.summary();
        assert!(s.contains("web"));
        assert!(s.contains("Horizontal"));
    }

    #[test]
    fn debug_log_create_and_note() {
        let dir = std::env::temp_dir().join(format!("dbg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("debug.log");
        let t0 = Instant::now();
        let log = DebugLog::create(&log_path, t0).unwrap();
        log.note("hello world");
        log.chunk("PTY→", b"test\x1b[A");
        drop(log);
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("hello world"));
        assert!(content.contains("PTY→"));
        assert!(content.contains("test"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn debug_log_chunk_hex_format() {
        let dir = std::env::temp_dir().join(format!("dbg2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("debug.log");
        let t0 = Instant::now();
        let log = DebugLog::create(&log_path, t0).unwrap();
        log.chunk("IN", b"\x1b[32m");
        drop(log);
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("hex="));
        assert!(content.contains("1b "));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn debug_log_empty_chunk() {
        let dir = std::env::temp_dir().join(format!("dbg3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("debug.log");
        let t0 = Instant::now();
        let log = DebugLog::create(&log_path, t0).unwrap();
        log.chunk("PTY→", b"");
        drop(log);
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("0B"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn colored_bash_produces_ansi_codes_more() {
        let result = colored("/bin/bash", "\\[\\e[1;31m\\]", "", "test");
        assert!(result.contains("1;31m"));
        assert!(result.contains("test"));
        assert!(result.contains("\\[\\e[0m\\]"));
    }

    #[test]
    fn colored_zsh_uses_colon_syntax_more() {
        let result = colored("/bin/zsh", "", "%F{red}", "hello");
        assert!(result.contains("red"));
        assert!(result.contains("hello"));
    }

    #[test]
    fn default_prompt_bash_has_dollar() {
        let p = default_prompt("/bin/bash");
        assert!(p.contains("$ "));
        assert!(p.contains("user@demo"));
    }

    #[test]
    fn default_prompt_zsh_has_dollar() {
        let p = default_prompt("/bin/zsh");
        assert!(p.contains("$ "));
        assert!(p.contains("user@demo"));
    }

    #[test]
    fn clean_prompt_strips_question_mark_prefix() {
        assert_eq!(clean_prompt("? Password:"), "Password:");
    }

    #[test]
    fn clean_prompt_strips_arrow_prefix() {
        assert_eq!(clean_prompt("> Enter secret:"), "Enter secret:");
    }

    #[test]
    fn clean_prompt_strips_diamond_prefix() {
        assert_eq!(clean_prompt("◆ Token:"), "Token:");
    }

    #[test]
    fn clean_prompt_strips_bullet_prefix() {
        assert_eq!(clean_prompt("● API key:"), "API key:");
    }

    #[test]
    fn clean_prompt_strips_asterisk_prefix() {
        assert_eq!(clean_prompt("* Secret:"), "Secret:");
    }

    #[test]
    fn clean_prompt_preserves_literal_bracket_without_final_letter() {
        // A `[` followed by digits but no final letter is NOT a CSI — keep it.
        assert_eq!(clean_prompt("item[1]"), "item[1]");
    }

    #[test]
    fn clean_prompt_empty_string() {
        assert_eq!(clean_prompt(""), "");
    }

    #[test]
    fn is_secret_prompt_rejects_long_lines() {
        let long = format!("{}:", "x".repeat(300));
        assert!(!is_secret_prompt(&long));
    }

    #[test]
    fn is_secret_prompt_rejects_no_colon_or_question() {
        assert!(!is_secret_prompt("Password"));
        assert!(!is_secret_prompt("secret"));
    }

    #[test]
    fn is_secret_prompt_accepts_question_mark() {
        assert!(is_secret_prompt("Enter passphrase?"));
    }

    #[test]
    fn is_secret_prompt_accepts_token() {
        assert!(is_secret_prompt("Token:"));
    }

    #[test]
    fn is_secret_prompt_accepts_api_key() {
        assert!(is_secret_prompt("API key:"));
    }

    #[test]
    fn is_secret_prompt_accepts_access_key() {
        assert!(is_secret_prompt("Access key:"));
    }

    #[test]
    fn is_secret_prompt_accepts_credential() {
        assert!(is_secret_prompt("Credential:"));
    }

    #[test]
    fn is_secret_prompt_accepts_verification_code() {
        assert!(is_secret_prompt("Verification code:"));
    }

    #[test]
    fn is_secret_prompt_rejects_case_insensitive() {
        // Should match regardless of case
        assert!(is_secret_prompt("PASSWORD:"));
        assert!(is_secret_prompt("PASSPHRASE:"));
    }

    #[test]
    fn is_secret_prompt_rejects_tui_painting() {
        // TUI paint without colon/question
        assert!(!is_secret_prompt("some random text without prompt marker"));
    }

    #[test]
    fn track_and_detect_multiple_lines() {
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut line = String::new();
        track_and_detect(
            &mut line,
            "hello\nworld\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        assert!(!sensitive.load(Ordering::SeqCst));
        assert_eq!(line, "");
    }

    #[test]
    fn track_and_detect_partial_line() {
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut line = String::new();
        track_and_detect(
            &mut line,
            "partial",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        assert!(!sensitive.load(Ordering::SeqCst));
        assert_eq!(line, "partial");
    }

    #[test]
    fn track_and_detect_csi_residue_in_line() {
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Vault passphrase:\r\x1b[?25h",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        assert!(sensitive.load(Ordering::SeqCst));
    }

    #[test]
    fn track_and_detect_control_chars_ignored() {
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut line = String::new();
        track_and_detect(
            &mut line,
            "abc\x01\x02\x03def",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        assert_eq!(line, "abcdef");
    }

    #[test]
    fn cue_matches_regex_prefix() {
        assert!(cue_matches("done in 123ms", "re:\\d+ms"));
        assert!(!cue_matches("no numbers", "re:\\d+ms"));
    }

    #[test]
    fn cue_matches_regex_invalid_is_ignored() {
        // Invalid regex should not match
        assert!(!cue_matches("anything", "re:[invalid"));
    }

    #[test]
    fn cue_matches_substring() {
        assert!(cue_matches("Report generated successfully.", "Report"));
        assert!(!cue_matches("Report generated", "Error"));
    }

    #[test]
    fn parse_reveal_empty_panes() {
        let v = serde_json::json!({"cmd": "reveal", "panes": []});
        assert!(parse_reveal(&v).is_none());
    }

    #[test]
    fn parse_reveal_missing_panes() {
        let v = serde_json::json!({"cmd": "reveal"});
        assert!(parse_reveal(&v).is_none());
    }

    #[test]
    fn parse_reveal_with_when() {
        let v = serde_json::json!({
            "cmd": "reveal",
            "panes": [{"id": "main"}],
            "orientation": "horizontal",
            "when": "some pattern",
        });
        let r = parse_reveal(&v).unwrap();
        // 'when' is not part of Reveal struct, so just verify parsing works
        assert_eq!(r.panes.len(), 1);
        assert_eq!(r.orientation, Orientation::Horizontal);
    }

    #[test]
    fn reveal_to_event_with_scroll_false() {
        let r = Reveal {
            panes: vec![RevealPane::terminal()],
            orientation: Orientation::Horizontal,
            hold_ms: None,
            scroll: false,
        };
        let ev = r.to_event(500);
        if let RawEvent::Reveal { scroll, .. } = ev {
            assert!(!scroll);
        } else {
            panic!("expected Reveal");
        }
    }

    #[test]
    fn reveal_to_event_with_hold_none() {
        let r = Reveal {
            panes: vec![RevealPane::terminal()],
            orientation: Orientation::Vertical,
            hold_ms: None,
            scroll: false,
        };
        let ev = r.to_event(100);
        if let RawEvent::Reveal { hold_ms, .. } = ev {
            assert_eq!(hold_ms, None);
        } else {
            panic!("expected Reveal");
        }
    }

    #[test]
    fn canvas_from_aspect_quality_case_insensitive_fullhd() {
        assert_eq!(
            canvas_from_aspect_quality("16:9", "FullHD").unwrap(),
            (1920, 1080)
        );
    }

    #[test]
    fn canvas_from_aspect_quality_case_insensitive_hd() {
        assert_eq!(canvas_from_aspect_quality("1:1", "HD").unwrap(), (720, 720));
    }

    #[test]
    fn canvas_from_aspect_quality_invalid_aspect() {
        assert!(canvas_from_aspect_quality("3:2", "fullhd").is_err());
    }

    #[test]
    fn canvas_from_aspect_quality_invalid_quality() {
        assert!(canvas_from_aspect_quality("16:9", "4k").is_err());
    }

    #[test]
    fn parse_resolution_auto() {
        assert_eq!(parse_resolution("auto").unwrap(), None);
    }

    #[test]
    fn parse_resolution_invalid_format() {
        assert!(parse_resolution("huge").is_err());
    }

    #[test]
    fn parse_resolution_zero_width() {
        assert!(parse_resolution("0x100").is_err());
    }

    #[test]
    fn parse_resolution_zero_height() {
        assert!(parse_resolution("100x0").is_err());
    }

    #[test]
    fn parse_fps_whitespace() {
        assert_eq!(parse_fps("  15  ").unwrap(), 15);
    }

    #[test]
    fn parse_fps_negative() {
        assert!(parse_fps("-1").is_err());
    }

    #[test]
    fn is_meta_command_demo_focus_with_args() {
        assert!(is_meta_command("demo focus main docs"));
    }

    #[test]
    fn is_meta_command_demo_open_with_url() {
        assert!(is_meta_command("demo open http://example.com"));
    }

    #[test]
    fn is_meta_command_demo_stop() {
        assert!(is_meta_command("demo stop"));
    }

    #[test]
    fn is_meta_command_leading_whitespace() {
        assert!(is_meta_command("  demo stop"));
    }

    #[test]
    fn is_meta_command_not_a_command() {
        assert!(!is_meta_command("echo demo stop"));
    }

    #[test]
    fn is_meta_command_empty() {
        assert!(!is_meta_command(""));
    }

    #[test]
    fn is_meta_command_just_demo() {
        assert!(!is_meta_command("demo"));
    }

    #[test]
    fn is_meta_command_demo_space() {
        assert!(!is_meta_command("demo "));
    }

    #[test]
    fn ps1_text_backslash_at_start() {
        assert_eq!(ps1_text(r"\start"), r"\\start");
    }

    #[test]
    fn ps1_text_multiple_backslashes() {
        assert_eq!(ps1_text(r"\a\b\c"), r"\\a\\b\\c");
    }

    #[test]
    fn utf8_len_continuation_byte() {
        assert_eq!(utf8_len(0x80), 1);
    }

    #[test]
    fn utf8_len_two_byte_lead() {
        assert_eq!(utf8_len(0xc0), 2);
    }

    #[test]
    fn utf8_len_three_byte_lead() {
        assert_eq!(utf8_len(0xe0), 3);
    }

    #[test]
    fn utf8_len_four_byte_lead() {
        assert_eq!(utf8_len(0xf0), 4);
    }

    #[test]
    fn utf8_len_ascii_max() {
        assert_eq!(utf8_len(0x7f), 1);
    }

    #[test]
    fn decode_streaming_rejects_overlong_encoding() {
        let mut pending = Vec::new();
        // Overlong encoding of '/' (0x2f) as 0xc0 0xaf — invalid UTF-8
        let out = decode_streaming(&mut pending, &[0xc0, 0xaf]);
        // Should produce replacement character
        assert!(out.contains('\u{FFFD}') || out.is_empty());
    }

    #[test]
    fn decode_streaming_rejects_surrogate_half() {
        let mut pending = Vec::new();
        // Surrogate half (0xED 0xA0 0x80 = U+D800) is invalid UTF-8.
        // It should be replaced with U+FFFD.
        let out = decode_streaming(&mut pending, &[0xed, 0xa0, 0x80]);
        assert!(!out.is_empty());
        // The output should contain the replacement character
        assert!(out.contains('\u{FFFD}') || out.contains('\u{fffd}'));
        assert!(pending.is_empty());
    }

    #[test]
    fn route_input_chunk_echoes_normal_text() {
        let (to_pty, mute) = route(&[b"hello world\n"]);
        assert_eq!(to_pty, b"hello world\n");
        assert!(!mute);
    }

    #[test]
    fn route_input_chunk_split_across_chunks() {
        let (to_pty, _) = route(&[b"hel", b"lo\n"]);
        assert_eq!(to_pty, b"hello\n");
    }

    #[test]
    fn route_input_chunk_backspace() {
        let (to_pty, _) = route(&[b"ab\x7f"]);
        assert_eq!(to_pty, b"ab\x7f");
    }

    #[test]
    fn route_input_chunk_utf8() {
        let (to_pty, _) = route(&["café\n".as_bytes()]);
        assert_eq!(to_pty, "café\n".as_bytes());
    }

    #[test]
    fn route_input_chunk_meta_command_across_chunks() {
        let (_, mute) = route(&[b"demo ", b"stop\n"]);
        assert!(mute);
    }

    #[test]
    fn route_input_chunk_not_meta_command() {
        let (_, mute) = route(&[b"demodocs\n"]);
        assert!(!mute);
    }

    #[test]
    fn track_and_detect_secret_at_newline_boundary() {
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Password:\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        assert!(sensitive.load(Ordering::SeqCst));
    }

    #[test]
    fn track_and_detect_no_secret_without_colon() {
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Password\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        assert!(!sensitive.load(Ordering::SeqCst));
    }

    #[test]
    fn track_and_detect_long_line_not_truncated() {
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut line = String::new();
        // Line under MAX_PROMPT_LINE should be kept
        let short = "a".repeat(100);
        track_and_detect(
            &mut line,
            &short,
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        assert_eq!(line.len(), 100);
    }

    #[test]
    fn track_and_detect_long_line_truncated() {
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut line = String::new();
        // Line over MAX_PROMPT_LINE should be truncated
        let long = "a".repeat(300);
        track_and_detect(
            &mut line,
            &long,
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        assert!(line.len() <= MAX_PROMPT_LINE);
    }

    #[test]
    fn clean_prompt_csi_sequence_with_params() {
        assert_eq!(clean_prompt("[38;5;10mPassword:[39m"), "Password:");
    }

    #[test]
    fn clean_prompt_csi_sequence_cursor() {
        assert_eq!(clean_prompt("[?25lPassword:"), "Password:");
    }

    #[test]
    fn clean_prompt_mixed_content() {
        assert_eq!(clean_prompt("[31m[?25l> Password:"), "Password:");
    }

    #[test]
    fn is_secret_prompt_with_space_before_colon() {
        assert!(is_secret_prompt("Password :"));
    }

    #[test]
    fn is_secret_prompt_with_tab() {
        assert!(is_secret_prompt("Password:\t"));
    }

    #[test]
    fn is_secret_prompt_case_insensitive() {
        assert!(is_secret_prompt("PASSWORD:"));
        assert!(is_secret_prompt("Passphrase:"));
        assert!(is_secret_prompt("PASSPHRASE:"));
    }

    #[test]
    fn is_secret_prompt_with_leading_whitespace() {
        assert!(is_secret_prompt("  Password:"));
    }

    #[test]
    fn ps1_text_backslash_in_middle() {
        assert_eq!(ps1_text("a\\b"), "a\\\\b");
    }

    #[test]
    fn ps1_text_multiple_consecutive_backslashes() {
        assert_eq!(ps1_text("\\\\a"), "\\\\\\\\a");
    }

    #[test]
    fn colored_bash_basic() {
        let result = colored("/bin/bash", "\\[\\e[32m\\]", "", "test");
        assert!(result.contains("32m"));
        assert!(result.contains("test"));
        assert!(result.contains("\\[\\e[0m\\]"));
    }

    #[test]
    fn colored_zsh_basic() {
        let result = colored("/bin/zsh", "", "%F{green}", "test");
        assert!(result.contains("%F{green}"));
        assert!(result.contains("test"));
    }

    #[test]
    fn default_prompt_bash_contains_dollar() {
        let p = default_prompt("/bin/bash");
        assert!(p.contains("$ "));
        assert!(p.contains("user@demo"));
    }

    #[test]
    fn default_prompt_zsh_contains_dollar() {
        let p = default_prompt("/bin/zsh");
        assert!(p.contains("$ "));
        assert!(p.contains("user@demo"));
    }

    #[test]
    fn cue_matches_regex_pattern() {
        assert!(cue_matches("done in 123ms", "re:\\d+ms"));
        assert!(!cue_matches("no numbers", "re:\\d+ms"));
    }

    #[test]
    fn cue_matches_plain_substring_long() {
        assert!(cue_matches("Report generated successfully.", "Report"));
        assert!(!cue_matches("Report generated", "Error"));
    }

    #[test]
    fn cue_matches_empty_pattern_matches_anything() {
        assert!(cue_matches("anything", ""));
    }

    #[test]
    fn parse_reveal_with_theme() {
        let v = serde_json::json!({
            "cmd": "reveal",
            "panes": [{"id": "main", "theme": "dark"}],
            "orientation": "horizontal",
        });
        let r = parse_reveal(&v).unwrap();
        assert_eq!(r.panes.len(), 1);
        assert_eq!(r.panes[0].theme.as_deref(), Some("dark"));
    }

    #[test]
    fn parse_reveal_with_url() {
        let v = serde_json::json!({
            "cmd": "reveal",
            "panes": [{"id": "browser", "url": "https://example.com"}],
            "orientation": "vertical",
        });
        let r = parse_reveal(&v).unwrap();
        assert_eq!(r.orientation, Orientation::Vertical);
        assert_eq!(r.panes[0].url.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn decode_streaming_complete_utf8() {
        let mut pending = Vec::new();
        let out = decode_streaming(&mut pending, "hello world".as_bytes());
        assert_eq!(out, "hello world");
        assert!(pending.is_empty());
    }

    #[test]
    fn decode_streaming_split_emoji() {
        let mut pending = Vec::new();
        // 🎉 is 4 bytes: f0 9f 8e 89
        let emoji = "🎉";
        let bytes = emoji.as_bytes();
        let first = decode_streaming(&mut pending, &bytes[..2]);
        assert_eq!(first, "");
        let second = decode_streaming(&mut pending, &bytes[2..]);
        assert_eq!(second, emoji);
        assert!(pending.is_empty());
    }

    #[test]
    fn decode_streaming_empty_pending() {
        let mut pending = Vec::new();
        let out = decode_streaming(&mut pending, &[]);
        assert_eq!(out, "");
        assert!(pending.is_empty());
    }

    #[test]
    fn route_input_chunk_multiple_chars() {
        let (to_pty, _) = route(&[b"abc\n"]);
        assert_eq!(to_pty, b"abc\n");
    }

    #[test]
    fn route_input_chunk_control_chars() {
        let (to_pty, _) = route(&[b"\x03"]);
        assert_eq!(to_pty, b"\x03");
    }

    #[test]
    fn utf8_len_all_ranges() {
        // ASCII
        assert_eq!(utf8_len(0x00), 1);
        assert_eq!(utf8_len(0x7f), 1);
        // Continuation bytes
        assert_eq!(utf8_len(0x80), 1);
        assert_eq!(utf8_len(0xbf), 1);
        // 2-byte leads
        assert_eq!(utf8_len(0xc0), 2);
        assert_eq!(utf8_len(0xdf), 2);
        // 3-byte leads
        assert_eq!(utf8_len(0xe0), 3);
        assert_eq!(utf8_len(0xef), 3);
        // 4-byte leads
        assert_eq!(utf8_len(0xf0), 4);
        assert_eq!(utf8_len(0xf4), 4);
    }

    #[test]
    fn is_meta_command_variations() {
        assert!(is_meta_command("demo stop"));
        assert!(is_meta_command("demo open http://example.com"));
        assert!(is_meta_command("demo focus main"));
        assert!(!is_meta_command("echo demo stop"));
        assert!(!is_meta_command("ls"));
        assert!(!is_meta_command(""));
        assert!(!is_meta_command("demo"));
        assert!(!is_meta_command("demo "));
    }

    #[test]
    fn secret_step_on_submit_no_guard_records() {
        let mut last: Option<String> = None;
        assert!(secret_step_on_submit(&mut last, false, "Password:"));
        assert_eq!(last.as_deref(), Some("Password:"));
    }

    #[test]
    fn secret_step_on_submit_same_prompt_is_dup() {
        let mut last = Some("Password:".to_string());
        assert!(!secret_step_on_submit(&mut last, false, "Password:"));
    }

    #[test]
    fn secret_step_on_submit_different_prompt_records() {
        let mut last = Some("Password:".to_string());
        assert!(secret_step_on_submit(&mut last, false, "Token:"));
        assert_eq!(last.as_deref(), Some("Token:"));
    }

    #[test]
    fn secret_step_on_submit_clears_guard_when_prompt_left_screen() {
        let mut last = Some("Password:".to_string());
        assert!(secret_step_on_submit(&mut last, true, "Password:"));
        assert_eq!(last.as_deref(), Some("Password:"));
    }

    #[test]
    fn secret_dedup_same_prompt_with_submission_produces_two_events() {
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut last_secret_prompt: Option<String> = None;
        let mut events = Vec::new();

        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Password:\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        let prompt = secret_prompt.lock().unwrap().take().unwrap();
        let cleared = secret_prompt_cleared.swap(false, Ordering::SeqCst);
        if secret_step_on_submit(&mut last_secret_prompt, cleared, &prompt) {
            events.push(prompt);
        }
        sensitive.store(false, Ordering::SeqCst);

        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Welcome to sudo\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        assert!(secret_prompt_cleared.load(Ordering::SeqCst));

        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Password:\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        let prompt = secret_prompt.lock().unwrap().take().unwrap();
        let cleared = secret_prompt_cleared.swap(false, Ordering::SeqCst);
        if secret_step_on_submit(&mut last_secret_prompt, cleared, &prompt) {
            events.push(prompt);
        }

        assert_eq!(events.len(), 2);
    }

    #[test]
    fn secret_dedup_same_prompt_no_submission_produces_one_event() {
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut last_secret_prompt: Option<String> = None;
        let mut events = Vec::new();

        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Password:\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );

        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Password:\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );

        let prompt = secret_prompt.lock().unwrap().take().unwrap();
        let cleared = secret_prompt_cleared.swap(false, Ordering::SeqCst);
        if secret_step_on_submit(&mut last_secret_prompt, cleared, &prompt) {
            events.push(prompt);
        }

        assert_eq!(events.len(), 1);
    }

    #[test]
    fn secret_dedup_different_prompts_produce_two_events() {
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut last_secret_prompt: Option<String> = None;
        let mut events = Vec::new();

        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Password:\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        let prompt = secret_prompt.lock().unwrap().take().unwrap();
        let cleared = secret_prompt_cleared.swap(false, Ordering::SeqCst);
        if secret_step_on_submit(&mut last_secret_prompt, cleared, &prompt) {
            events.push(prompt);
        }
        sensitive.store(false, Ordering::SeqCst);

        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Some output\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );

        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Token:\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        let prompt = secret_prompt.lock().unwrap().take().unwrap();
        let cleared = secret_prompt_cleared.swap(false, Ordering::SeqCst);
        if secret_step_on_submit(&mut last_secret_prompt, cleared, &prompt) {
            events.push(prompt);
        }

        assert_eq!(events.len(), 2);
    }

    #[test]
    fn secret_dedup_guard_must_be_cleared_by_prompt_leaving_screen() {
        // Exercises the extracted helper directly: same prompt submitted twice
        // with prompt_left_screen=true between them must produce two events.
        // If the guard were capture-wide (prompt_left_screen ignored), the
        // second call would return false and this test would fail.
        let mut last: Option<String> = None;
        assert!(secret_step_on_submit(&mut last, false, "Password:"));
        assert!(secret_step_on_submit(&mut last, true, "Password:"));
    }

    #[test]
    fn secret_dedup_immediate_redraw_after_enter_is_suppressed() {
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut last_secret_prompt: Option<String> = None;
        let mut events = Vec::new();

        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Password:\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        let prompt = secret_prompt.lock().unwrap().take().unwrap();
        let cleared = secret_prompt_cleared.swap(false, Ordering::SeqCst);
        if secret_step_on_submit(&mut last_secret_prompt, cleared, &prompt) {
            events.push(prompt);
        }
        sensitive.store(false, Ordering::SeqCst);

        let mut line = String::new();
        track_and_detect(
            &mut line,
            "Password:\n",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        let prompt = secret_prompt.lock().unwrap().take().unwrap();
        let cleared = secret_prompt_cleared.swap(false, Ordering::SeqCst);
        if secret_step_on_submit(&mut last_secret_prompt, cleared, &prompt) {
            events.push(prompt);
        }

        assert_eq!(events.len(), 1);
    }

    #[test]
    fn secret_prompt_cleared_not_set_on_partial_line() {
        // A prompt split across PTY reads must not spuriously set the cleared
        // flag: "[sudo] password for " at chunk end (no newline) is a partial
        // line, not a completed non-secret line.
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let secret_prompt_cleared = AtomicBool::new(false);
        let mut line = String::new();
        track_and_detect(
            &mut line,
            "[sudo] password for ",
            &sensitive,
            &secret_prompt,
            &secret_prompt_cleared,
        );
        assert!(
            !secret_prompt_cleared.load(Ordering::SeqCst),
            "partial line at chunk end must not set secret_prompt_cleared"
        );
    }

    fn test_reveal() -> Reveal {
        Reveal {
            panes: vec![RevealPane {
                id: "main".into(),
                url: Some("http://example.com".into()),
                theme: None,
            }],
            orientation: Orientation::Horizontal,
            hold_ms: None,
            scroll: false,
        }
    }

    #[test]
    fn read_control_arms_after_running_when_queueing_after_reveal() {
        let cpath = std::env::temp_dir().join(format!("demo-test-control-{}", std::process::id()));
        let cmd = serde_json::json!({
            "cmd": "reveal",
            "after": true,
            "panes": [{"id": "main", "url": "http://example.com"}],
        });
        std::fs::write(&cpath, serde_json::to_string(&cmd).unwrap()).unwrap();

        let events: Arc<Mutex<Vec<RawEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let pending: PendingWhen = Arc::new(Mutex::new(Vec::new()));
        let after: PendingAfter = Arc::new(Mutex::new(Vec::new()));
        let after_running = AtomicBool::new(false);
        let after_last_out = Mutex::new(Instant::now());
        let muting = Arc::new(AtomicBool::new(false));
        let mute_start: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let mute_spans: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let t0 = Instant::now();
        let mut read = 0u64;

        let result = read_control(
            &cpath,
            &mut read,
            &events,
            &pending,
            &after,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            None,
        );

        assert!(result.is_none());
        assert!(
            after_running.load(Ordering::SeqCst),
            "--after must arm after_running immediately so the current command is tracked"
        );
        assert_eq!(after.lock().unwrap().len(), 1);
        assert!(events.lock().unwrap().is_empty(), "no immediate reveal");
        let _ = std::fs::remove_file(&cpath);
    }

    #[test]
    fn drain_remaining_reveals_emits_after_queue_as_events() {
        let after: PendingAfter = Arc::new(Mutex::new(vec![test_reveal()]));
        let pending: PendingWhen = Arc::new(Mutex::new(Vec::new()));
        let mut events = Vec::new();

        let summary = drain_remaining_reveals(&after, &pending, &mut events, 9999);

        assert_eq!(summary.after_summaries.len(), 1);
        assert!(summary.when_unmatched.is_empty());
        assert_eq!(events.len(), 1);
        assert!(after.lock().unwrap().is_empty());
        match &events[0] {
            RawEvent::Reveal { t_ms, .. } => assert_eq!(*t_ms, 9999),
            other => panic!("expected Reveal, got {other:?}"),
        }
    }

    #[test]
    fn drain_remaining_reveals_reports_unmatched_when_cues() {
        let after: PendingAfter = Arc::new(Mutex::new(Vec::new()));
        let pending: PendingWhen = Arc::new(Mutex::new(vec![(
            test_reveal(),
            "never-gonna-appear".into(),
        )]));
        let mut events = Vec::new();

        let summary = drain_remaining_reveals(&after, &pending, &mut events, 5000);

        assert_eq!(summary.after_summaries.len(), 0);
        assert_eq!(
            summary.when_unmatched,
            vec!["never-gonna-appear".to_string()]
        );
        assert_eq!(events.len(), 1);
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn drain_remaining_reveals_handles_both_queues_at_once() {
        let after: PendingAfter = Arc::new(Mutex::new(vec![test_reveal()]));
        let pending: PendingWhen = Arc::new(Mutex::new(vec![
            (test_reveal(), "cue-alpha".into()),
            (test_reveal(), "re:cue-beta".into()),
        ]));
        let mut events = Vec::new();

        let summary = drain_remaining_reveals(&after, &pending, &mut events, 1000);

        assert_eq!(summary.after_summaries.len(), 1);
        assert_eq!(summary.when_unmatched.len(), 2);
        assert_eq!(events.len(), 3);
        assert!(after.lock().unwrap().is_empty());
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn drained_reveal_survives_from_raw_after_demo_stop() {
        let mut events = vec![
            RawEvent::Output {
                t_ms: 100,
                data: "real output".into(),
            },
            RawEvent::Input {
                t_ms: 2000,
                bytes: "demo stop\r".into(),
            },
            RawEvent::Output {
                t_ms: 2010,
                data: "demo stop".into(),
            },
        ];
        let after: PendingAfter = Arc::new(Mutex::new(vec![test_reveal()]));
        let pending: PendingWhen =
            Arc::new(Mutex::new(vec![(test_reveal(), "never-matched".into())]));
        let raw_for_cutoff = crate::model::RawMacro {
            meta: crate::model::RawMeta {
                shell: String::new(),
                cols: 0,
                rows: 0,
                idle_timeout_ms: 0,
                resolution: None,
                fps: None,
                stage: None,
                mute_spans: Vec::new(),
            },
            events: events.clone(),
        };
        let drain_ts = recording::stop_cutoff_ms(&raw_for_cutoff)
            .map(|c| c.saturating_sub(1))
            .unwrap_or_else(|| {
                events
                    .iter()
                    .filter_map(|e| match e {
                        RawEvent::Output { t_ms, .. } => Some(*t_ms),
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0)
            });
        drain_remaining_reveals(&after, &pending, &mut events, drain_ts);
        let raw = crate::model::RawMacro {
            meta: crate::model::RawMeta {
                shell: "/bin/bash".into(),
                cols: 80,
                rows: 24,
                idle_timeout_ms: 0,
                resolution: None,
                fps: None,
                stage: None,
                mute_spans: Vec::new(),
            },
            events,
        };
        let (rec, layout, _) = recording::from_raw(&raw, "t");
        assert!(
            rec.events.iter().any(|(_, data)| data == "real output"),
            "real output must survive"
        );
        assert!(
            !layout.panes.is_empty(),
            "drained reveal must survive normalization"
        );
        let has_browser = layout
            .panes
            .iter()
            .any(|p| p.kind == crate::model::PaneKind::Browser);
        assert!(
            has_browser,
            "drained --after reveal must produce a browser pane"
        );
    }

    #[test]
    fn reveal_cancel_closes_mute_span_without_recording_reveal() {
        let cpath = std::env::temp_dir().join(format!("demo-test-cancel-{}", std::process::id()));
        let cmd = serde_json::json!({ "cmd": "reveal_cancel" });
        std::fs::write(&cpath, serde_json::to_string(&cmd).unwrap()).unwrap();

        let events: Arc<Mutex<Vec<RawEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let pending: PendingWhen = Arc::new(Mutex::new(Vec::new()));
        let after: PendingAfter = Arc::new(Mutex::new(Vec::new()));
        let after_running = AtomicBool::new(false);
        let after_last_out = Mutex::new(Instant::now());
        let muting = Arc::new(AtomicBool::new(true));
        let mute_start: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(Some(1000)));
        let mute_spans: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let t0 = Instant::now();
        let mut read = 0u64;

        let result = read_control(
            &cpath,
            &mut read,
            &events,
            &pending,
            &after,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            None,
        );

        assert!(result.is_none());
        assert!(
            !muting.load(Ordering::SeqCst),
            "reveal_cancel must stop muting"
        );
        assert!(
            mute_start.lock().unwrap().is_none(),
            "reveal_cancel must take the mute_start"
        );
        assert_eq!(
            mute_spans.lock().unwrap().len(),
            1,
            "reveal_cancel must close the span"
        );
        assert!(
            events.lock().unwrap().is_empty(),
            "reveal_cancel must not record a reveal event"
        );
        let _ = std::fs::remove_file(&cpath);
    }

    #[test]
    fn reveal_closes_mute_span_and_records_event() {
        let cpath =
            std::env::temp_dir().join(format!("demo-test-reveal-close-{}", std::process::id()));
        let cmd = serde_json::json!({
            "cmd": "reveal",
            "panes": [{"id": "main", "url": "http://example.com"}],
        });
        std::fs::write(&cpath, serde_json::to_string(&cmd).unwrap()).unwrap();

        let events: Arc<Mutex<Vec<RawEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let pending: PendingWhen = Arc::new(Mutex::new(Vec::new()));
        let after: PendingAfter = Arc::new(Mutex::new(Vec::new()));
        let after_running = AtomicBool::new(false);
        let after_last_out = Mutex::new(Instant::now());
        let muting = Arc::new(AtomicBool::new(true));
        let mute_start: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(Some(1000)));
        let mute_spans: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let t0 = Instant::now();
        let mut read = 0u64;

        let result = read_control(
            &cpath,
            &mut read,
            &events,
            &pending,
            &after,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            None,
        );

        assert!(result.is_none());
        assert!(!muting.load(Ordering::SeqCst), "reveal must stop muting");
        assert!(
            mute_start.lock().unwrap().is_none(),
            "reveal must take the mute_start"
        );
        assert_eq!(
            mute_spans.lock().unwrap().len(),
            1,
            "reveal must close the span"
        );
        assert_eq!(
            events.lock().unwrap().len(),
            1,
            "reveal must record a reveal event"
        );
        let _ = std::fs::remove_file(&cpath);
    }

    #[test]
    fn double_close_is_safe() {
        let cpath =
            std::env::temp_dir().join(format!("demo-test-double-close-{}", std::process::id()));
        // First cancel, then reveal — both try to close the same span.
        let cmds = format!(
            "{}\n{}",
            serde_json::to_string(&serde_json::json!({ "cmd": "reveal_cancel" })).unwrap(),
            serde_json::to_string(&serde_json::json!({
                "cmd": "reveal",
                "panes": [{"id": "main", "url": "http://example.com"}],
            }))
            .unwrap()
        );
        std::fs::write(&cpath, cmds).unwrap();

        let events: Arc<Mutex<Vec<RawEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let pending: PendingWhen = Arc::new(Mutex::new(Vec::new()));
        let after: PendingAfter = Arc::new(Mutex::new(Vec::new()));
        let after_running = AtomicBool::new(false);
        let after_last_out = Mutex::new(Instant::now());
        let muting = Arc::new(AtomicBool::new(true));
        let mute_start: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(Some(1000)));
        let mute_spans: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let t0 = Instant::now();
        let mut read = 0u64;

        let result = read_control(
            &cpath,
            &mut read,
            &events,
            &pending,
            &after,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            None,
        );

        assert!(result.is_none());
        assert_eq!(
            mute_spans.lock().unwrap().len(),
            1,
            "only the first close must record a span (second finds mute_start empty)"
        );
        assert_eq!(
            events.lock().unwrap().len(),
            1,
            "reveal still records its event even after cancel closed the span"
        );
        let _ = std::fs::remove_file(&cpath);
    }

    #[test]
    fn safety_valve_closes_span_and_emits_diagnostic() {
        let t0 = Instant::now();
        let muting = AtomicBool::new(true);
        // Pretend muting started 91 seconds ago.
        let mute_since = Mutex::new(t0 - Duration::from_secs(91));
        let mute_start: Mutex<Option<u64>> = Mutex::new(Some(500));
        let mute_spans: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

        let log_path = std::env::temp_dir().join(format!("demo-test-valve-{}", std::process::id()));
        let debug = DebugLog::create(&log_path, t0).unwrap();

        let fired = maybe_close_safety_valve(
            &muting,
            &mute_since,
            &mute_start,
            &mute_spans,
            t0,
            Some(&debug),
        );

        assert!(fired, "safety valve must report that it fired");
        assert!(
            !muting.load(Ordering::SeqCst),
            "safety valve must stop muting"
        );
        assert!(
            mute_start.lock().unwrap().is_none(),
            "safety valve must take the mute_start"
        );
        assert_eq!(
            mute_spans.lock().unwrap().len(),
            1,
            "safety valve must close the span into mute_spans"
        );
        let log_contents = std::fs::read_to_string(&log_path).unwrap();
        assert!(
            log_contents.contains("safety valve:"),
            "safety valve must write its diagnostic to the debug log, got: {log_contents:?}"
        );
        let _ = std::fs::remove_file(&log_path);
    }

    #[test]
    fn safety_valve_does_not_fire_before_90s() {
        let t0 = Instant::now();
        let muting = AtomicBool::new(true);
        let mute_since = Mutex::new(t0 - Duration::from_secs(60));
        let mute_start: Mutex<Option<u64>> = Mutex::new(Some(500));
        let mute_spans: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

        let fired =
            maybe_close_safety_valve(&muting, &mute_since, &mute_start, &mute_spans, t0, None);

        assert!(!fired, "safety valve must not fire before 90s");
        assert!(
            muting.load(Ordering::SeqCst),
            "muting must remain on when valve hasn't fired"
        );
        assert_eq!(
            mute_spans.lock().unwrap().len(),
            0,
            "no span must be closed when valve hasn't fired"
        );
    }

    #[allow(clippy::type_complexity)]
    fn make_read_control_fixtures() -> (
        std::path::PathBuf,
        Arc<Mutex<Vec<RawEvent>>>,
        PendingWhen,
        PendingAfter,
        AtomicBool,
        Mutex<Instant>,
        Arc<AtomicBool>,
        Arc<Mutex<Option<u64>>>,
        Arc<Mutex<Vec<(u64, u64)>>>,
        Instant,
    ) {
        // Unique per CALL, not per process. `std::process::id()` alone is enough
        // under nextest, which gives every test its own process — and useless
        // under `cargo test`, which runs them as threads in one process, so two
        // tests sharing this fixture raced over the same file. CI runs
        // `cargo test`; a pid-keyed temp path is a test that only passes on the
        // runner that isolates it for you.
        static FIXTURE_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = FIXTURE_SEQ.fetch_add(1, Ordering::SeqCst);
        let cpath = std::env::temp_dir().join(format!(
            "demo-test-final-drain-{}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&cpath);
        let events: Arc<Mutex<Vec<RawEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let pending: PendingWhen = Arc::new(Mutex::new(Vec::new()));
        let after: PendingAfter = Arc::new(Mutex::new(Vec::new()));
        let after_running = AtomicBool::new(false);
        let after_last_out = Mutex::new(Instant::now());
        let muting = Arc::new(AtomicBool::new(false));
        let mute_start: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let mute_spans: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let t0 = Instant::now();
        (
            cpath,
            events,
            pending,
            after,
            after_running,
            after_last_out,
            muting,
            mute_start,
            mute_spans,
            t0,
        )
    }

    #[test]
    fn final_drain_picks_up_control_line_appended_after_last_watchdog_read() {
        let (
            cpath,
            events,
            pending,
            after,
            after_running,
            after_last_out,
            muting,
            mute_start,
            mute_spans,
            t0,
        ) = make_read_control_fixtures();
        let mut read = 0u64;

        std::fs::write(&cpath, "").unwrap();
        let _ = read_control(
            &cpath,
            &mut read,
            &events,
            &pending,
            &after,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            None,
        );
        assert_eq!(read, 0);

        let reveal = serde_json::to_string(&serde_json::json!({
            "cmd": "reveal",
            "panes": [{"id": "main", "url": "http://example.com"}],
        }))
        .unwrap();
        std::fs::write(&cpath, format!("{reveal}\n")).unwrap();

        let _ = read_control(
            &cpath,
            &mut read,
            &events,
            &pending,
            &after,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            None,
        );

        assert_eq!(
            events.lock().unwrap().len(),
            1,
            "final drain must process a reveal appended after the last watchdog pass"
        );
        let _ = std::fs::remove_file(&cpath);
    }

    #[test]
    fn final_drain_on_empty_or_fully_consumed_file_is_noop() {
        let (
            cpath,
            events,
            pending,
            after,
            after_running,
            after_last_out,
            muting,
            mute_start,
            mute_spans,
            t0,
        ) = make_read_control_fixtures();
        let mut read = 0u64;

        std::fs::write(&cpath, "").unwrap();
        let result = read_control(
            &cpath,
            &mut read,
            &events,
            &pending,
            &after,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            None,
        );
        assert!(result.is_none());
        assert_eq!(read, 0);
        assert!(events.lock().unwrap().is_empty());

        let reveal = serde_json::to_string(&serde_json::json!({
            "cmd": "reveal",
            "panes": [{"id": "main", "url": "http://example.com"}],
        }))
        .unwrap();
        std::fs::write(&cpath, format!("{reveal}\n")).unwrap();
        let _ = read_control(
            &cpath,
            &mut read,
            &events,
            &pending,
            &after,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            None,
        );
        assert_eq!(events.lock().unwrap().len(), 1);
        let offset_after_first = read;

        let result = read_control(
            &cpath,
            &mut read,
            &events,
            &pending,
            &after,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            None,
        );
        assert!(result.is_none());
        assert_eq!(
            read, offset_after_first,
            "offset must not change on empty re-read"
        );
        assert_eq!(events.lock().unwrap().len(), 1, "no duplicate events");
        let _ = std::fs::remove_file(&cpath);
    }

    #[test]
    fn byte_offset_never_rewinds_and_torn_line_is_not_double_consumed() {
        let (
            cpath,
            events,
            pending,
            after,
            after_running,
            after_last_out,
            muting,
            mute_start,
            mute_spans,
            t0,
        ) = make_read_control_fixtures();
        let mut read = 0u64;

        std::fs::write(&cpath, "tea").unwrap();
        let _ = read_control(
            &cpath,
            &mut read,
            &events,
            &pending,
            &after,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            None,
        );
        let offset_after_partial = read;
        assert_eq!(
            offset_after_partial, 3,
            "offset must advance past the partial bytes"
        );

        std::fs::write(&cpath, "tear\n").unwrap();
        let offset_before = read;
        let _ = read_control(
            &cpath,
            &mut read,
            &events,
            &pending,
            &after,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            None,
        );
        assert!(
            read >= offset_before,
            "offset must never rewind (was {offset_before}, now {read})"
        );
        assert!(
            events.lock().unwrap().is_empty(),
            "torn partial must not produce a phantom event"
        );

        let reveal = serde_json::to_string(&serde_json::json!({
            "cmd": "reveal",
            "panes": [{"id": "main", "url": "http://example.com"}],
        }))
        .unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&cpath)
            .unwrap();
        use std::io::Write;
        writeln!(file, "{reveal}").unwrap();
        drop(file);

        let _ = read_control(
            &cpath,
            &mut read,
            &events,
            &pending,
            &after,
            &after_running,
            &after_last_out,
            &muting,
            &mute_start,
            &mute_spans,
            t0,
            None,
        );
        assert_eq!(
            events.lock().unwrap().len(),
            1,
            "reveal appended after the torn line must be processed exactly once"
        );
        let _ = std::fs::remove_file(&cpath);
    }
}
