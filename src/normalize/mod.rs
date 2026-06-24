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
use crate::model::{RawEvent, RawMacro, ScrollDirection, Velocity};
use edit::Action;

/// Knobs for normalization (humanized-typing params are stored in the score).
#[derive(Debug, Clone)]
pub struct Options {
    pub typing_ms: u64,
    pub salt_ms: u64,
    pub seed: Option<u64>,
}

/// Longest pause shown between two actions (caps human "think time").
const MAX_SETTLE_MS: u64 = 2500;
/// Longest tail held after the final command (the idle that stopped the
/// recording is trimmed away entirely).
const MAX_TAIL_MS: u64 = 1500;

/// Assumed monospace cell size (px) used to size the default canvas.
const CELL_W: u32 = 10;
const CELL_H: u32 = 20;

/// Normalize a raw capture into a clean score named `name`. A capture with no
/// `demo open` reveals yields a single terminal pane; each reveal adds a browser
/// pane plus a focus (and scroll/hold) step, so a re-run via `demo record`
/// reproduces the browser scene too.
pub fn normalize(raw: &RawMacro, name: &str, opts: &Options) -> Score {
    let reveals = collect_reveals(raw);

    let mut timeline = Vec::new();
    timeline.push(Step::Focus {
        pane: "main".to_string(),
    });
    timeline.extend(terminal_steps(raw, &reveals));
    timeline.push(Step::Terminate);

    Score {
        demo: DemoMeta {
            name: name.to_string(),
            output_dir: "./dist".into(),
            prompt: None,
        },
        env: None,
        typing: Some(typing(opts)),
        layout: layout_with_reveals(raw, &reveals),
        timeline,
    }
}

/// A `demo open` reveal lifted from the raw capture, with a stable scene id.
struct Reveal {
    t_ms: u64,
    id: String,
    url: String,
    mode: String,
    hold_ms: Option<u64>,
    scroll: bool,
}

/// Default time a revealed browser scene is held on screen when no `--hold` was
/// given — longer for a scrolling scene so the pan has time to read.
const REVEAL_HOLD_MS: u64 = 2500;
const SCROLL_HOLD_MS: u64 = 4000;

fn reveal_hold_ms(hold: Option<u64>, scroll: bool) -> u64 {
    hold.unwrap_or(if scroll {
        SCROLL_HOLD_MS
    } else {
        REVEAL_HOLD_MS
    })
}

/// Collect the capture's `demo open` reveals in time order, naming them
/// `scene1`, `scene2`, … to match the panes built in [`layout_with_reveals`].
fn collect_reveals(raw: &RawMacro) -> Vec<Reveal> {
    let mut opens: Vec<(u64, String, String, Option<u64>, bool)> = raw
        .events
        .iter()
        .filter_map(|e| match e {
            RawEvent::Open {
                t_ms,
                url,
                mode,
                hold_ms,
                scroll,
            } => Some((*t_ms, url.clone(), mode.clone(), *hold_ms, *scroll)),
            _ => None,
        })
        .collect();
    opens.sort_by_key(|(t, ..)| *t);
    opens
        .into_iter()
        .enumerate()
        .map(|(i, (t_ms, url, mode, hold_ms, scroll))| Reveal {
            t_ms,
            id: format!("scene{}", i + 1),
            url,
            mode,
            hold_ms,
            scroll,
        })
        .collect()
}

