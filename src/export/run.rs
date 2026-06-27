//! Executes a terminal-only score in a real PTY and captures the output as a
//! timestamped recording — the basis for the cast/html/gif targets.
//!
//! The shell echoes typed characters, so pacing the writes (humanized typing)
//! produces a natural char-by-char appearance in the capture. A clean `PS1` is
//! forced before the clock starts, so demos never leak `user@host`.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use vt100::Parser as VtParser;

use crate::error::{Error, Result};
use crate::model::{PaneKind, Score, Step};
use crate::normalize::salt::humanize_delays;
use crate::normalize::Rng;

/// Assumed monospace cell size (px), inverse of the normalizer's sizing.
const CELL_W: u32 = 10;
const CELL_H: u32 = 20;
/// Built-in demo prompt (bash `PS1`): a realistic, generic Linux prompt —
/// `user@demo:~$` with the Ubuntu-style green user@host and blue path — used when
/// the score pins none. Generic on purpose (never the real user/host). Set
/// `[demo] prompt` to customize.
pub const DEFAULT_PROMPT: &str =
    "\\[\\e[1;32m\\]user@demo\\[\\e[0m\\]:\\[\\e[1;34m\\]~\\[\\e[0m\\]$ ";

/// Returns true if the shell path looks like zsh.
pub fn is_zsh(shell: &str) -> bool {
    shell.ends_with("/zsh") || shell == "zsh"
}

/// Build a shell-appropriate default prompt string.
pub fn default_prompt_for(shell: &str) -> String {
    if is_zsh(shell) {
        "%B%F{green}user@demo%f%b:%B%F{blue}~%f%b$ ".to_string()
    } else {
        DEFAULT_PROMPT.to_string()
    }
}
/// Default seed when the score pins none.
const DEFAULT_SEED: u64 = 0xD370_5EED;
/// Cap for `wait_for_stdout` so a missing match can't hang export.
const WAIT_FOR_TIMEOUT_MS: u64 = 15_000;
/// Grace period for the shell to exit after the demo's `exit` before we kill it.
/// A demo whose last command leaves a process in the foreground (a server, a
/// REPL — anything that swallows `exit`) would otherwise block teardown forever.
const EXIT_GRACE_MS: u64 = 2_000;
/// Cap on waiting for the capture thread to drain once the shell is gone. A
/// stray foreground process can keep the PTY open, so we never block on it.
const READER_JOIN_MS: u64 = 1_000;
/// Pre-roll: discard startup chatter once output has been quiet this long…
const PREROLL_QUIET_MS: u64 = 400;
/// …but never wait longer than this for it to settle.
const PREROLL_MAX_MS: u64 = 4_000;
/// End of run: consider the demo finished once output is quiet this long (this
/// also becomes how long the final frame is held).
const SETTLE_QUIET_MS: u64 = 1_500;
/// …capped, so a last command that streams forever can't hang the export.
const SETTLE_MAX_MS: u64 = 12_000;

/// A captured terminal recording.
#[derive(Clone)]
pub struct Recording {
    pub cols: u16,
    pub rows: u16,
    pub title: String,
    /// `(seconds_from_start, utf8 chunk)` output events.
    pub events: Vec<(f64, String)>,
    /// `(seconds_from_start, text)` caption changes (empty text clears).
    pub captions: Vec<(f64, String)>,
    /// `(seconds_from_start, pane_id)` of each `focus` — used by the stage to
    /// reveal a browser pane at the moment it's focused.
    pub focuses: Vec<(f64, String)>,
    pub duration: f64,
}

/// Run a single-terminal score (cast/html/gif fast path) — rejects browser panes.
pub fn run_terminal(score: &Score) -> Result<Recording> {
    let pane = single_terminal_pane(score)?;
    run_with_pane(score, pane)
}

