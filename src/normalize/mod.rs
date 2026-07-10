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
}
