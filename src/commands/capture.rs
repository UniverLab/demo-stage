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
use crate::export::recording;
use crate::export::run::{is_zsh, sh_single_quote};
use crate::model::{DemoMeta, RawEvent, RawMacro, RawMeta, Score};
use crate::normalize::{merge_into_stage, normalize, Options};

/// Marker the captured shell echoes once it's at our forced prompt — recording
/// starts after it, so the prompt-setup chatter is discarded. Assembled by the
/// shell so the typed command doesn't itself match (only the printed output).
const PROMPT_READY: &str = "demostage_capture_ready";

/// A `demo open` reveal: where, how, how long/whether to scroll, and theme.
#[derive(Clone)]
struct Reveal {
    url: String,
    name: Option<String>,
    mode: String,
    hold_ms: Option<u64>,
    scroll: bool,
    theme: Option<String>,
}

/// Reveals armed by `demo open --when <pat>`, each with its cue pattern.
type PendingWhen = Arc<Mutex<Vec<(Reveal, String)>>>;
/// Reveals armed by `demo open --after`, fired when the running command finishes.
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

/// Track the current terminal line and detect a secret prompt at each line
/// boundary (`\r` redraw or `\n`) — crucially BEFORE the boundary clears the line.
/// `inquire` emits the prompt immediately followed by `\r` (`Vault passphrase:\r…`),
/// so checking only at the chunk end (after the `\r` cleared it) missed it and the
/// secret leaked. Latches `sensitive` and records the prompt label when found.
fn track_and_detect(
    line: &mut String,
    text: &str,
    sensitive: &AtomicBool,
    secret_prompt: &Mutex<Option<String>>,
) {
    let check = |line: &str| {
        if is_secret_prompt(line) {
            sensitive.store(true, Ordering::SeqCst);
            *secret_prompt.lock().unwrap() = Some(clean_prompt(line));
        }
    };
    for ch in text.chars() {
        match ch {
            '\n' | '\r' => {
                check(line);
                line.clear();
            }
            c if c.is_control() => {}
            c => line.push(c),
        }
    }
    // The prompt may sit at the chunk end with no trailing newline yet.
    check(line);
}

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