/// Run the score's timeline in a PTY sized to `pane`, capturing its output.
/// Browser steps (focus/scroll on browser panes) are no-ops here; the stage
/// drives browser panes separately and composites the result.
pub fn run_with_pane(score: &Score, pane: &crate::model::Pane) -> Result<Recording> {
    // Secrets the demo enters are NOT stored — ask for them up front and keep them
    // only in memory for this run; they're typed at each `Secret` step below.
    let secrets = collect_secrets(score)?;

    let cols = (pane.width / CELL_W).clamp(1, 1000) as u16;
    let rows = (pane.height / CELL_H).clamp(1, 1000) as u16;

    let pair = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| Error::Export(format!("openpty: {e}")))?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let prompt = score
        .demo
        .prompt
        .as_deref()
        .map(|p| p.to_string())
        .unwrap_or_else(|| default_prompt_for(&shell));
    let mut cmd = CommandBuilder::new(&shell);
    let ps_var = if is_zsh(&shell) { "PROMPT" } else { "PS1" };
    cmd.env(ps_var, &prompt);
    cmd.env("PS2", "> ");
    cmd.env("TERM", "xterm-256color");
    let mut child = pair
        .slave
        .spawn_command(cmd)
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

    let (tx, rx) = mpsc::channel::<(Instant, Vec<u8>)>();
    let reader_done = Arc::new(AtomicBool::new(false));
    {
        let reader_done = reader_done.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send((Instant::now(), buf[..n].to_vec())).is_err() {
                    break;
                }
            }
            reader_done.store(true, Ordering::SeqCst);
        });
    }

    // ── Pre-roll (untimed): force a clean prompt + run setup, then discard. ──
    if let Some(setup) = score.env.as_ref().and_then(|e| e.setup_script.as_deref()) {
        let _ = writeln!(writer, "{setup}");
    }
    // Force the prompt after the rc files (which usually set their own PS1), so a
    // demo never leaks `user@host`. Other env (tokens, etc.) is inherited.
    let _ = writeln!(writer, "{ps_var}={}; clear", sh_single_quote(&prompt));
    // A readiness marker makes the start deterministic: wait until the shell
    // echoes it back, which proves all rc/startup chatter and our setup have
    // flushed. A fixed delay (or plain quiet-detection) is racy on slow shells
    // (e.g. WSL) where startup output arrives *after* the silence we'd wait out,
    // leaking `user@host`/`PS1=…` into the demo.
    // The printed marker is assembled by the shell so it differs from the typed
    // command text — otherwise we'd match the PTY's *instant* line-discipline
    // echo of the command (15 ms) instead of the shell actually running it.
    let _ = writeln!(writer, "printf 'demostage_%s_ready\\n' OK");
    wait_for_marker(&rx, "demostage_OK_ready", PREROLL_MAX_MS);
    // Discard the marker's own output and the prompt that follows it.
    drain_until_quiet(&rx, PREROLL_QUIET_MS, PREROLL_MAX_MS);

    // ── Timed run. ───────────────────────────────────────────────────────
    let t0 = Instant::now();
    // The startup prompt was discarded above; emit one fresh prompt so the first
    // command has a clean prompt (`$ `) in front of it.
    let _ = writer.write_all(b"\r");
    let _ = writer.flush();
    let mut events: Vec<(f64, String)> = Vec::new();
    let mut captions: Vec<(f64, String)> = Vec::new();
    let mut focuses: Vec<(f64, String)> = Vec::new();
    let typing = score.typing.clone().unwrap_or_default();
    let mut rng = Rng::new(typing.seed.unwrap_or(DEFAULT_SEED));
    // Capture that fresh prompt so it leads the first command.
    sleep_collecting(120, &mut events, &rx, t0);

    // Index into `events` up to which the VT parser has been fed. Used by
    // wait_for_screen to avoid re-processing already-consumed events.
    let mut vt: Option<VtParser> = None;
    let mut vt_fed: usize = 0;

    let total_steps = score.timeline.len();
    for (step_idx, step) in score.timeline.iter().enumerate() {
        progress_bar("recording", step_idx + 1, total_steps);

        match step {
            Step::Focus { pane, scene } => {
                let target = if let Some(p) = pane {
                    p.clone()
                } else if let Some(s) = scene {
                    s.clone()
                } else {
                    continue;
                };
                focuses.push((t0.elapsed().as_secs_f64(), target));
            }
            Step::Caption { text } => {
                captions.push((t0.elapsed().as_secs_f64(), text.clone()));
            }
            Step::Type { text, human_salt } => {
                let delays = if *human_salt {
                    humanize_delays(text, typing.base_ms, typing.salt_ms, &mut rng)
                } else {
                    vec![0; text.chars().count()]
                };
                let mut b = [0u8; 4];
                for (ch, d) in text.chars().zip(delays) {
                    if d > 0 {
                        thread::sleep(Duration::from_millis(d));
                    }
                    let _ = writer.write_all(ch.encode_utf8(&mut b).as_bytes());
                    let _ = writer.flush();
                    collect(&mut events, &rx, t0);
                }
            }
            Step::Keypress { key } => {
                let _ = writer.write_all(&key_to_bytes(key));
                let _ = writer.flush();
                sleep_collecting(60, &mut events, &rx, t0);
            }
            Step::Wait { duration_ms } => sleep_collecting(*duration_ms, &mut events, &rx, t0),
            Step::WaitForStdout { pattern, .. } => wait_for(pattern, &mut events, &rx, t0),
            Step::WaitForQuiet { quiet_ms, max_ms } => {
                settle(
                    &mut events,
                    &rx,
                    t0,
                    *quiet_ms,
                    max_ms.unwrap_or(WAIT_FOR_TIMEOUT_MS),
                );
            }
            Step::WaitForScreen {
                pattern,
                timeout_ms,
            } => {
                wait_for_screen(
                    pattern,
                    &mut events,
                    &rx,
                    t0,
                    &mut vt,
                    &mut vt_fed,
                    rows,
                    cols,
                    timeout_ms.unwrap_or(WAIT_FOR_TIMEOUT_MS),
                );
            }
            Step::Secret { prompt } => {
                // Supply the secret ONLY once the matching prompt is actually
                // showing, so it can never land in the wrong field (e.g. the repo
                // name). Wait for the prompt label to appear (or confirm it already
                // printed), then type the value collected up front (in memory only).
                let needle = secret_needle(prompt);
                if !needle.is_empty() && !recent_contains(&events, &needle) {
                    wait_for(&needle, &mut events, &rx, t0);
                }
                sleep_collecting(150, &mut events, &rx, t0);
                if let Some(val) = secrets.get(prompt) {
                    let _ = writer.write_all(val.as_bytes());
                }
                let _ = writer.write_all(b"\r");
                let _ = writer.flush();
                sleep_collecting(150, &mut events, &rx, t0);
            }
            Step::Scroll { .. } => {} // browser-only; no-op for terminal capture
            Step::Terminate => break,
        }
    }

    // Clear the progress line.
    progress_clear();

    // ── Settle: hold after the last step until output goes quiet, so the final
    // result (a command's output, an error) finishes rendering and is held on
    // screen — rather than being cut off by a fixed timer. ──────────────────
    let settle_end = settle(&mut events, &rx, t0, SETTLE_QUIET_MS, SETTLE_MAX_MS);

    // ── Teardown + close. ──────────────────────────────────────────────────
    if let Some(td) = score
        .env
        .as_ref()
        .and_then(|e| e.teardown_script.as_deref())
    {
        let _ = writeln!(writer, "{td} >/dev/null 2>&1");
    }
    let _ = writer.write_all(b"\nexit\n");
    let _ = writer.flush();
    drop(writer);

    // Bounded shutdown: give the shell a grace period to exit on its own, then
    // kill it. Without this, a demo whose last command leaves a process in the
    // foreground hangs export indefinitely.
    let deadline = Instant::now() + Duration::from_millis(EXIT_GRACE_MS);
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }

    // Drain trailing output, waiting — bounded — for the capture thread to reach
    // EOF. A stray foreground process can keep the PTY open, so we never join it
    // unconditionally; the thread is detached and reaped when the process exits.
    thread::sleep(Duration::from_millis(50));
    let join_deadline = Instant::now() + Duration::from_millis(READER_JOIN_MS);
    while !reader_done.load(Ordering::SeqCst) && Instant::now() < join_deadline {
        collect(&mut events, &rx, t0);
        thread::sleep(Duration::from_millis(20));
    }
    collect(&mut events, &rx, t0);

    // Drop the post-settle teardown noise (the `exit` echo) and hold the final
    // frame for the settle's quiet period.
    events.retain(|(t, _)| *t <= settle_end);
    let duration = settle_end.max(events.last().map(|(t, _)| *t).unwrap_or(0.0)) + 0.2;
    Ok(Recording {
        cols,
        rows,
        title: score.demo.name.clone(),
        events,
        captions,
        focuses,
        duration,
    })
}

