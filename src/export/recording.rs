//! The recording artifact (`.rec`) that `demo record` writes and `demo export`
//! plays back.
//!
//! It is a JSON header line followed by `[t,"o",data]` timestamped output lines;
//! the header carries the demo-stage render config (layout, typing, captions,
//! focuses) under a `demostage` key, so `export` reconstructs everything it needs
//! to render **without re-executing** the demo.
//!
//! `read` also accepts a raw capture (`macro.raw.toml`): it renders the live
//! session's recorded output directly, with a default single-terminal layout.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::run::Recording;
use crate::error::{Error, Result};
use crate::model::{DemoMeta, Layout, Pane, PaneKind, RawEvent, RawMacro, Score, Typing};

/// Assumed monospace cell size (px), matching the recorder's geometry.
const CELL_W: u32 = 10;
const CELL_H: u32 = 20;

/// The extra demo-stage render config stashed in the cast header.
#[derive(Serialize, Deserialize)]
struct DemoStageMeta {
    demo: DemoMeta,
    layout: Layout,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    typing: Option<Typing>,
    #[serde(default)]
    captions: Vec<(f64, String)>,
    #[serde(default)]
    focuses: Vec<(f64, String)>,
}

/// Serialize a recording plus its render config: a JSON header line (with the
/// `demostage` render config) followed by timestamped output lines.
pub fn write(rec: &Recording, score: &Score) -> Result<String> {
    let meta = DemoStageMeta {
        demo: score.demo.clone(),
        layout: score.layout.clone(),
        typing: score.typing.clone(),
        captions: rec.captions.clone(),
        focuses: rec.focuses.clone(),
    };
    let header = json!({
        "version": 2,
        "width": rec.cols,
        "height": rec.rows,
        "title": rec.title,
        "env": { "TERM": "xterm-256color" },
        "demostage": meta,
    });

    let mut out = serde_json::to_string(&header)?;
    out.push('\n');
    for (t, data) in &rec.events {
        out.push_str(&serde_json::to_string(&json!([t, "o", data]))?);
        out.push('\n');
    }
    Ok(out)
}

/// Load a recording for playback. Accepts either a `.cast` (from `demo record`)
/// or a raw capture (`macro.raw.toml`). Returns the recording plus a score that
/// carries the layout/styling to render it (its timeline is empty — playback
/// replays the recorded events, it never executes the timeline).
pub fn read(path: &Path) -> Result<(Recording, Score)> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    if text.trim_start().starts_with('{') {
        read_cast(&text)
    } else {
        read_raw(path, &text)
    }
}

fn read_cast(text: &str) -> Result<(Recording, Score)> {
    let mut lines = text.lines();
    let header: Value = lines
        .next()
        .ok_or_else(|| Error::Export("empty cast".to_string()))
        .and_then(|l| {
            serde_json::from_str(l).map_err(|e| Error::Export(format!("cast header: {e}")))
        })?;

    let cols = header["width"].as_u64().unwrap_or(80) as u16;
    let rows = header["height"].as_u64().unwrap_or(24) as u16;
    let title = header["title"].as_str().unwrap_or("demo").to_string();

    let mut events = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let ev: Value =
            serde_json::from_str(line).map_err(|e| Error::Export(format!("cast event: {e}")))?;
        if let (Some(t), Some("o"), Some(data)) = (
            ev.get(0).and_then(Value::as_f64),
            ev.get(1).and_then(Value::as_str),
            ev.get(2).and_then(Value::as_str),
        ) {
            events.push((t, data.to_string()));
        }
    }
    let duration = events.last().map(|(t, _)| *t).unwrap_or(0.0);

    // Recover the demo-stage render config if this cast came from `demo record`;
    // otherwise fall back to a default single-terminal layout.
    let (score, captions, focuses) = match header.get("demostage") {
        Some(v) => {
            let meta: DemoStageMeta = serde_json::from_value(v.clone())
                .map_err(|e| Error::Export(format!("cast demostage header: {e}")))?;
            let score = Score {
                demo: meta.demo,
                env: None,
                typing: meta.typing,
                layout: meta.layout,
                timeline: Vec::new(),
            };
            (score, meta.captions, meta.focuses)
        }
        None => (default_score("demo", cols, rows), Vec::new(), Vec::new()),
    };

    Ok((
        Recording {
            cols,
            rows,
            title,
            events,
            captions,
            focuses,
            duration,
        },
        score,
    ))
}

