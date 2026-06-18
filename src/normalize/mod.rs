//! Smart normalizer: turns a raw capture into a clean [`Score`].
//!
//! - §3.1 backspace pruning → [`edit::reconstruct`]
//! - §3.2 humanized typing → [`salt::humanize_delays`] (applied at export)
//! - §3.3 idle trimming → bounded settle waits + trimmed tail, here

mod edit;
mod rng;
pub mod salt;

pub use rng::Rng;

use crate::model::{DemoMeta, Layout, Pane, PaneKind, Score, Step, Typing};
use crate::model::{RawEvent, RawMacro};

/// Knobs for normalization (humanized-typing params are stored in the score).
#[derive(Debug, Clone)]
pub struct Options {
    pub typing_ms: u64,
    pub salt_ms: u64,
    pub seed: Option<u64>,
}

/// Shortest pause shown between two commands.
const MIN_SETTLE_MS: u64 = 300;
/// Longest pause shown between two commands (caps human "think time").
const MAX_SETTLE_MS: u64 = 2500;
/// Longest tail held after the final command (the idle that stopped the
/// recording is trimmed away entirely).
const MAX_TAIL_MS: u64 = 1500;

/// Assumed monospace cell size (px) used to size the default canvas.
const CELL_W: u32 = 10;
const CELL_H: u32 = 20;

/// Normalize a raw capture into a clean score named `name`.
pub fn normalize(raw: &RawMacro, name: &str, opts: &Options) -> Score {
    let inputs: Vec<(u64, &str)> = raw
        .events
        .iter()
        .filter_map(|e| match e {
            RawEvent::Input { t_ms, bytes } => Some((*t_ms, bytes.as_str())),
            RawEvent::Output { .. } => None,
        })
        .collect();

    let commands = edit::reconstruct(&inputs);

    let last_output_ms = raw
        .events
        .iter()
        .filter_map(|e| match e {
            RawEvent::Output { t_ms, .. } => Some(*t_ms),
            RawEvent::Input { .. } => None,
        })
        .max()
        .unwrap_or(0);

    let mut timeline = Vec::with_capacity(commands.len() * 3 + 2);
    timeline.push(Step::Focus {
        pane: "main".to_string(),
    });

    for (i, cmd) in commands.iter().enumerate() {
        timeline.push(Step::Type {
            text: cmd.text.clone(),
            human_salt: true,
        });
        if cmd.had_enter {
            timeline.push(Step::Keypress {
                key: "enter".to_string(),
            });
        }

        let wait = match commands.get(i + 1) {
            // Pause until the next command starts typing, clamped to a sane range.
            Some(next) => next
                .start_ms
                .saturating_sub(cmd.enter_ms)
                .clamp(MIN_SETTLE_MS, MAX_SETTLE_MS),
            // Final command: hold for the time output kept arriving, capped; the
            // trailing idle that stopped the recording is dropped.
            None => last_output_ms.saturating_sub(cmd.enter_ms).min(MAX_TAIL_MS),
        };
        if wait > 0 {
            timeline.push(Step::Wait { duration_ms: wait });
        }
    }

    timeline.push(Step::Terminate);

    Score {
        demo: DemoMeta {
            name: name.to_string(),
            output_dir: "./dist".into(),
        },
        env: None,
        typing: Some(Typing {
            base_ms: opts.typing_ms,
            salt_ms: opts.salt_ms,
            seed: opts.seed,
        }),
        layout: default_layout(raw),
        timeline,
    }
}

/// A single terminal pane sized to the captured grid.
fn default_layout(raw: &RawMacro) -> Layout {
    let width = (raw.meta.cols as u32 * CELL_W).max(CELL_W);
    let height = (raw.meta.rows as u32 * CELL_H).max(CELL_H);
    Layout {
        width,
        height,
        fps: 15,
        background: Some("#0b0f14".to_string()),
        panes: vec![Pane {
            id: "main".to_string(),
            kind: PaneKind::Terminal,
            x: 0,
            y: 0,
            width,
            height,
            font_family: Some("monospace".to_string()),
            font_size: Some(16),
            url: None,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RawEvent, RawMeta};

    fn raw(events: Vec<RawEvent>) -> RawMacro {
        RawMacro {
            meta: RawMeta {
                shell: "/bin/bash".into(),
                cols: 80,
                rows: 24,
                idle_timeout_ms: 3000,
            },
            events,
        }
    }

    fn opts() -> Options {
        Options {
            typing_ms: 80,
            salt_ms: 15,
            seed: Some(1),
        }
    }

    #[test]
    fn builds_a_valid_score() {
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 100,
                bytes: "ls\r".into(),
            },
            RawEvent::Output {
                t_ms: 200,
                data: "file.txt".into(),
            },
        ]);
        let score = normalize(&r, "demo", &opts());
        // Default layout is one terminal pane filling the canvas.
        assert_eq!(score.layout.panes.len(), 1);
        assert_eq!(score.layout.panes[0].kind, PaneKind::Terminal);
        // The normalized score must itself pass validation.
        assert!(crate::validate::validate(&score).is_empty());
        // focus, type, keypress, (tail wait), terminate
        assert!(matches!(score.timeline.first(), Some(Step::Focus { .. })));
        assert!(matches!(score.timeline.last(), Some(Step::Terminate)));
        assert_eq!(score.typing.as_ref().unwrap().base_ms, 80);
    }

    #[test]
    fn clamps_settle_between_commands() {
        // 10s gap between the two enters → clamped to MAX_SETTLE_MS.
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "ls\r".into(),
            },
            RawEvent::Input {
                t_ms: 10_000,
                bytes: "pwd\r".into(),
            },
        ]);
        let score = normalize(&r, "demo", &opts());
        let waits: Vec<u64> = score
            .timeline
            .iter()
            .filter_map(|s| match s {
                Step::Wait { duration_ms } => Some(*duration_ms),
                _ => None,
            })
            .collect();
        assert!(waits.contains(&MAX_SETTLE_MS));
    }

    #[test]
    fn prunes_typos_end_to_end() {
        let r = raw(vec![RawEvent::Input {
            t_ms: 0,
            bytes: "gti\u{7f}\u{7f}it status\r".into(),
        }]);
        let score = normalize(&r, "demo", &opts());
        let typed: Vec<&str> = score
            .timeline
            .iter()
            .filter_map(|s| match s {
                Step::Type { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(typed, vec!["git status"]);
    }
}