/// Ask for every secret the score enters, up front, keeping the values only in
/// memory for this run (they're typed at each [`Step::Secret`]). Returns a map of
/// prompt label → value. No `Secret` steps → no prompts.
fn collect_secrets(score: &Score) -> Result<std::collections::HashMap<String, String>> {
    let mut prompts: Vec<&str> = Vec::new();
    for step in &score.timeline {
        if let Step::Secret { prompt } = step {
            if !prompts.contains(&prompt.as_str()) {
                prompts.push(prompt);
            }
        }
    }
    let mut out = std::collections::HashMap::new();
    if prompts.is_empty() {
        return Ok(out);
    }
    eprintln!(
        "This demo enters {} secret(s); they're asked now and kept only in memory \
         (never written to disk):",
        prompts.len()
    );
    for p in prompts {
        let val = inquire::Password::new(p)
            .with_display_mode(inquire::PasswordDisplayMode::Masked)
            .without_confirmation()
            .prompt()
            .map_err(|e| Error::Export(format!("secret prompt: {e}")))?;
        out.insert(p.to_string(), val);
    }
    Ok(out)
}

/// A stable substring of a secret's prompt label to wait for in the output (the
/// label minus its trailing punctuation), e.g. `Vault passphrase` for
/// `Vault passphrase:`.
fn secret_needle(prompt: &str) -> String {
    prompt
        .trim()
        .trim_end_matches([':', '?', ' '])
        .trim()
        .to_string()
}