fn read_raw(path: &Path, text: &str) -> Result<(Recording, Score)> {
    let raw: RawMacro =
        toml::from_str(text).map_err(|e| Error::Export(format!("{}: {e}", path.display())))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.strip_suffix(".raw").unwrap_or(s))
        .unwrap_or("demo");
    let (cols, rows) = (raw.meta.cols, raw.meta.rows);
    Ok((from_raw(&raw, name), default_score(name, cols, rows)))
}

/// Longest pause kept between consecutive output chunks when normalizing a
/// faithful recording — longer waits (thinking, network) are trimmed to this, so
/// playback feels deliberate without dead air.
const MAX_GAP_MS: f64 = 1200.0;
/// How long the final frame is held after the last output, so the result is
/// readable before the demo ends / loops (instead of cutting off abruptly).
const TAIL_HOLD_MS: f64 = 1800.0;

/// Build a playback recording from a raw capture's real output stream, with its
/// **timing normalized**: the trailing `demo stop` you typed is dropped, and idle
/// gaps are trimmed. Input events are dropped — the output already echoes them.
///
/// Note: typo-pruning and re-humanized typing can't be applied to a faithful
/// recording (the keystrokes are already baked into the output) — those need
/// re-execution via `demo record`. Timing is what we can clean here.
pub fn from_raw(raw: &RawMacro, name: &str) -> Recording {
    // Drop everything from the moment the user started typing the stop command.
    let cutoff = stop_cutoff_ms(raw);
    let raw_events: Vec<(f64, String)> = raw
        .events
        .iter()
        .filter_map(|e| match e {
            RawEvent::Output { t_ms, data } if cutoff.is_none_or(|c| *t_ms < c) => {
                Some((*t_ms as f64 / 1000.0, data.clone()))
            }
            _ => None,
        })
        .collect();

    // Trim idle: cap the gap before each chunk (also trims leading dead air).
    let cap = MAX_GAP_MS / 1000.0;
    let mut events = Vec::with_capacity(raw_events.len());
    let mut acc = 0.0;
    let mut prev = 0.0;
    for (i, (t, data)) in raw_events.iter().enumerate() {
        let gap = if i == 0 { *t } else { t - prev };
        acc += gap.clamp(0.0, cap);
        prev = *t;
        events.push((acc, data.clone()));
    }
    // Hold the final frame a moment so the last result is readable.
    let duration = events
        .last()
        .map(|(t, _)| *t + TAIL_HOLD_MS / 1000.0)
        .unwrap_or(0.0);

    Recording {
        cols: raw.meta.cols,
        rows: raw.meta.rows,
        title: name.to_string(),
        events,
        captions: Vec::new(),
        focuses: Vec::new(),
        duration,
    }
}

/// The `t_ms` at which the user started typing the final `demo stop` line, if the
/// capture ended that way — so its echo (and the "stopping" message) is dropped.
fn stop_cutoff_ms(raw: &RawMacro) -> Option<u64> {
    let mut line = String::new();
    let mut line_start: Option<u64> = None;
    let mut cutoff: Option<u64> = None;
    for e in &raw.events {
        if let RawEvent::Input { t_ms, bytes } = e {
            for ch in bytes.chars() {
                match ch {
                    '\r' | '\n' => {
                        if line.trim() == crate::STOP_COMMAND {
                            cutoff = line_start;
                        }
                        line.clear();
                        line_start = None;
                    }
                    c if c.is_control() => {}
                    c => {
                        line_start.get_or_insert(*t_ms);
                        line.push(c);
                    }
                }
            }
        }
    }
    if line.trim() == crate::STOP_COMMAND {
        cutoff = cutoff.or(line_start);
    }
    cutoff
}