/// Heuristic: does this line look like a program prompting for a secret? Matches
/// a secret keyword on a line that ends like a prompt (`:` or `?`), so a typed
/// command that merely mentions "password" is not mistaken for a prompt.
fn is_secret_prompt(line: &str) -> bool {
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

/// Ask the user for the target export resolution.
fn choose_resolution() -> Result<Option<(u32, u32)>> {
    let choice = inquire::Select::new(
        "Export resolution:",
        vec![
            "Landscape   1920×1080",
            "Portrait    1080×1920",
            "Square      1080×1080",
            "Standard    1024×768",
            "Compact     1280×720",
            "Custom",
            "Auto (derive from terminal size)",
        ],
    )
    .prompt()
    .map_err(|e| Error::Export(format!("resolution wizard: {e}")))?;

    let res = if choice.starts_with("Landscape") {
        Some((1920, 1080))
    } else if choice.starts_with("Portrait") {
        Some((1080, 1920))
    } else if choice.starts_with("Square") {
        Some((1080, 1080))
    } else if choice.starts_with("Standard") {
        Some((1024, 768))
    } else if choice.starts_with("Compact") {
        Some((1280, 720))
    } else if choice.starts_with("Custom") {
        let w = inquire::Text::new("width:")
            .with_default("1920")
            .prompt()
            .map_err(|e| Error::Export(format!("resolution wizard: {e}")))?;
        let h = inquire::Text::new("height:")
            .with_default("1080")
            .prompt()
            .map_err(|e| Error::Export(format!("resolution wizard: {e}")))?;
        let w: u32 = w.trim().parse().unwrap_or(1920);
        let h: u32 = h.trim().parse().unwrap_or(1080);
        Some((w, h))
    } else {
        None
    };
    Ok(res)
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
    let (cols, rows) = size().unwrap_or((80, 24));

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
    let mut command = CommandBuilder::new(&shell);
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
    // Browser reveals armed by `demo open --when <pat>`, fired by the output
    // thread when the pattern appears.
    let pending_opens: PendingWhen = Arc::new(Mutex::new(Vec::new()));
    // Reveals armed by `demo open --after`: fired once the next foreground command
    // produces output and then goes quiet (back at the prompt). `after_running`
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
    let resolution = choose_resolution()?;
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

    // Tell the user how to end the capture before the shell takes over — the
    // only cues otherwise are typing `exit` or Ctrl-D, neither of which is
    // obvious mid-demo.
    println!("● capturing — run your demo, then type `demo stop` (or `exit` / Ctrl-D) to stop");
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
                        let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                        // Detect the secret prompt at each line boundary and LATCH
                        // the flag (only set here; the input thread clears it on
                        // Enter), so the masked redraws can't unset it mid-secret.
                        track_and_detect(&mut line, &text, &sensitive, &secret_prompt);
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
                        // Fire any `demo open --when <pat>` whose cue just appeared.
                        {
                            let mut pend = pending_opens.lock().unwrap();
                            if !pend.is_empty() {
                                let mut evs = events.lock().unwrap();
                                pend.retain(|(r, pat)| {
                                    if recent.contains(pat.as_str()) {
                                        evs.push(RawEvent::Open {
                                            t_ms: now,
                                            url: r.url.clone(),
                                            name: r.name.clone(),
                                            mode: r.mode.clone(),
                                            hold_ms: r.hold_ms,
                                            scroll: r.scroll,
                                            theme: r.theme.clone(),
                                        });
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
        let debug = debug.clone();
        let muting = muting.clone();
        let mute_since = mute_since.clone();
        let mute_start = mute_start.clone();
        let ready = ready.clone();
        let after_opens = after_opens.clone();
        let after_running = after_running.clone();
        let after_last_out = after_last_out.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let mut stdin = std::io::stdin();
            let mut cmd_line = String::new();
            // When the current input line started (first char), so a `demo open`/
            // `demo stop` span can be excised from the command's echo, not just
            // from the Enter that follows it.
            let mut cmd_start: Option<u64> = None;
            while !stop.load(Ordering::SeqCst) {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                    Ok(n) => {
                        if writer.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        let _ = writer.flush();
                        *last.lock().unwrap() = Instant::now();
                        // Don't record input during the prompt-setup pre-roll.
                        if !ready.load(Ordering::SeqCst) {
                            continue;
                        }
                        // Watch typed lines for a `demo open`/`demo stop` command and
                        // start muting its output (cleared when the recorder gets it).
                        for ch in String::from_utf8_lossy(&buf[..n]).chars() {
                            if ch == '\r' || ch == '\n' {
                                let t = cmd_line.trim_start();
                                if t.starts_with("demo open") || t.starts_with("demo stop") {
                                    *mute_since.lock().unwrap() = Instant::now();
                                    muting.store(true, Ordering::SeqCst);
                                    // Start the excision span at the START of the
                                    // typed line, so the command's own echo (and the
                                    // wizard that follows) is removed — not just the
                                    // bytes after the Enter. Covers `demo open` AND
                                    // `demo stop` (closed by the control msg / at end).
                                    let mut s = mute_start.lock().unwrap();
                                    if s.is_none() {
                                        *s = Some(cmd_start.unwrap_or_else(|| ms(t0)));
                                    }
                                } else if !t.is_empty()
                                    && !after_opens.lock().unwrap().is_empty()
                                    && !after_running.load(Ordering::SeqCst)
                                {
                                    // A real command launched while an `--after`
                                    // reveal is armed → watch for it to finish.
                                    *after_last_out.lock().unwrap() = Instant::now();
                                    after_running.store(true, Ordering::SeqCst);
                                }
                                cmd_line.clear();
                                cmd_start = None;
                            } else if !ch.is_control() {
                                if cmd_line.is_empty() {
                                    cmd_start = Some(ms(t0));
                                }
                                cmd_line.push(ch);
                            }
                        }
                        // Secret prompt active → forward the keystrokes but do NOT
                        // record them; clear once the prompt is answered (Enter).
                        let masked = sensitive.load(Ordering::SeqCst);
                        if let Some(d) = &debug {
                            // Never write the secret to the debug log: at a secret
                            // prompt, log only the byte count, not the keystrokes.
                            if masked {
                                d.note(&format!("IN* {n} bytes (redacted — secret prompt)"));
                            } else {
                                d.chunk("IN ", &buf[..n]);
                            }
                        }
                        if masked {
                            if buf[..n].iter().any(|b| *b == b'\r' || *b == b'\n') {
                                sensitive.store(false, Ordering::SeqCst);
                                // Record THAT a secret was entered (its prompt label
                                // only, never the value) so `demo record` can ask for
                                // it again and re-supply it.
                                let prompt =
                                    secret_prompt.lock().unwrap().take().unwrap_or_default();
                                events.lock().unwrap().push(RawEvent::Secret {
                                    t_ms: ms(t0),
                                    prompt,
                                });
                            }
                            continue;
                        }
                        // Don't record input while a meta-command (demo open/stop)
                        // is running — its wizard answers must not enter the demo.
                        if muting.load(Ordering::SeqCst) {
                            continue;
                        }
                        events.lock().unwrap().push(RawEvent::Input {
                            t_ms: ms(t0),
                            bytes: String::from_utf8_lossy(&buf[..n]).into_owned(),
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
                    d.note(&format!("open (after): {} ({})", r.url, r.mode));
                }
                evs.push(RawEvent::Open {
                    t_ms: now,
                    url: r.url,
                    name: r.name,
                    mode: r.mode,
                    hold_ms: r.hold_ms,
                    scroll: r.scroll,
                    theme: r.theme,
                });
            }
            after_running.store(false, Ordering::SeqCst);
        }
        // Safety: an abandoned `demo open` (wizard cancelled, command never sent)
        // shouldn't mute the rest of the demo forever.
        if muting.load(Ordering::SeqCst)
            && mute_since.lock().unwrap().elapsed() > Duration::from_secs(90)
        {
            muting.store(false, Ordering::SeqCst);
            // Close a stranded open span so it doesn't swallow the rest of the demo.
            if let Some(start) = mute_start.lock().unwrap().take() {
                mute_spans.lock().unwrap().push((start, ms(t0)));
            }
        }
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
    if let Some(d) = &debug {
        d.note(&format!("stopping — reason: {reason}"));
    }

    stop.store(true, Ordering::SeqCst);
    let _ = child.kill();
    let _ = child.wait();
    let _ = out_handle.join();
    let _ = disable_raw_mode();
    let _ = std::fs::remove_file(&control_abs);

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
            RawEvent::Open { .. } | RawEvent::Secret { .. } => (i, o),
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
        None => normalize(&raw, name, &opts),
    };
    // Persist the prompt the demo was captured with, so `demo record` reproduces
    // it instead of falling back to the built-in default.
    if force_prompt {
        score.demo.prompt = Some(forced_ps1.clone());
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

/// Read any new control-file commands (`demo open` / `demo stop`). Records
/// immediate reveals, arms `--when` reveals, and returns `Some(reason)` on stop.
#[allow(clippy::too_many_arguments)]
fn read_control(
    path: &std::path::Path,
    read: &mut u64,
    events: &Arc<Mutex<Vec<RawEvent>>>,
    pending: &PendingWhen,
    after: &PendingAfter,
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
            // A `demo open` is starting in the captured shell → mute its wizard.
            // If input detection didn't already mark the start, fall back to a
            // little before now so the wizard's first output is still excised.
            Some("open_begin") => {
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
            Some("open") => {
                // The command finished → stop muting and close its excision span.
                muting.store(false, Ordering::SeqCst);
                if let Some(start) = mute_start.lock().unwrap().take() {
                    mute_spans.lock().unwrap().push((start, ms(t0)));
                }
                let url = v.get("url").and_then(|u| u.as_str()).unwrap_or("");
                if url.is_empty() {
                    continue;
                }
                let mode = v.get("mode").and_then(|m| m.as_str()).unwrap_or("replace");
                let hold_ms = v.get("hold").and_then(|h| h.as_u64());
                let scroll = v.get("scroll").and_then(|s| s.as_bool()).unwrap_or(false);
                let theme = v
                    .get("theme")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
                let name = v
                    .get("name")
                    .and_then(|n| n.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                let reveal = Reveal {
                    url: url.into(),
                    name,
                    mode: mode.into(),
                    hold_ms,
                    scroll,
                    theme,
                };
                let when = v
                    .get("when")
                    .and_then(|w| w.as_str())
                    .filter(|s| !s.is_empty());
                let after_flag = v.get("after").and_then(|a| a.as_bool()).unwrap_or(false);
                if let Some(pat) = when {
                    if let Some(d) = debug {
                        d.note(&format!("open armed: {url} ({mode}) when {pat:?}"));
                    }
                    pending.lock().unwrap().push((reveal, pat.into()));
                } else if after_flag {
                    if let Some(d) = debug {
                        d.note(&format!("open armed: {url} ({mode}) after command"));
                    }
                    after.lock().unwrap().push(reveal);
                } else {
                    if let Some(d) = debug {
                        d.note(&format!("open now: {url} ({mode})"));
                    }
                    events.lock().unwrap().push(RawEvent::Open {
                        t_ms: ms(t0),
                        url: reveal.url,
                        name: reveal.name,
                        mode: reveal.mode,
                        hold_ms: reveal.hold_ms,
                        scroll: reveal.scroll,
                        theme: reveal.theme,
                    });
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
    let (rec, layout, timeline) = recording::from_raw(raw, name);
    let final_score = Score {
        demo: score.map(|s| s.demo.clone()).unwrap_or_else(|| DemoMeta {
            name: name.to_string(),
            output_dir: "./dist".into(),
            prompt: None,
        }),
        env: None,
        typing: score.and_then(|s| s.typing.clone()),
        sources: vec![],
        scenes: vec![],
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
    fn detects_secret_at_a_carriage_return_boundary() {
        // inquire emits the prompt immediately followed by `\r` and cursor codes —
        // detection must fire on the prompt BEFORE the `\r` clears the line.
        let sensitive = AtomicBool::new(false);
        let secret_prompt = Mutex::new(None);
        let mut line = String::new();
        // `\x1b` (ESC) is a control char; `[?25h` is the residue after it.
        track_and_detect(
            &mut line,
            " Vault passphrase:\r\x1b[?25h",
            &sensitive,
            &secret_prompt,
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
        // Tokens / API keys are secrets too.
        assert!(is_secret_prompt("GitHub token:"));
        assert!(is_secret_prompt("Personal access token: "));
        assert!(is_secret_prompt("API key:"));
        // A typed command that mentions the word is NOT a prompt.
        assert!(!is_secret_prompt("$ echo my secret plan"));
        assert!(!is_secret_prompt("Cloning into 'repo'..."));
        assert!(!is_secret_prompt("Refreshing access token cache"));
        // A masked prompt redraws as it's typed (`Vault passphrase: ***`) — those
        // redraws no longer end in `:`/`?`, so they are NOT detected. This is why
        // the recorder LATCHES the secret flag once set and only clears it on Enter
        // (otherwise the rest of the secret would be recorded).
        assert!(!is_secret_prompt("Vault passphrase: ***"));
    }
}