/// Has `needle` shown up in the recent captured output (so a prompt we're waiting
/// for has already printed)?
fn recent_contains(events: &[(f64, String)], needle: &str) -> bool {
    events
        .iter()
        .rev()
        .take(40)
        .any(|(_, d)| d.contains(needle))
}

fn single_terminal_pane(score: &Score) -> Result<&crate::model::Pane> {
    if score
        .layout
        .panes
        .iter()
        .any(|p| p.kind == PaneKind::Browser)
    {
        return Err(Error::Export(
            "browser panes need a browser renderer (chromium) and aren't supported yet — \
             terminal panes only for now"
                .to_string(),
        ));
    }
    let mut terms = score
        .layout
        .panes
        .iter()
        .filter(|p| p.kind == PaneKind::Terminal);
    match (terms.next(), terms.next()) {
        (Some(p), None) => Ok(p),
        (None, _) => Err(Error::Export("no terminal pane to record".to_string())),
        (Some(_), Some(_)) => Err(Error::Export(
            "cast/html support a single terminal pane (found several)".to_string(),
        )),
    }
}

fn collect(events: &mut Vec<(f64, String)>, rx: &Receiver<(Instant, Vec<u8>)>, t0: Instant) {
    while let Ok((ts, bytes)) = rx.try_recv() {
        let secs = ts.saturating_duration_since(t0).as_secs_f64();
        events.push((secs, String::from_utf8_lossy(&bytes).into_owned()));
    }
}

fn sleep_collecting(
    ms: u64,
    events: &mut Vec<(f64, String)>,
    rx: &Receiver<(Instant, Vec<u8>)>,
    t0: Instant,
) {
    let until = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < until {
        collect(events, rx, t0);
        thread::sleep(Duration::from_millis(10));
    }
    collect(events, rx, t0);
}