/// A plain single-terminal score sized to a `cols`×`rows` capture, used to render
/// a recording that carries no layout of its own.
pub fn default_score(name: &str, cols: u16, rows: u16) -> Score {
    let width = cols as u32 * CELL_W;
    let height = rows as u32 * CELL_H;
    Score {
        demo: DemoMeta {
            name: name.to_string(),
            output_dir: "./dist".into(),
            prompt: None,
        },
        env: None,
        typing: None,
        layout: Layout {
            width,
            height,
            fps: 15,
            line_height: 1.0,
            background: None,
            panes: vec![Pane {
                id: "main".to_string(),
                kind: PaneKind::Terminal,
                x: 0,
                y: 0,
                width,
                height,
                font_family: None,
                font_size: None,
                url: None,
            }],
        },
        timeline: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Recording, Score) {
        let rec = Recording {
            cols: 80,
            rows: 24,
            title: "t".into(),
            events: vec![(0.1, "hi".into()), (0.5, "\r\n".into())],
            captions: vec![(0.2, "step one".into())],
            focuses: vec![],
            duration: 0.5,
        };
        (rec, default_score("t", 80, 24))
    }

    #[test]
    fn cast_round_trips_through_write_and_read() {
        let (rec, score) = sample();
        let cast = write(&rec, &score).unwrap();
        // First line is a valid JSON header carrying our config.
        assert!(cast.lines().next().unwrap().contains("\"demostage\""));

        let tmp = std::env::temp_dir().join(format!("rec-{}.cast", std::process::id()));
        std::fs::write(&tmp, &cast).unwrap();
        let (back, back_score) = read(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();

        assert_eq!(back.events, rec.events);
        assert_eq!(back.captions, rec.captions);
        assert_eq!(back.cols, 80);
        assert_eq!(back_score.layout.panes.len(), 1);
    }

    #[test]
    fn reads_a_raw_capture_as_a_recording() {
        let raw = r#"
[meta]
shell = "/bin/bash"
cols = 90
rows = 30

[[events]]
kind = "input"
t_ms = 0
bytes = "ls\r"

[[events]]
kind = "output"
t_ms = 250
data = "file.txt\n"
"#;
        let tmp = std::env::temp_dir().join(format!("cap-{}.raw.toml", std::process::id()));
        std::fs::write(&tmp, raw).unwrap();
        let (rec, score) = read(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();

        // Only output events become the playback stream; input is ignored.
        assert_eq!(rec.events, vec![(0.25, "file.txt\n".to_string())]);
        assert_eq!(rec.cols, 90);
        assert_eq!(score.layout.width, 90 * CELL_W);
    }

    fn raw(events: Vec<RawEvent>) -> RawMacro {
        RawMacro {
            meta: crate::model::RawMeta {
                shell: "/bin/bash".into(),
                cols: 80,
                rows: 24,
                idle_timeout_ms: 0,
                stage: None,
            },
            events,
        }
    }

    #[test]
    fn from_raw_caps_idle_gaps() {
        // A 5s dead-air gap between two chunks is trimmed to the 1.2s cap.
        let r = raw(vec![
            RawEvent::Output {
                t_ms: 0,
                data: "a".into(),
            },
            RawEvent::Output {
                t_ms: 5000,
                data: "b".into(),
            },
        ]);
        let rec = from_raw(&r, "t");
        assert_eq!(rec.events[0].0, 0.0);
        assert!((rec.events[1].0 - 1.2).abs() < 1e-9, "{}", rec.events[1].0);
    }

    #[test]
    fn from_raw_drops_the_trailing_demo_stop() {
        // The `demo stop` the user typed (and its echo/stopping message) is gone.
        let r = raw(vec![
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
            RawEvent::Output {
                t_ms: 2100,
                data: "● stopping recording…".into(),
            },
        ]);
        let rec = from_raw(&r, "t");
        assert_eq!(rec.events.len(), 1);
        assert_eq!(rec.events[0].1, "real output");
    }
}