/// Splice the captured terminal flow into a prepared `stage`, keeping its
/// layout, panes and trigger steps. The flow is inserted right after the first
/// `focus` on a terminal pane (or, lacking one, prepended with a focus).
pub fn merge_into_stage(mut stage: Score, raw: &RawMacro, opts: &Options) -> Score {
    let term_id = stage
        .layout
        .panes
        .iter()
        .find(|p| p.kind == PaneKind::Terminal)
        .map(|p| p.id.clone());

    // A prepared stage already declares its own browser panes and trigger steps,
    // so only splice the captured terminal flow (no capture-derived reveals).
    let steps = terminal_steps(raw, &[]);

    let mut timeline = Vec::with_capacity(stage.timeline.len() + steps.len() + 1);
    let mut spliced = false;
    for step in stage.timeline.drain(..) {
        let is_anchor = matches!(&step, Step::Focus { pane } if Some(pane) == term_id.as_ref());
        timeline.push(step);
        if is_anchor && !spliced {
            timeline.extend(steps.iter().cloned());
            spliced = true;
        }
    }
    if !spliced {
        let mut head = Vec::new();
        if let Some(id) = &term_id {
            head.push(Step::Focus { pane: id.clone() });
        }
        head.extend(steps);
        head.append(&mut timeline);
        timeline = head;
    }
    if !timeline.iter().any(|s| matches!(s, Step::Terminate)) {
        timeline.push(Step::Terminate);
    }

    stage.typing = Some(typing(opts));
    stage.timeline = timeline;
    stage
}

fn typing(opts: &Options) -> Typing {
    Typing {
        base_ms: opts.typing_ms,
        salt_ms: opts.salt_ms,
        seed: opts.seed,
    }
}

/// Build the humanized terminal step sequence (Type / Keypress / Wait) from a
/// raw capture — without any Focus/Terminate wrappers, so it can fill either a
/// fresh score or a prepared stage's terminal pane. Any `reveals` are interleaved
/// by time: each opens its browser scene (a focus, an optional scroll, and a hold)
/// at the point in the flow where the capture revealed it.
fn terminal_steps(raw: &RawMacro, reveals: &[Reveal]) -> Vec<Step> {
    let inputs: Vec<(u64, &str)> = raw
        .events
        .iter()
        .filter_map(|e| match e {
            RawEvent::Input { t_ms, bytes } => Some((*t_ms, bytes.as_str())),
            _ => None,
        })
        .collect();

    let mut actions = edit::reconstruct(&inputs);
    strip_trailing_stop(&mut actions);

    let last_output_ms = raw
        .events
        .iter()
        .filter_map(|e| match e {
            RawEvent::Output { t_ms, .. } => Some(*t_ms),
            _ => None,
        })
        .max()
        .unwrap_or(0);

    let mut steps = Vec::with_capacity(actions.len() * 2 + reveals.len() * 3);
    let mut next_reveal = 0;
    for (i, action) in actions.iter().enumerate() {
        // Open any reveal whose moment has arrived before this action; refocus the
        // terminal afterwards, since more terminal input still follows.
        while next_reveal < reveals.len() && reveals[next_reveal].t_ms <= action_start(action) {
            push_reveal(&mut steps, &reveals[next_reveal], true);
            next_reveal += 1;
        }

        match action {
            Action::Type { text, .. } => steps.push(Step::Type {
                text: text.clone(),
                human_salt: true,
            }),
            Action::Key { key, .. } => steps.push(Step::Keypress { key: key.clone() }),
        }

        // Pause from when this action finished until the next one begins, capped.
        let this_end = action_end(action);
        let wait = match actions.get(i + 1) {
            Some(next) => action_start(next)
                .saturating_sub(this_end)
                .min(MAX_SETTLE_MS),
            // Final action: hold for the time output kept arriving, capped.
            None => last_output_ms.saturating_sub(this_end).min(MAX_TAIL_MS),
        };
        if wait > 0 {
            steps.push(Step::Wait { duration_ms: wait });
        }
    }
    // Any reveal after the last command — the common case (open once the demo is
    // done) — closes out the demo, so it keeps focus and just holds.
    for r in &reveals[next_reveal..] {
        push_reveal(&mut steps, r, false);
    }
    steps
}