fn wait_for(
    pattern: &str,
    events: &mut Vec<(f64, String)>,
    rx: &Receiver<(Instant, Vec<u8>)>,
    t0: Instant,
) {
    let deadline = Instant::now() + Duration::from_millis(WAIT_FOR_TIMEOUT_MS);
    let mut seen = String::new();
    while Instant::now() < deadline {
        let before = events.len();
        collect(events, rx, t0);
        for (_, data) in &events[before..] {
            seen.push_str(data);
        }
        if seen.contains(pattern) {
            return;
        }
        thread::sleep(Duration::from_millis(15));
    }
}

/// Block until `pattern` is visible on the parsed VT screen buffer.
/// Unlike `wait_for`, this strips escape codes and checks only rendered text.
#[allow(clippy::too_many_arguments)]
fn wait_for_screen(
    pattern: &str,
    events: &mut Vec<(f64, String)>,
    rx: &Receiver<(Instant, Vec<u8>)>,
    t0: Instant,
    vt: &mut Option<VtParser>,
    vt_fed: &mut usize,
    rows: u16,
    cols: u16,
    timeout_ms: u64,
) {
    let parser = vt.get_or_insert_with(|| {
        let mut p = VtParser::new(rows, cols, 0);
        // Feed all events accumulated so far.
        for (_, data) in events.iter() {
            p.process(data.as_bytes());
        }
        *vt_fed = events.len();
        p
    });

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        collect(events, rx, t0);
        // Feed new events into the VT parser.
        for (_, data) in &events[*vt_fed..] {
            parser.process(data.as_bytes());
        }
        *vt_fed = events.len();
        // Check if pattern is visible on the rendered screen.
        if screen_contains(parser, pattern) {
            return;
        }
        thread::sleep(Duration::from_millis(15));
    }
}

/// Check whether `pattern` appears in the visible text of the VT screen.
fn screen_contains(parser: &VtParser, pattern: &str) -> bool {
    parser.screen().contents().contains(pattern)
}

/// Read and discard output until `marker` appears (or `max_ms` elapses). Used in
/// the pre-roll to wait out slow shell startup deterministically.
fn wait_for_marker(rx: &Receiver<(Instant, Vec<u8>)>, marker: &str, max_ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(max_ms);
    let mut seen = String::new();
    while Instant::now() < deadline {
        let mut got = false;
        while let Ok((_, bytes)) = rx.try_recv() {
            seen.push_str(&String::from_utf8_lossy(&bytes));
            got = true;
        }
        if seen.contains(marker) {
            return;
        }
        if !got {
            thread::sleep(Duration::from_millis(15));
        }
    }
}

