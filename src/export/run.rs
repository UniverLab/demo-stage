//! Executes a terminal-only score in a real PTY and captures the output as a
//! timestamped recording — the basis for the cast/html/gif targets.
//!
//! The shell echoes typed characters, so pacing the writes (humanized typing)
//! produces a natural char-by-char appearance in the capture. A clean `PS1` is
//! forced before the clock starts, so demos never leak `user@host`.

use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::error::{Error, Result};
use crate::model::{PaneKind, Score, Step};
use crate::normalize::salt::humanize_delays;
use crate::normalize::Rng;

/// Assumed monospace cell size (px), inverse of the normalizer's sizing.
const CELL_W: u32 = 10;
const CELL_H: u32 = 20;
/// Default seed when the score pins none.
const DEFAULT_SEED: u64 = 0xD370_5EED;
/// Cap for `wait_for_stdout` so a missing match can't hang export.
const WAIT_FOR_TIMEOUT_MS: u64 = 15_000;

/// A captured terminal recording.
pub struct Recording {
    pub cols: u16,
    pub rows: u16,
    pub title: String,
    /// `(seconds_from_start, utf8 chunk)` output events.
    pub events: Vec<(f64, String)>,
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
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("PS1", "$ ");
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
    let reader_handle = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send((Instant::now(), buf[..n].to_vec())).is_err() {
                break;
            }
        }
    });

    // ── Pre-roll (untimed): force a clean prompt + run setup, then discard. ──
    if let Some(setup) = score.env.as_ref().and_then(|e| e.setup_script.as_deref()) {
        let _ = writeln!(writer, "{setup}");
    }
    let _ = writeln!(writer, "PS1='$ '; clear");
    thread::sleep(Duration::from_millis(400));
    drain(&rx);

    // ── Timed run. ───────────────────────────────────────────────────────
    let t0 = Instant::now();
    let mut events: Vec<(f64, String)> = Vec::new();
    let typing = score.typing.clone().unwrap_or_default();
    let mut rng = Rng::new(typing.seed.unwrap_or(DEFAULT_SEED));

    for step in &score.timeline {
        match step {
            Step::Focus { .. } => {}
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
            Step::Scroll { .. } => {} // browser-only; no-op for terminal capture
            Step::Terminate => break,
        }
    }

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
    let _ = child.wait();
    thread::sleep(Duration::from_millis(50));
    collect(&mut events, &rx, t0);
    let _ = reader_handle.join();

    let duration = events.last().map(|(t, _)| t + 0.5).unwrap_or(0.5);
    Ok(Recording {
        cols,
        rows,
        title: score.demo.name.clone(),
        events,
        duration,
    })
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

fn drain(rx: &Receiver<(Instant, Vec<u8>)>) {
    while rx.try_recv().is_ok() {}
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
}