/// Append the steps that reveal one browser scene: focus it, optionally scroll
/// it, and hold it on screen. `refocus_main` returns focus to the terminal after
/// (for a reveal in the middle of the flow, where typing continues).
fn push_reveal(steps: &mut Vec<Step>, r: &Reveal, refocus_main: bool) {
    let hold = reveal_hold_ms(r.hold_ms, r.scroll);
    steps.push(Step::Focus { pane: r.id.clone() });
    if r.scroll {
        steps.push(Step::Scroll {
            direction: ScrollDirection::Down,
            velocity: Velocity::Constant,
            duration_ms: hold,
            pane: Some(r.id.clone()),
        });
    }
    steps.push(Step::Wait { duration_ms: hold });
    if refocus_main {
        steps.push(Step::Focus {
            pane: "main".to_string(),
        });
    }
}

fn action_start(a: &Action) -> u64 {
    match a {
        Action::Type { t_ms, .. } | Action::Key { t_ms, .. } => *t_ms,
    }
}

fn action_end(a: &Action) -> u64 {
    match a {
        Action::Type { end_ms, .. } => *end_ms,
        Action::Key { t_ms, .. } => *t_ms,
    }
}

/// Drop the trailing `demo stop` the user typed to end the capture (and its
/// `enter`), so it never shows up in the finished demo.
fn strip_trailing_stop(actions: &mut Vec<Action>) {
    let is_enter = |a: &Action| matches!(a, Action::Key { key, .. } if key == "enter");
    let is_stop =
        |a: &Action| matches!(a, Action::Type { text, .. } if text.trim() == crate::STOP_COMMAND);

    let n = actions.len();
    if n >= 2 && is_enter(&actions[n - 1]) && is_stop(&actions[n - 2]) {
        actions.truncate(n - 2);
    } else if actions.last().is_some_and(is_stop) {
        actions.pop();
    }
}

/// Build the layout for a capture: a terminal pane, plus one browser pane per
/// `demo open` reveal — `replace` covers the canvas, `split` sits to the right of
/// the terminal (doubling the canvas width). No reveals → the single-pane default.
fn layout_with_reveals(raw: &RawMacro, reveals: &[Reveal]) -> Layout {
    if reveals.is_empty() {
        return default_layout(raw);
    }
    let tw = (raw.meta.cols as u32 * CELL_W).max(CELL_W);
    let th = (raw.meta.rows as u32 * CELL_H).max(CELL_H);
    let any_split = reveals.iter().any(|r| r.mode == "split");
    let (canvas_w, canvas_h) = if any_split { (tw * 2, th) } else { (tw, th) };

    let mut panes = vec![Pane {
        id: "main".to_string(),
        kind: PaneKind::Terminal,
        x: 0,
        y: 0,
        width: tw,
        height: th,
        font_family: Some("monospace".to_string()),
        font_size: Some(16),
        url: None,
    }];
    for r in reveals {
        let (x, y, w, h) = if r.mode == "split" {
            (tw, 0, canvas_w - tw, th)
        } else {
            (0, 0, canvas_w, canvas_h)
        };
        panes.push(Pane {
            id: r.id.clone(),
            kind: PaneKind::Browser,
            x,
            y,
            width: w,
            height: h,
            font_family: None,
            font_size: None,
            url: Some(r.url.clone()),
        });
    }
    Layout {
        width: canvas_w,
        height: canvas_h,
        fps: 15,
        line_height: 1.2,
        background: Some("#0b0f14".to_string()),
        panes,
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
        line_height: 1.2,
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
                stage: None,
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
    fn trims_trailing_idle_to_a_cap() {
        // Output keeps arriving for 9s after the only command → tail capped.
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "ls\r".into(),
            },
            RawEvent::Output {
                t_ms: 9000,
                data: "done".into(),
            },
        ]);
        let score = normalize(&r, "demo", &opts());
        let last_wait = score.timeline.iter().rev().find_map(|s| match s {
            Step::Wait { duration_ms } => Some(*duration_ms),
            _ => None,
        });
        assert_eq!(last_wait, Some(MAX_TAIL_MS));
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

    #[test]
    fn drops_the_trailing_stop_command() {
        // The `demo stop` the user typed to end the capture is not part of the demo.
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "echo hi\r".into(),
            },
            RawEvent::Input {
                t_ms: 1000,
                bytes: "demo stop\r".into(),
            },
        ]);
        let score = normalize(&r, "demo", &opts());
        let typed: Vec<&str> = score
            .timeline
            .iter()
            .filter_map(|s| match s {
                Step::Type { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(typed, vec!["echo hi"]);
    }

    #[test]
    fn a_capture_reveal_becomes_a_browser_pane_and_focus() {
        // A `demo open --scroll` after the command adds a browser pane plus a
        // focus + scroll + hold so `demo record` reproduces the scene.
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "ls\r".into(),
            },
            RawEvent::Output {
                t_ms: 100,
                data: "file.txt".into(),
            },
            RawEvent::Open {
                t_ms: 500,
                url: "https://example.com".into(),
                mode: "replace".into(),
                hold_ms: Some(5000),
                scroll: true,
            },
        ]);
        let score = normalize(&r, "demo", &opts());
        assert!(crate::validate::validate(&score).is_empty());
        // terminal + one browser scene.
        assert_eq!(score.layout.panes.len(), 2);
        let scene = &score.layout.panes[1];
        assert_eq!(scene.kind, PaneKind::Browser);
        assert_eq!(scene.url.as_deref(), Some("https://example.com"));
        // The timeline focuses the scene, scrolls it, and holds for the requested
        // duration before terminating.
        assert!(score
            .timeline
            .iter()
            .any(|s| matches!(s, Step::Focus { pane } if pane == &scene.id)));
        assert!(score.timeline.iter().any(|s| matches!(
            s,
            Step::Scroll {
                duration_ms: 5000,
                direction: ScrollDirection::Down,
                ..
            }
        )));
        assert!(score
            .timeline
            .iter()
            .any(|s| matches!(s, Step::Wait { duration_ms: 5000 })));
        assert!(matches!(score.timeline.last(), Some(Step::Terminate)));
    }

    const STAGE: &str = r##"