/// Discard output until none has arrived for `quiet_ms` (or `max_ms` elapses) —
/// used to wait out (and throw away) slow shell-startup chatter before the timed
/// run, instead of a racy fixed delay.
fn drain_until_quiet(rx: &Receiver<(Instant, Vec<u8>)>, quiet_ms: u64, max_ms: u64) {
    let start = Instant::now();
    let mut last = Instant::now();
    loop {
        let mut got = false;
        while rx.try_recv().is_ok() {
            got = true;
        }
        if got {
            last = Instant::now();
        }
        if last.elapsed() >= Duration::from_millis(quiet_ms)
            || start.elapsed() >= Duration::from_millis(max_ms)
        {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Capture output until none has arrived for `quiet_ms` (or `max_ms` elapses),
/// returning the elapsed time when it went quiet — so the final result finishes
/// rendering and the last frame is held for the quiet period.
fn settle(
    events: &mut Vec<(f64, String)>,
    rx: &Receiver<(Instant, Vec<u8>)>,
    t0: Instant,
    quiet_ms: u64,
    max_ms: u64,
) -> f64 {
    let start = Instant::now();
    let mut last_change = Instant::now();
    loop {
        let before = events.len();
        collect(events, rx, t0);
        if events.len() != before {
            last_change = Instant::now();
        }
        if last_change.elapsed() >= Duration::from_millis(quiet_ms)
            || start.elapsed() >= Duration::from_millis(max_ms)
        {
            return t0.elapsed().as_secs_f64();
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Print a progress bar to stderr: `  label… [████████░░░░░░░░] 42%`
pub fn progress_bar(label: &str, current: usize, total: usize) {
    const WIDTH: usize = 24;
    let pct = if total == 0 {
        100
    } else {
        (current * 100 / total).min(100)
    };
    let filled = (pct * WIDTH) / 100;
    let bar: String = "█".repeat(filled) + &"░".repeat(WIDTH - filled);
    eprint!("\r  {label} [{bar}] {pct:>3}%");
}

/// Clear the progress bar line.
pub fn progress_clear() {
    eprint!("\r{}\r", " ".repeat(60));
}

/// Wrap a string in POSIX single quotes for safe substitution into a shell
/// command (each embedded `'` becomes `'\''`), so an arbitrary configured prompt
/// can't break out of the `PS1=…` assignment.
pub fn sh_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Translate a named key into the bytes a terminal expects.
fn key_to_bytes(key: &str) -> Vec<u8> {
    match key.to_ascii_lowercase().as_str() {
        "enter" | "return" | "\\n" => vec![b'\r'],
        "tab" => vec![b'\t'],
        "space" => vec![b' '],
        "esc" | "escape" => vec![0x1b],
        "backspace" => vec![0x7f],
        "up" => vec![0x1b, b'[', b'A'],
        "down" => vec![0x1b, b'[', b'B'],
        "right" => vec![0x1b, b'[', b'C'],
        "left" => vec![0x1b, b'[', b'D'],
        "ctrl+c" => vec![0x03],
        "ctrl+d" => vec![0x04],
        "ctrl+u" => vec![0x15],
        "ctrl+l" => vec![0x0c],
        other => {
            // ctrl+<letter>
            if let Some(letter) = other.strip_prefix("ctrl+") {
                if let Some(c) = letter.chars().next() {
                    if c.is_ascii_alphabetic() {
                        return vec![(c.to_ascii_lowercase() as u8) - b'a' + 1];
                    }
                }
            }
            // a single character → itself; otherwise Enter as a safe default
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c.to_string().into_bytes(),
                _ => vec![b'\r'],
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_named_keys() {
        assert_eq!(key_to_bytes("enter"), vec![b'\r']);
        assert_eq!(key_to_bytes("ctrl+c"), vec![0x03]);
        assert_eq!(key_to_bytes("ctrl+a"), vec![0x01]);
        assert_eq!(key_to_bytes("tab"), vec![b'\t']);
        assert_eq!(key_to_bytes("a"), b"a".to_vec());
        assert_eq!(key_to_bytes("up"), vec![0x1b, b'[', b'A']);
    }

    #[test]
    fn single_quotes_prompts_safely() {
        // The default prompt (colour escapes and all) is wrapped verbatim.
        assert_eq!(
            sh_single_quote(DEFAULT_PROMPT),
            format!("'{DEFAULT_PROMPT}'")
        );
        assert_eq!(sh_single_quote("$ "), "'$ '");
        // An embedded quote can't break out of the assignment.
        assert_eq!(sh_single_quote("a'b"), "'a'\\''b'");
    }

    /// A demo whose last command leaves a process in the foreground must not hang
    /// teardown: bounded shutdown caps it at the grace period plus the drain, not
    /// the command's lifetime.
    #[cfg(unix)]
    #[test]
    fn replay_does_not_hang_on_a_long_running_command() {
        let score: Score = toml::from_str(
            r#"
[demo]
name = "hang"
[layout]
width = 800
height = 400
fps = 10
  [[layout.panes]]
  id = "c"
  type = "terminal"
  x = 0
  y = 0
  width = 800
  height = 400
[[timeline]]
action = "type"
text = "sleep 120\n"
[[timeline]]
action = "wait"
duration_ms = 200
"#,
        )
        .unwrap();

        let start = Instant::now();
        let rec = run_terminal(&score).expect("replay should return");
        let elapsed = start.elapsed();
        // Well under `sleep 120`: pre-roll + grace (2s) + drain (≤1s) + slack.
        assert!(
            elapsed < Duration::from_secs(10),
            "teardown hung: took {elapsed:?}"
        );
        assert_eq!(rec.cols, 80);
    }
}
