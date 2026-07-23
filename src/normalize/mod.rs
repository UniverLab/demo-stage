//! Smart normalizer: turns a raw capture into a clean [`Score`].
//!
//! - §3.1 backspace pruning → [`edit::reconstruct`]
//! - §3.2 humanized typing → [`salt::humanize_delays`] (applied at export)
//! - §3.3 idle trimming → bounded settle waits + trimmed tail, here

mod edit;
mod rng;
pub mod salt;

pub use rng::Rng;

use crate::model::{
    DemoMeta, Layout, Orientation, Pane, PaneKind, RevealPane, Score, Step, Typing,
};
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
/// A pause right before an `enter` is hesitation, not content — every fixed
/// wait immediately followed by an enter keypress is normalized to this, so
/// commands fire with one deliberate rhythm.
const ENTER_SETTLE_MS: u64 = 200;

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
        pane: Some("main".to_string()),
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
        sources: vec![],
        layout: layout_with_reveals(raw, &reveals),
        timeline,
    }
}

/// A reveal lifted from the raw capture: the panes to show, how arranged.
struct Reveal {
    t_ms: u64,
    /// Position among the reveals — makes the per-pane ids stable and unique.
    index: usize,
    panes: Vec<RevealPane>,
    orientation: Orientation,
    hold_ms: Option<u64>,
    scroll: bool,
}

impl Reveal {
    /// The browser panes of this reveal (the terminal excluded), each with a
    /// stable id matching the faithful path (`<source>-r<n>`).
    fn browsers(&self) -> Vec<(String, &RevealPane)> {
        self.panes
            .iter()
            .filter(|p| !p.is_terminal())
            .map(|p| (format!("{}-r{}", p.id, self.index + 1), p))
            .collect()
    }
}

/// Default time a revealed browser scene is held on screen when no `--hold` was
/// given — long enough to read the page; longer for a scrolling scene.
const REVEAL_HOLD_MS: u64 = 6000;
const SCROLL_HOLD_MS: u64 = 8000;

fn reveal_hold_ms(hold: Option<u64>, scroll: bool) -> u64 {
    hold.unwrap_or(if scroll {
        SCROLL_HOLD_MS
    } else {
        REVEAL_HOLD_MS
    })
}

