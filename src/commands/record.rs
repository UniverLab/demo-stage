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

use crate::cli::RecordArgs;
use crate::error::{Error, Result};
use crate::model::{RawEvent, RawMacro, RawMeta};

fn ms(t0: Instant) -> u64 {
    t0.elapsed().as_millis() as u64
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
    let mut child = pair
        .slave
        .spawn_command(CommandBuilder::new(&shell))
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
    let t0 = Instant::now();

    enable_raw_mode().map_err(|e| Error::Export(format!("raw mode: {e}")))?;

    // PTY → stdout, recorded as output events.
    let out_handle = {
        let events = events.clone();
        let last = last_activity.clone();
        let exited = shell_exited.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut stdout = std::io::stdout();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let _ = stdout.write_all(&buf[..n]);
                        let _ = stdout.flush();
                        *last.lock().unwrap() = Instant::now();
                        events.lock().unwrap().push(RawEvent::Output {
                            t_ms: ms(t0),
                            data: String::from_utf8_lossy(&buf[..n]).into_owned(),
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
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let mut stdin = std::io::stdin();
            while !stop.load(Ordering::SeqCst) {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if writer.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        let _ = writer.flush();
                        *last.lock().unwrap() = Instant::now();
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

    let events = events.lock().unwrap().clone();
    let raw = RawMacro {
        meta: RawMeta {
            shell,
            cols,
            rows,
            idle_timeout_ms: idle,
        },
        events,
    };
    raw.save(&args.output)?;

    println!(
        "recorded {} events → {} (next: demo normalize {})",
        raw.events.len(),
        args.output.display(),
        args.output.display()
    );
    Ok(())
}