[demo]
name = "texforge"
[layout]
width = 1280
height = 720
  [[layout.panes]]
  id = "term"
  type = "terminal"
  x = 0
  y = 0
  width = 768
  height = 720
  [[layout.panes]]
  id = "view"
  type = "browser"
  x = 768
  y = 0
  width = 512
  height = 720
  url = "file:///x.pdf"
[[timeline]]
action = "focus"
pane = "term"
[[timeline]]
action = "focus"
pane = "view"
[[timeline]]
action = "scroll"
direction = "down"
duration_ms = 1000
pane = "view"
[[timeline]]
action = "terminate"
"##;

    #[test]
    fn merges_flow_into_a_prepared_stage() {
        let stage: Score = toml::from_str(STAGE).unwrap();
        let r = raw(vec![RawEvent::Input {
            t_ms: 0,
            bytes: "ls\r".into(),
        }]);
        let merged = merge_into_stage(stage, &r, &opts());

        // The merged score is valid and keeps the stage's two panes.
        assert!(crate::validate::validate(&merged).is_empty());
        assert_eq!(merged.layout.panes.len(), 2);

        // The captured command was spliced in...
        let typed: Vec<&str> = merged
            .timeline
            .iter()
            .filter_map(|s| match s {
                Step::Type { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(typed, vec!["ls"]);

        // ...after `focus term` and before the stage's `focus view` trigger.
        let type_at = merged
            .timeline
            .iter()
            .position(|s| matches!(s, Step::Type { .. }))
            .unwrap();
        let view_at = merged
            .timeline
            .iter()
            .position(|s| matches!(s, Step::Focus { pane } if pane == "view"))
            .unwrap();
        assert!(type_at < view_at, "flow must precede the view trigger");
    }
}