/// Collect the capture's reveals in time order.
fn collect_reveals(raw: &RawMacro) -> Vec<Reveal> {
    let mut revs: Vec<Reveal> = raw
        .events
        .iter()
        .filter_map(|e| match e {
            RawEvent::Reveal {
                t_ms,
                panes,
                orientation,
                hold_ms,
                scroll,
            } => Some(Reveal {
                t_ms: *t_ms,
                index: 0,
                panes: panes.clone(),
                orientation: *orientation,
                hold_ms: *hold_ms,
                scroll: *scroll,
            }),
            _ => None,
        })
        .collect();
    revs.sort_by_key(|r| r.t_ms);
    for (i, r) in revs.iter_mut().enumerate() {
        r.index = i;
    }
    revs
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
        let is_anchor =
            matches!(&step, Step::Focus { pane, .. } if pane.as_ref() == term_id.as_ref());
        timeline.push(step);
        if is_anchor && !spliced {
            timeline.extend(steps.iter().cloned());
            spliced = true;
        }
    }
    if !spliced {
        let mut head = Vec::new();
        if let Some(id) = &term_id {
            head.push(Step::Focus {
                pane: Some(id.clone()),
            });
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
    // Drop input typed inside a `demo open` meta-command span (the command itself
    // and its in-session wizard answers), so it never becomes demo typing.
    let inputs: Vec<(u64, &str)> = raw
        .events
        .iter()
        .filter_map(|e| match e {
            RawEvent::Input { t_ms, bytes } if !raw.meta.is_muted(*t_ms) => {
                Some((*t_ms, bytes.as_str()))
            }
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

    // Collect all output event timestamps for the smart-wait heuristic.
    let output_times: Vec<u64> = raw
        .events
        .iter()
        .filter_map(|e| match e {
            RawEvent::Output { t_ms, .. } => Some(*t_ms),
            _ => None,
        })
        .collect();

    // Secret prompts entered during the capture (label only — no value), in time
    // order, so they can be re-supplied at the same point during `demo record`.
    let mut secrets: Vec<(u64, String)> = raw
        .events
        .iter()
        .filter_map(|e| match e {
            RawEvent::Secret { t_ms, prompt } => Some((*t_ms, prompt.clone())),
            _ => None,
        })
        .collect();
    secrets.sort_by_key(|(t, _)| *t);

    let mut steps = Vec::with_capacity(actions.len() * 2 + reveals.len() * 3 + secrets.len());
    let mut next_reveal = 0;
    let mut next_secret = 0;
    for (i, action) in actions.iter().enumerate() {
        // Supply any secret whose moment arrived before this action (it sits where
        // the redacted keystrokes were — between the command and the next input).
        while next_secret < secrets.len() && secrets[next_secret].0 <= action_start(action) {
            steps.push(Step::Secret {
                prompt: secrets[next_secret].1.clone(),
            });
            next_secret += 1;
        }
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
            // Smart heuristic: if there was significant output activity in this
            // window followed by silence, emit wait_for_quiet instead of a fixed
            // wait — it's more robust against timing variations.
            if should_use_wait_for_quiet(this_end, this_end + wait, &output_times) {
                steps.push(Step::WaitForQuiet {
                    quiet_ms: 500,
                    max_ms: None,
                });
            } else {
                steps.push(Step::Wait { duration_ms: wait });
            }
        }
    }
    // Any secret entered after the last reconstructed input (unusual).
    for (_, prompt) in &secrets[next_secret..] {
        steps.push(Step::Secret {
            prompt: prompt.clone(),
        });
    }
    // Any reveal after the last command — the common case (open once the demo is
    // done) — closes out the demo, so it keeps focus and just holds.
    for r in &reveals[next_reveal..] {
        push_reveal(&mut steps, r, false);
    }
    settle_waits_before_enter(&mut steps);
    steps
}

/// Normalize every fixed `wait` that immediately precedes an `enter` keypress
/// to [`ENTER_SETTLE_MS`]: that pause is hesitation before firing the command,
/// not part of the demo. Event-based waits (`wait_for_quiet`, …) are left alone.
fn settle_waits_before_enter(steps: &mut [Step]) {
    for i in 0..steps.len().saturating_sub(1) {
        let before_enter =
            matches!(&steps[i + 1], Step::Keypress { key } if key.eq_ignore_ascii_case("enter"));
        if before_enter {
            if let Step::Wait { duration_ms } = &mut steps[i] {
                *duration_ms = ENTER_SETTLE_MS;
            }
        }
    }
}

/// Append the steps that reveal a scene: focus each of its browser panes (and
/// optionally scroll them) and hold on screen. A reveal with only the terminal
/// just refocuses `main`. `refocus_main` returns focus to the terminal after (for
/// a reveal mid-flow, where typing continues).
fn push_reveal(steps: &mut Vec<Step>, r: &Reveal, refocus_main: bool) {
    let hold = reveal_hold_ms(r.hold_ms, r.scroll);
    let browsers = r.browsers();
    if browsers.is_empty() {
        // "Back to the terminal" — just focus it.
        steps.push(Step::Focus {
            pane: Some("main".to_string()),
        });
        return;
    }
    for (id, _) in &browsers {
        steps.push(Step::Focus {
            pane: Some(id.clone()),
        });
        if r.scroll {
            steps.push(Step::Scroll {
                direction: ScrollDirection::Down,
                velocity: Velocity::Constant,
                duration_ms: hold,
                pane: Some(id.clone()),
            });
        }
    }
    steps.push(Step::Wait { duration_ms: hold });
    if refocus_main {
        steps.push(Step::Focus {
            pane: Some("main".to_string()),
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

/// Minimum output activity span (ms) to consider the wait "output-dominated".
const OUTPUT_ACTIVITY_MIN_MS: u64 = 200;
/// Minimum trailing silence (ms) after last output in the window to trigger the
/// heuristic. If the user waited this long after output stopped, they were likely
/// waiting for the program to finish.
const TRAILING_SILENCE_MIN_MS: u64 = 300;

/// Returns true if the wait window [from_ms, to_ms) shows a pattern of "output
/// activity followed by silence" — meaning the user was waiting for a program to
/// finish producing output. In that case, `wait_for_quiet` is more robust.
fn should_use_wait_for_quiet(from_ms: u64, to_ms: u64, output_times: &[u64]) -> bool {
    let window_ms = to_ms.saturating_sub(from_ms);
    if window_ms < OUTPUT_ACTIVITY_MIN_MS + TRAILING_SILENCE_MIN_MS {
        return false;
    }
    // Find output events in this window.
    let in_window: Vec<u64> = output_times
        .iter()
        .filter(|&&t| t >= from_ms && t < to_ms)
        .copied()
        .collect();
    if in_window.len() < 2 {
        return false; // Too few output events — probably just a prompt echo.
    }
    let first_out = in_window[0];
    let last_out = in_window[in_window.len() - 1];
    let activity_span = last_out.saturating_sub(first_out);
    let trailing_silence = to_ms.saturating_sub(last_out);

    activity_span >= OUTPUT_ACTIVITY_MIN_MS && trailing_silence >= TRAILING_SILENCE_MIN_MS
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

/// Pane rectangles for a reveal: 1 pane fills the canvas; 2 split it by
/// `orientation` (horizontal → left/right, vertical → top/bottom).
fn pane_rects(
    tw: u32,
    th: u32,
    count: usize,
    orientation: Orientation,
) -> Vec<(u32, u32, u32, u32)> {
    match count {
        0 | 1 => vec![(0, 0, tw, th)],
        _ => match orientation {
            Orientation::Horizontal => {
                let half = tw / 2;
                vec![(0, 0, half, th), (half, 0, tw - half, th)]
            }
            Orientation::Vertical => {
                let half = th / 2;
                vec![(0, 0, tw, half), (0, half, tw, th - half)]
            }
        },
    }
}

/// Build the layout for a capture: the canvas is the terminal size, the terminal
/// pane is the always-on background, and each reveal overlays its browser panes
/// for a window `[reveal_at, hide_at)` so views can switch or hand back to the
/// terminal. No reveals → the single-pane default.
fn layout_with_reveals(raw: &RawMacro, reveals: &[Reveal]) -> Layout {
    if reveals.is_empty() {
        return default_layout(raw);
    }
    let (canvas_w, canvas_h) = raw.meta.resolution.unwrap_or_else(|| {
        let tw = (raw.meta.cols as u32 * CELL_W).max(CELL_W);
        let th = (raw.meta.rows as u32 * CELL_H).max(CELL_H);
        (tw, th)
    });

    let mut panes = vec![Pane {
        id: "main".to_string(),
        kind: PaneKind::Terminal,
        x: 0,
        y: 0,
        width: canvas_w,
        height: canvas_h,
        font_family: Some("monospace".to_string()),
        font_size: Some(16),
        url: None,
        theme: None,
        reveal_at: None,
        hide_at: None,
    }];
    for (i, r) in reveals.iter().enumerate() {
        let reveal_at = r.t_ms as f64 / 1000.0;
        let hide_at = reveals.get(i + 1).map(|n| n.t_ms as f64 / 1000.0);
        let rects = pane_rects(canvas_w, canvas_h, r.panes.len(), r.orientation);
        for ((id, p), (x, y, w, h)) in r
            .browsers()
            .into_iter()
            .zip(browser_rects(&rects, &r.panes))
        {
            panes.push(Pane {
                id,
                kind: PaneKind::Browser,
                x,
                y,
                width: w,
                height: h,
                font_family: None,
                font_size: None,
                url: p.url.clone(),
                theme: p.theme.clone(),
                reveal_at: Some(reveal_at),
                hide_at,
            });
        }
    }
    Layout {
        width: canvas_w,
        height: canvas_h,
        fps: raw.meta.fps.unwrap_or(15),
        line_height: 1.2,
        background: Some("#0b0f14".to_string()),
        font_family: None,
        font_size: None,
        panes,
    }
}

/// Pick the rectangles for the browser panes (in `panes` order, terminal skipped)
/// out of the full reveal `rects`.
fn browser_rects(
    rects: &[(u32, u32, u32, u32)],
    panes: &[RevealPane],
) -> Vec<(u32, u32, u32, u32)> {
    panes
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.is_terminal())
        .map(|(idx, _)| rects.get(idx).copied().unwrap_or((0, 0, 0, 0)))
        .collect()
}

/// A single terminal pane sized to the captured grid (or explicit resolution).
fn default_layout(raw: &RawMacro) -> Layout {
    let (width, height) = raw.meta.resolution.unwrap_or_else(|| {
        let w = (raw.meta.cols as u32 * CELL_W).max(CELL_W);
        let h = (raw.meta.rows as u32 * CELL_H).max(CELL_H);
        (w, h)
    });
    Layout {
        width,
        height,
        fps: raw.meta.fps.unwrap_or(15),
        line_height: 1.2,
        background: Some("#0b0f14".to_string()),
        font_family: None,
        font_size: None,
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
            theme: None,
            reveal_at: None,
            hide_at: None,
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
                resolution: None,
                fps: None,
                stage: None,
                mute_spans: Vec::new(),
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
    fn settles_the_hesitation_wait_before_enter() {
        // 1.5s of thinking between typing the command and pressing enter →
        // pinned to ENTER_SETTLE_MS; other waits keep their computed value.
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "ls".into(),
            },
            RawEvent::Input {
                t_ms: 1500,
                bytes: "\r".into(),
            },
        ]);
        let score = normalize(&r, "demo", &opts());
        let mut saw = false;
        for w in score.timeline.windows(2) {
            if let (Step::Wait { duration_ms }, Step::Keypress { key }) = (&w[0], &w[1]) {
                if key == "enter" {
                    assert_eq!(*duration_ms, ENTER_SETTLE_MS);
                    saw = true;
                }
            }
        }
        assert!(saw, "expected a wait right before the enter keypress");
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
    fn a_secret_event_becomes_a_secret_step() {
        // A captured secret (prompt only) is re-supplied as a Secret step between
        // the command and the next input — the value is never present.
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "ghscaff\r".into(),
            },
            RawEvent::Secret {
                t_ms: 800,
                prompt: "Vault passphrase:".into(),
            },
            RawEvent::Input {
                t_ms: 2000,
                bytes: "demo-repo\r".into(),
            },
        ]);
        let score = normalize(&r, "demo", &opts());
        assert!(crate::validate::validate(&score).is_empty());
        let secrets: Vec<&str> = score
            .timeline
            .iter()
            .filter_map(|s| match s {
                Step::Secret { prompt } => Some(prompt.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(secrets, vec!["Vault passphrase:"]);
        // The secret sits before the repo-name input.
        let secret_at = score
            .timeline
            .iter()
            .position(|s| matches!(s, Step::Secret { .. }))
            .unwrap();
        let repo_at = score
            .timeline
            .iter()
            .position(|s| matches!(s, Step::Type { text, .. } if text == "demo-repo"))
            .unwrap();
        assert!(secret_at < repo_at);
    }

    #[test]
    fn drops_input_typed_inside_a_meta_command_span() {
        // The `demo open` command + its wizard answers (typed at 1000..3000) are
        // excised, so they never become demo typing.
        let mut r = raw(vec![
            RawEvent::Input {
                t_ms: 100,
                bytes: "echo hi\r".into(),
            },
            RawEvent::Input {
                t_ms: 1200,
                bytes: "demo open\r".into(),
            },
            RawEvent::Input {
                t_ms: 1800,
                bytes: "https://example.com\r".into(),
            },
        ]);
        r.meta.mute_spans = vec![(1000, 3000)];
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
            RawEvent::Reveal {
                t_ms: 500,
                panes: vec![RevealPane {
                    id: "browser".into(),
                    url: Some("https://example.com".into()),
                    theme: None,
                }],
                orientation: Orientation::Horizontal,
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
            .any(|s| matches!(s, Step::Focus { pane, .. } if pane.as_ref() == Some(&scene.id))));
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
            .position(|s| matches!(s, Step::Focus { pane, .. } if pane.as_deref() == Some("view")))
            .unwrap();
        assert!(type_at < view_at, "flow must precede the view trigger");
    }

    // ── helper function coverage ──────────────────────────────────────

    #[test]
    fn reveal_hold_ms_with_explicit_hold() {
        assert_eq!(reveal_hold_ms(Some(3000), false), 3000);
        assert_eq!(reveal_hold_ms(Some(5000), true), 5000);
    }

    #[test]
    fn reveal_hold_ms_default_non_scroll() {
        assert_eq!(reveal_hold_ms(None, false), REVEAL_HOLD_MS);
    }

    #[test]
    fn reveal_hold_ms_default_scroll() {
        assert_eq!(reveal_hold_ms(None, true), SCROLL_HOLD_MS);
    }

    #[test]
    fn collect_reveals_sorted_by_time() {
        let r = raw(vec![
            RawEvent::Reveal {
                t_ms: 500,
                panes: vec![RevealPane {
                    id: "b".into(),
                    url: Some("https://b".into()),
                    theme: None,
                }],
                orientation: Orientation::Horizontal,
                hold_ms: None,
                scroll: false,
            },
            RawEvent::Reveal {
                t_ms: 100,
                panes: vec![RevealPane {
                    id: "a".into(),
                    url: Some("https://a".into()),
                    theme: None,
                }],
                orientation: Orientation::Horizontal,
                hold_ms: None,
                scroll: false,
            },
        ]);
        let reveals = collect_reveals(&r);
        assert_eq!(reveals.len(), 2);
        assert_eq!(reveals[0].t_ms, 100);
        assert_eq!(reveals[1].t_ms, 500);
        assert_eq!(reveals[0].index, 0);
        assert_eq!(reveals[1].index, 1);
    }

    #[test]
    fn collect_reveals_empty_when_no_reveals() {
        let r = raw(vec![RawEvent::Output {
            t_ms: 0,
            data: "hi".into(),
        }]);
        let reveals = collect_reveals(&r);
        assert!(reveals.is_empty());
    }

    #[test]
    fn action_start_and_end_for_type() {
        let a = Action::Type {
            text: "ls".into(),
            t_ms: 100,
            end_ms: 200,
        };
        assert_eq!(action_start(&a), 100);
        assert_eq!(action_end(&a), 200);
    }

    #[test]
    fn action_start_and_end_for_key() {
        let a = Action::Key {
            key: "enter".into(),
            t_ms: 300,
        };
        assert_eq!(action_start(&a), 300);
        assert_eq!(action_end(&a), 300);
    }

    #[test]
    fn should_use_wait_for_quiet_true_when_gap() {
        // 2s gap between commands with output activity then silence → use wait_for_quiet
        assert!(should_use_wait_for_quiet(0, 2000, &[500, 800]));
    }

    #[test]
    fn should_use_wait_for_quiet_false_when_no_output() {
        // No output during the 0..2s gap → can't use wait_for_quiet
        assert!(!should_use_wait_for_quiet(0, 2000, &[]));
    }

    #[test]
    fn should_use_wait_for_quiet_false_when_short_gap() {
        // Short gap (< 500ms) → no wait needed
        assert!(!should_use_wait_for_quiet(0, 200, &[100]));
    }

    #[test]
    fn should_use_wait_for_quiet_false_single_output() {
        // Only one output event → too few
        assert!(!should_use_wait_for_quiet(0, 2000, &[500]));
    }

    #[test]
    fn strip_trailing_stop_removes_demo_stop() {
        let mut actions = vec![
            Action::Type {
                text: "echo hi".into(),
                t_ms: 0,
                end_ms: 100,
            },
            Action::Key {
                key: "enter".into(),
                t_ms: 100,
            },
            Action::Type {
                text: "demo stop".into(),
                t_ms: 200,
                end_ms: 300,
            },
            Action::Key {
                key: "enter".into(),
                t_ms: 300,
            },
        ];
        strip_trailing_stop(&mut actions);
        assert_eq!(actions.len(), 2);
        assert!(matches!(&actions[0], Action::Type { text, .. } if text == "echo hi"));
    }

    #[test]
    fn strip_trailing_stop_preserves_non_stop() {
        let mut actions = vec![
            Action::Type {
                text: "ls".into(),
                t_ms: 0,
                end_ms: 50,
            },
            Action::Key {
                key: "enter".into(),
                t_ms: 50,
            },
        ];
        strip_trailing_stop(&mut actions);
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn typing_function_uses_options() {
        let opts = Options {
            typing_ms: 100,
            salt_ms: 20,
            seed: Some(42),
        };
        let t = typing(&opts);
        assert_eq!(t.base_ms, 100);
        assert_eq!(t.salt_ms, 20);
        assert_eq!(t.seed, Some(42));
    }

    #[test]
    fn default_layout_sets_canvas_dimensions() {
        let r = raw(vec![RawEvent::Output {
            t_ms: 0,
            data: "x".into(),
        }]);
        let layout = default_layout(&r);
        assert_eq!(layout.width, 800);
        assert_eq!(layout.height, 480);
        assert_eq!(layout.panes.len(), 1);
        assert_eq!(layout.panes[0].kind, PaneKind::Terminal);
    }

    #[test]
    fn default_layout_with_explicit_resolution() {
        let mut r = raw(vec![RawEvent::Output {
            t_ms: 0,
            data: "x".into(),
        }]);
        r.meta.resolution = Some((1920, 1080));
        let layout = default_layout(&r);
        assert_eq!(layout.width, 1920);
        assert_eq!(layout.height, 1080);
    }

    #[test]
    fn pane_rects_single_terminal() {
        let rects = pane_rects(800, 480, 1, Orientation::Horizontal);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0], (0, 0, 800, 480));
    }

    #[test]
    fn pane_rects_horizontal_split() {
        let rects = pane_rects(800, 480, 2, Orientation::Horizontal);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0], (0, 0, 400, 480));
        assert_eq!(rects[1], (400, 0, 400, 480));
    }

    #[test]
    fn pane_rects_vertical_split() {
        let rects = pane_rects(800, 480, 2, Orientation::Vertical);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0], (0, 0, 800, 240));
        assert_eq!(rects[1], (0, 240, 800, 240));
    }

    #[test]
    fn pane_rects_empty() {
        let rects = pane_rects(800, 480, 0, Orientation::Horizontal);
        assert_eq!(rects.len(), 1);
    }

    #[test]
    fn reveal_hold_ms_with_zero_hold() {
        assert_eq!(reveal_hold_ms(Some(0), false), 0);
    }

    #[test]
    fn collect_reveals_preserves_orientation() {
        let r = raw(vec![RawEvent::Reveal {
            t_ms: 100,
            panes: vec![RevealPane {
                id: "b".into(),
                url: Some("https://b".into()),
                theme: None,
            }],
            orientation: Orientation::Vertical,
            hold_ms: Some(3000),
            scroll: true,
        }]);
        let reveals = collect_reveals(&r);
        assert_eq!(reveals[0].orientation, Orientation::Vertical);
        assert_eq!(reveals[0].hold_ms, Some(3000));
        assert!(reveals[0].scroll);
    }

    #[test]
    fn merge_stage_prepends_focus_when_no_terminal_anchor() {
        // A stage with no focus on the terminal pane → merged flow is prepended.
        let stage: Score = toml::from_str(
            r##"
[demo]
name = "t"
[layout]
width = 800
height = 480
  [[layout.panes]]
  id = "main"
  type = "terminal"
  x = 0
  y = 0
  width = 800
  height = 480
[[timeline]]
action = "focus"
pane = "main"
[[timeline]]
action = "terminate"
"##,
        )
        .unwrap();
        let r = raw(vec![RawEvent::Input {
            t_ms: 0,
            bytes: "echo\r".into(),
        }]);
        let merged = merge_into_stage(stage, &r, &opts());
        let typed: Vec<&str> = merged
            .timeline
            .iter()
            .filter_map(|s| match s {
                Step::Type { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(typed, vec!["echo"]);
    }

    #[test]
    fn merge_stage_adds_terminate_if_missing() {
        let stage: Score = toml::from_str(
            r##"
[demo]
name = "t"
[layout]
width = 800
height = 480
  [[layout.panes]]
  id = "main"
  type = "terminal"
  x = 0
  y = 0
  width = 800
  height = 480
[[timeline]]
action = "focus"
pane = "main"
"##,
        )
        .unwrap();
        let r = raw(vec![RawEvent::Input {
            t_ms: 0,
            bytes: "ls\r".into(),
        }]);
        let merged = merge_into_stage(stage, &r, &opts());
        assert!(matches!(merged.timeline.last(), Some(Step::Terminate)));
    }

    #[test]
    fn terminal_steps_with_secret_between_commands() {
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "git pull\r".into(),
            },
            RawEvent::Secret {
                t_ms: 500,
                prompt: "Passphrase:".into(),
            },
            RawEvent::Input {
                t_ms: 1000,
                bytes: "echo done\r".into(),
            },
        ]);
        let reveals = collect_reveals(&r);
        let steps = terminal_steps(&r, &reveals);
        let secrets: Vec<&str> = steps
            .iter()
            .filter_map(|s| match s {
                Step::Secret { prompt } => Some(prompt.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(secrets, vec!["Passphrase:"]);
    }

    #[test]
    fn terminal_steps_with_reveal_after_last_command() {
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "ls\r".into(),
            },
            RawEvent::Reveal {
                t_ms: 500,
                panes: vec![RevealPane {
                    id: "browser".into(),
                    url: Some("https://example.com".into()),
                    theme: None,
                }],
                orientation: Orientation::Horizontal,
                hold_ms: None,
                scroll: false,
            },
        ]);
        let reveals = collect_reveals(&r);
        let steps = terminal_steps(&r, &reveals);
        // Browser pane IDs are formatted as "{id}-r{index}"
        assert!(steps.iter().any(
            |s| matches!(s, Step::Focus { pane, .. } if pane.as_deref() == Some("browser-r1"))
        ));
    }

    #[test]
    fn terminal_steps_with_reveal_before_command() {
        let r = raw(vec![
            RawEvent::Reveal {
                t_ms: 0,
                panes: vec![RevealPane {
                    id: "docs".into(),
                    url: Some("https://docs.rs".into()),
                    theme: None,
                }],
                orientation: Orientation::Horizontal,
                hold_ms: None,
                scroll: false,
            },
            RawEvent::Input {
                t_ms: 500,
                bytes: "ls\r".into(),
            },
        ]);
        let reveals = collect_reveals(&r);
        let steps = terminal_steps(&r, &reveals);
        // Browser pane IDs are formatted as "{id}-r{index}"
        let focus_at = steps.iter().position(
            |s| matches!(s, Step::Focus { pane, .. } if pane.as_deref() == Some("docs-r1")),
        );
        let type_at = steps.iter().position(|s| matches!(s, Step::Type { .. }));
        assert!(focus_at.unwrap() < type_at.unwrap());
    }

    #[test]
    fn terminal_steps_secret_after_last_input() {
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "ssh host\r".into(),
            },
            RawEvent::Secret {
                t_ms: 1000,
                prompt: "password:".into(),
            },
        ]);
        let reveals = collect_reveals(&r);
        let steps = terminal_steps(&r, &reveals);
        let secrets: Vec<&str> = steps
            .iter()
            .filter_map(|s| match s {
                Step::Secret { prompt } => Some(prompt.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(secrets, vec!["password:"]);
    }

    #[test]
    fn settle_waits_normalizes_wait_before_enter() {
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "cmd".into(),
            },
            RawEvent::Input {
                t_ms: 800,
                bytes: "\r".into(),
            },
        ]);
        let score = normalize(&r, "t", &opts());
        // The wait before enter should be ENTER_SETTLE_MS
        for w in score.timeline.windows(2) {
            if let (Step::Wait { duration_ms }, Step::Keypress { key }) = (&w[0], &w[1]) {
                if key == "enter" {
                    assert_eq!(*duration_ms, ENTER_SETTLE_MS);
                }
            }
        }
    }

    #[test]
    fn wait_for_quiet_emitted_for_long_gap_with_output() {
        // A 2s gap with output at 500ms and 800ms → should use wait_for_quiet
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "make\r".into(),
            },
            RawEvent::Output {
                t_ms: 500,
                data: "building...".into(),
            },
            RawEvent::Output {
                t_ms: 800,
                data: "done!".into(),
            },
            RawEvent::Input {
                t_ms: 2000,
                bytes: "echo ok\r".into(),
            },
        ]);
        let score = normalize(&r, "t", &opts());
        let has_quiet = score
            .timeline
            .iter()
            .any(|s| matches!(s, Step::WaitForQuiet { .. }));
        assert!(has_quiet, "expected wait_for_quiet in the timeline");
    }

    #[test]
    fn no_wait_for_quiet_for_short_gap() {
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "ls\r".into(),
            },
            RawEvent::Input {
                t_ms: 100,
                bytes: "pwd\r".into(),
            },
        ]);
        let score = normalize(&r, "t", &opts());
        let has_quiet = score
            .timeline
            .iter()
            .any(|s| matches!(s, Step::WaitForQuiet { .. }));
        assert!(!has_quiet, "should not use wait_for_quiet for short gaps");
    }

    #[test]
    fn strip_trailing_stop_with_only_stop() {
        let mut actions = vec![
            Action::Type {
                text: "demo stop".into(),
                t_ms: 0,
                end_ms: 100,
            },
            Action::Key {
                key: "enter".into(),
                t_ms: 100,
            },
        ];
        strip_trailing_stop(&mut actions);
        assert!(actions.is_empty());
    }

    #[test]
    fn strip_trailing_stop_empty() {
        let mut actions: Vec<Action> = vec![];
        strip_trailing_stop(&mut actions);
        assert!(actions.is_empty());
    }

    #[test]
    fn pane_rects_many_horizontal_falls_back_to_two() {
        let rects = pane_rects(900, 600, 5, Orientation::Horizontal);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0], (0, 0, 450, 600));
        assert_eq!(rects[1], (450, 0, 450, 600));
    }

    #[test]
    fn pane_rects_many_vertical_falls_back_to_two() {
        let rects = pane_rects(900, 600, 5, Orientation::Vertical);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0], (0, 0, 900, 300));
        assert_eq!(rects[1], (0, 300, 900, 300));
    }

    #[test]
    fn layout_with_reveals_single_terminal_no_reveals() {
        let r = raw(vec![RawEvent::Output {
            t_ms: 0,
            data: "x".into(),
        }]);
        let reveals = collect_reveals(&r);
        let layout = layout_with_reveals(&r, &reveals);
        assert_eq!(layout.panes.len(), 1);
        assert_eq!(layout.panes[0].kind, PaneKind::Terminal);
    }

    #[test]
    fn layout_with_reveals_with_horizontal_split() {
        let r = raw(vec![RawEvent::Reveal {
            t_ms: 0,
            panes: vec![
                RevealPane::terminal(),
                RevealPane {
                    id: "web".into(),
                    url: Some("https://x.com".into()),
                    theme: None,
                },
            ],
            orientation: Orientation::Horizontal,
            hold_ms: None,
            scroll: false,
        }]);
        let reveals = collect_reveals(&r);
        let layout = layout_with_reveals(&r, &reveals);
        assert_eq!(layout.panes.len(), 2);
        let web = &layout.panes[1];
        assert_eq!(web.kind, PaneKind::Browser);
        assert!(web.x > 0, "browser pane should be offset to the right");
    }

    #[test]
    fn layout_with_reveals_with_vertical_split() {
        let r = raw(vec![RawEvent::Reveal {
            t_ms: 0,
            panes: vec![
                RevealPane::terminal(),
                RevealPane {
                    id: "web".into(),
                    url: Some("https://x.com".into()),
                    theme: None,
                },
            ],
            orientation: Orientation::Vertical,
            hold_ms: None,
            scroll: false,
        }]);
        let reveals = collect_reveals(&r);
        let layout = layout_with_reveals(&r, &reveals);
        assert_eq!(layout.panes.len(), 2);
        let web = &layout.panes[1];
        assert_eq!(web.kind, PaneKind::Browser);
        assert!(web.y > 0, "browser pane should be offset downward");
    }

    #[test]
    fn normalize_empty_capture() {
        let r = raw(vec![]);
        let score = normalize(&r, "t", &opts());
        assert!(crate::validate::validate(&score).is_empty());
    }

    #[test]
    fn should_use_wait_for_quiet_false_short_activity_span() {
        // Output activity span < 200ms
        assert!(!should_use_wait_for_quiet(0, 2000, &[100, 200]));
    }

    #[test]
    fn should_use_wait_for_quiet_false_short_trailing_silence() {
        // Activity span ok but trailing silence < 300ms (600 to 800 = 200ms)
        assert!(!should_use_wait_for_quiet(0, 800, &[100, 400, 600]));
    }

    #[test]
    fn should_use_wait_for_quiet_exact_boundary() {
        // Exactly OUTPUT_ACTIVITY_MIN_MS activity and TRAILING_SILENCE_MIN_MS silence
        assert!(should_use_wait_for_quiet(0, 500, &[0, 200, 200]));
    }

    #[test]
    fn should_use_wait_for_quiet_output_outside_window() {
        // Output exists but outside the [from, to) window
        assert!(!should_use_wait_for_quiet(500, 1000, &[100, 200]));
    }

    #[test]
    fn default_layout_with_fps() {
        let mut r = raw(vec![]);
        r.meta.fps = Some(30);
        let layout = default_layout(&r);
        assert_eq!(layout.fps, 30);
    }

    #[test]
    fn default_layout_without_fps() {
        let r = raw(vec![]);
        let layout = default_layout(&r);
        assert_eq!(layout.fps, 15);
    }

    #[test]
    fn layout_with_reveals_with_resolution_and_fps() {
        let mut r = raw(vec![RawEvent::Reveal {
            t_ms: 0,
            panes: vec![
                RevealPane::terminal(),
                RevealPane {
                    id: "web".into(),
                    url: Some("https://x.com".into()),
                    theme: None,
                },
            ],
            orientation: Orientation::Horizontal,
            hold_ms: None,
            scroll: false,
        }]);
        r.meta.resolution = Some((1920, 1080));
        r.meta.fps = Some(30);
        let reveals = collect_reveals(&r);
        let layout = layout_with_reveals(&r, &reveals);
        assert_eq!(layout.width, 1920);
        assert_eq!(layout.height, 1080);
        assert_eq!(layout.fps, 30);
        assert_eq!(layout.panes.len(), 2);
    }

    #[test]
    fn layout_with_reveals_multiple_reveals() {
        let r = raw(vec![
            RawEvent::Reveal {
                t_ms: 1000,
                panes: vec![
                    RevealPane::terminal(),
                    RevealPane {
                        id: "web".into(),
                        url: Some("https://a.com".into()),
                        theme: None,
                    },
                ],
                orientation: Orientation::Horizontal,
                hold_ms: None,
                scroll: false,
            },
            RawEvent::Reveal {
                t_ms: 5000,
                panes: vec![
                    RevealPane::terminal(),
                    RevealPane {
                        id: "docs".into(),
                        url: Some("https://b.com".into()),
                        theme: None,
                    },
                ],
                orientation: Orientation::Vertical,
                hold_ms: Some(3000),
                scroll: false,
            },
        ]);
        let reveals = collect_reveals(&r);
        let layout = layout_with_reveals(&r, &reveals);
        // Terminal pane + 2 browser panes
        assert_eq!(layout.panes.len(), 3);
        // First browser has hide_at = second reveal time (5.0s)
        let web = &layout.panes[1];
        assert_eq!(web.reveal_at, Some(1.0));
        assert_eq!(web.hide_at, Some(5.0));
        // Second browser has no hide_at
        let docs = &layout.panes[2];
        assert_eq!(docs.reveal_at, Some(5.0));
        assert_eq!(docs.hide_at, None);
    }

    #[test]
    fn layout_with_reveals_terminal_only_reveal() {
        // A reveal with only terminal pane (no browser)
        let r = raw(vec![RawEvent::Reveal {
            t_ms: 1000,
            panes: vec![RevealPane::terminal()],
            orientation: Orientation::Horizontal,
            hold_ms: None,
            scroll: false,
        }]);
        let reveals = collect_reveals(&r);
        let layout = layout_with_reveals(&r, &reveals);
        // Only the terminal pane (the reveal has no browser)
        assert_eq!(layout.panes.len(), 1);
    }

    #[test]
    fn strip_trailing_stop_only_stop_no_enter() {
        // Just "demo stop" without enter at the end
        let mut actions = vec![Action::Type {
            text: "demo stop".into(),
            t_ms: 0,
            end_ms: 100,
        }];
        strip_trailing_stop(&mut actions);
        assert!(actions.is_empty());
    }

    #[test]
    fn strip_trailing_stop_enter_without_stop() {
        // Enter that's not after stop should be kept
        let mut actions = vec![
            Action::Type {
                text: "ls".into(),
                t_ms: 0,
                end_ms: 50,
            },
            Action::Key {
                key: "enter".into(),
                t_ms: 50,
            },
            Action::Type {
                text: "echo hi".into(),
                t_ms: 100,
                end_ms: 200,
            },
            Action::Key {
                key: "enter".into(),
                t_ms: 200,
            },
        ];
        strip_trailing_stop(&mut actions);
        assert_eq!(actions.len(), 4);
    }

    #[test]
    fn strip_trailing_stop_stop_with_whitespace() {
        // "demo stop  " with trailing whitespace
        let mut actions = vec![
            Action::Type {
                text: "demo stop  ".into(),
                t_ms: 0,
                end_ms: 100,
            },
            Action::Key {
                key: "enter".into(),
                t_ms: 100,
            },
        ];
        strip_trailing_stop(&mut actions);
        assert!(actions.is_empty());
    }

    #[test]
    fn browser_rects_filters_terminal() {
        let panes = vec![
            RevealPane::terminal(),
            RevealPane {
                id: "web".into(),
                url: Some("https://x.com".into()),
                theme: None,
            },
        ];
        let rects = vec![(0, 0, 800, 480), (400, 0, 400, 480)];
        let result = browser_rects(&rects, &panes);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], (400, 0, 400, 480));
    }

    #[test]
    fn browser_rects_all_browser() {
        let panes = vec![
            RevealPane {
                id: "a".into(),
                url: Some("https://a.com".into()),
                theme: None,
            },
            RevealPane {
                id: "b".into(),
                url: Some("https://b.com".into()),
                theme: None,
            },
        ];
        let rects = vec![(0, 0, 400, 480), (400, 0, 400, 480)];
        let result = browser_rects(&rects, &panes);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn browser_rects_out_of_bounds_index() {
        let panes = vec![RevealPane {
            id: "a".into(),
            url: Some("https://a.com".into()),
            theme: None,
        }];
        let rects = vec![];
        let result = browser_rects(&rects, &panes);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], (0, 0, 0, 0));
    }

    #[test]
    fn settle_waits_before_enter_leaves_non_enter_alone() {
        let mut steps = vec![
            Step::Wait { duration_ms: 1000 },
            Step::Keypress { key: "tab".into() },
        ];
        settle_waits_before_enter(&mut steps);
        assert_eq!(steps[0], Step::Wait { duration_ms: 1000 });
    }

    #[test]
    fn settle_waits_before_enter_leaves_non_wait_alone() {
        let mut steps = vec![
            Step::Type {
                text: "ls".into(),
                human_salt: false,
            },
            Step::Keypress {
                key: "enter".into(),
            },
        ];
        settle_waits_before_enter(&mut steps);
        assert!(matches!(&steps[0], Step::Type { .. }));
    }

    #[test]
    fn push_reveal_terminal_only() {
        let mut steps = Vec::new();
        let r = Reveal {
            t_ms: 1000,
            index: 0,
            panes: vec![RevealPane::terminal()],
            orientation: Orientation::Horizontal,
            hold_ms: None,
            scroll: false,
        };
        push_reveal(&mut steps, &r, true);
        assert_eq!(steps.len(), 1);
        assert!(matches!(&steps[0], Step::Focus { pane } if pane.as_deref() == Some("main")));
    }

    #[test]
    fn push_reveal_with_scroll() {
        let mut steps = Vec::new();
        let r = Reveal {
            t_ms: 1000,
            index: 0,
            panes: vec![RevealPane {
                id: "web".into(),
                url: Some("https://x.com".into()),
                theme: None,
            }],
            orientation: Orientation::Horizontal,
            hold_ms: Some(2000),
            scroll: true,
        };
        push_reveal(&mut steps, &r, false);
        // Focus + Scroll + Wait = 3 steps
        assert_eq!(steps.len(), 3);
        assert!(matches!(&steps[0], Step::Focus { .. }));
        assert!(matches!(&steps[1], Step::Scroll { .. }));
        assert!(matches!(&steps[2], Step::Wait { .. }));
    }

    #[test]
    fn push_reveal_without_scroll() {
        let mut steps = Vec::new();
        let r = Reveal {
            t_ms: 1000,
            index: 0,
            panes: vec![RevealPane {
                id: "web".into(),
                url: Some("https://x.com".into()),
                theme: None,
            }],
            orientation: Orientation::Horizontal,
            hold_ms: Some(2000),
            scroll: false,
        };
        push_reveal(&mut steps, &r, true);
        // Focus + Wait + Focus(main) = 3 steps
        assert_eq!(steps.len(), 3);
        assert!(matches!(&steps[0], Step::Focus { .. }));
        assert!(matches!(&steps[1], Step::Wait { .. }));
        assert!(matches!(&steps[2], Step::Focus { pane } if pane.as_deref() == Some("main")));
    }

    #[test]
    fn push_reveal_multiple_browsers() {
        let mut steps = Vec::new();
        let r = Reveal {
            t_ms: 1000,
            index: 0,
            panes: vec![
                RevealPane::terminal(),
                RevealPane {
                    id: "a".into(),
                    url: Some("https://a.com".into()),
                    theme: None,
                },
                RevealPane {
                    id: "b".into(),
                    url: Some("https://b.com".into()),
                    theme: None,
                },
            ],
            orientation: Orientation::Horizontal,
            hold_ms: Some(2000),
            scroll: false,
        };
        push_reveal(&mut steps, &r, false);
        // 2 focuses (for a and b) + 1 wait = 3 steps
        let focuses: Vec<_> = steps
            .iter()
            .filter(|s| matches!(s, Step::Focus { .. }))
            .collect();
        assert_eq!(focuses.len(), 2);
    }

    #[test]
    fn normalize_output_only_no_input() {
        let r = raw(vec![
            RawEvent::Output {
                t_ms: 0,
                data: "welcome".into(),
            },
            RawEvent::Output {
                t_ms: 100,
                data: "banner".into(),
            },
        ]);
        let score = normalize(&r, "t", &opts());
        assert!(crate::validate::validate(&score).is_empty());
        // No type steps since no input
        let types: Vec<&str> = score
            .timeline
            .iter()
            .filter_map(|s| match s {
                Step::Type { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(types.is_empty());
    }

    #[test]
    fn normalize_preserves_ctrl_c_as_step() {
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "sleep 100\r".into(),
            },
            RawEvent::Input {
                t_ms: 500,
                bytes: "\u{3}".into(),
            },
        ]);
        let score = normalize(&r, "t", &opts());
        let has_ctrl_c = score
            .timeline
            .iter()
            .any(|s| matches!(s, Step::Keypress { key } if key == "ctrl+c"));
        assert!(has_ctrl_c, "ctrl+c should be preserved");
    }

    #[test]
    fn normalize_multiple_commands_with_output() {
        let r = raw(vec![
            RawEvent::Input {
                t_ms: 0,
                bytes: "echo a\r".into(),
            },
            RawEvent::Output {
                t_ms: 100,
                data: "a\n".into(),
            },
            RawEvent::Input {
                t_ms: 200,
                bytes: "echo b\r".into(),
            },
            RawEvent::Output {
                t_ms: 300,
                data: "b\n".into(),
            },
        ]);
        let score = normalize(&r, "t", &opts());
        assert!(crate::validate::validate(&score).is_empty());
        let types: Vec<&str> = score
            .timeline
            .iter()
            .filter_map(|s| match s {
                Step::Type { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(types, vec!["echo a", "echo b"]);
    }
}
