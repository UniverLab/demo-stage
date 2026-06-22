//! `demo record` — capture an interactive session into a raw macro.
//!
//! Spawns a shell in a PTY and bridges it to the real terminal (raw mode):
//! local stdin → PTY (recorded as `input` events), PTY → local stdout
//! (recorded as `output` events), each timestamped. Recording ends when the
//! shell exits or after `idle_timeout_ms` with no terminal output (SPEC §3.3).

use std::io::{IsTerminal, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::cli::{NormalizeArgs, RecordArgs};
use crate::commands::stop::STOP_FILE_ENV;
use crate::error::{Error, Result};
use crate::model::{RawEvent, RawMacro, RawMeta};

fn ms(t0: Instant) -> u64 {
    t0.elapsed().as_millis() as u64
}

/// Track the current terminal line from an output chunk (cleared on newline).
fn track_line(line: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '\n' | '\r' => line.clear(),
            c if c.is_control() => {}
            c => line.push(c),
        }
    }
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
    const HINTS: [&str; 6] = [
        "password",
        "passphrase",
        "passcode",
        "secret",
        "[sudo]",
        "verification code",
    ];
    HINTS.iter().any(|h| trimmed.contains(h))
}

pub fn run(args: RecordArgs) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::Export(
            "record needs an interactive terminal (stdin/stdout is not a TTY)".to_string(),
        ));
    }

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
    // Sentinel the captured shell can touch (via `demo stop`) to end the
    // capture without typing `exit` mid-demo. Unique per recorder process.
    let stopfile =
        std::env::temp_dir().join(format!("demo-stage-record-{}.stop", std::process::id()));
    let _ = std::fs::remove_file(&stopfile);
    let mut command = CommandBuilder::new(&shell);
    command.env(STOP_FILE_ENV, &stopfile);

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
    let t0 = Instant::now();

    // Tell the user how to end the capture before the shell takes over — the
    // only cues otherwise are typing `exit` or Ctrl-D, neither of which is
    // obvious mid-demo.
    println!("● recording — run your demo, then type `demo stop` (or `exit` / Ctrl-D) to stop");
    if args.idle_timeout_ms > 0 {
        println!(
            "  (auto-stops after {} ms with no output)",
            args.idle_timeout_ms
        );
    }
    println!();

    enable_raw_mode().map_err(|e| Error::Export(format!("raw mode: {e}")))?;

    // PTY → stdout, recorded as output events.
    let out_handle = {
        let events = events.clone();
        let last = last_activity.clone();
        let exited = shell_exited.clone();
        let sensitive = sensitive.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut stdout = std::io::stdout();
            let mut line = String::new();
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
                        let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                        track_line(&mut line, &text);
                        sensitive.store(is_secret_prompt(&line), Ordering::SeqCst);
                        events.lock().unwrap().push(RawEvent::Output {
                            t_ms: ms(t0),
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
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let mut stdin = std::io::stdin();
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
                        // Secret prompt active → forward the keystrokes but do NOT
                        // record them; clear once the prompt is answered (Enter).
                        if sensitive.load(Ordering::SeqCst) {
                            if buf[..n].iter().any(|b| *b == b'\r' || *b == b'\n') {
                                sensitive.store(false, Ordering::SeqCst);
                            }
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

    // Watchdog: stop on shell exit or idle timeout.
    let idle = args.idle_timeout_ms;
    loop {
        if shell_exited.load(Ordering::SeqCst) {
            break;
        }
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        // `demo stop`, run inside the capture, touches this file.
        if stopfile.exists() {
            break;
        }
        if idle > 0 && last_activity.lock().unwrap().elapsed() > Duration::from_millis(idle) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    stop.store(true, Ordering::SeqCst);
    let _ = child.kill();
    let _ = child.wait();
    let _ = out_handle.join();
    let _ = disable_raw_mode();
    let _ = std::fs::remove_file(&stopfile);

    let events = events.lock().unwrap().clone();
    let raw = RawMacro {
        meta: RawMeta {
            shell,
            cols,
            rows,
            idle_timeout_ms: idle,
            stage: args.into.as_ref().map(|p| p.display().to_string()),
        },
        events,
    };
    raw.save(&args.output)?;
    println!(
        "recorded {} events → {}",
        raw.events.len(),
        args.output.display()
    );

    // Normalize is part of finishing a recording, not a separate chore — run it
    // automatically into a clean score unless the user opted out.
    if args.no_normalize {
        println!("next: demo normalize {}", args.output.display());
        return Ok(());
    }

    crate::commands::normalize::run(NormalizeArgs {
        input: args.output.clone(),
        output: args.normalized_output.clone(),
        seed: None,
        typing_ms: 80,
        salt_ms: 15,
        stage: None,
    })?;
    println!(
        "next: demo export <cast|html|gif|mp4> {}",
        args.normalized_output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_secret_prompt;

    #[test]
    fn flags_secret_prompts_only() {
        assert!(is_secret_prompt(
            "Enter passphrase for key '/home/u/.ssh/id_ed25519':"
        ));
        assert!(is_secret_prompt("Password: "));
        assert!(is_secret_prompt("[sudo] password for jheison:"));
        assert!(is_secret_prompt("Vault passphrase?"));
        // A typed command that mentions the word is NOT a prompt.
        assert!(!is_secret_prompt("$ echo my secret plan"));
        assert!(!is_secret_prompt("Cloning into 'repo'..."));
    }
}
