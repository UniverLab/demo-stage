//! Static validation of a [`Score`] — the engine behind `demo check`.
//!
//! Pure and dependency-free: returns the list of problems found (empty = valid)
//! so it is trivially unit-testable and reusable by `export`.

use crate::model::{PaneKind, Score, Step};

/// Validate a score, returning a human-readable problem for each issue found.
/// An empty result means the score is valid.
pub fn validate(score: &Score) -> Vec<String> {
    let mut problems = Vec::new();

    // ── Required environment (provided at export time, not stored) ────────
    if let Some(env) = &score.env {
        for var in &env.requires {
            if std::env::var(var).is_err() {
                problems.push(format!(
                    "env: required variable ${var} is not set (export reads it from your environment)"
                ));
            }
        }
    }

    let layout = &score.layout;

    // ── Canvas ────────────────────────────────────────────────────────────
    if layout.width == 0 || layout.height == 0 {
        problems.push(format!(
            "layout: width and height must be > 0 (got {}x{})",
            layout.width, layout.height
        ));
    }
    if layout.fps == 0 {
        problems.push("layout: fps must be > 0".to_string());
    }
    if layout.panes.is_empty() {
        problems.push("layout: at least one pane is required".to_string());
    }

    // ── Panes ─────────────────────────────────────────────────────────────
    let mut seen = std::collections::HashSet::new();
    for pane in &layout.panes {
        let id = &pane.id;
        if !seen.insert(id.as_str()) {
            problems.push(format!("pane '{id}': duplicate id"));
        }
        if pane.width == 0 || pane.height == 0 {
            problems.push(format!("pane '{id}': width and height must be > 0"));
        }
        // Use u64 math to avoid overflow on absurd values.
        let (right, bottom) = (
            pane.x as u64 + pane.width as u64,
            pane.y as u64 + pane.height as u64,
        );
        if right > layout.width as u64 || bottom > layout.height as u64 {
            problems.push(format!(
                "pane '{id}': extends beyond the {}x{} canvas (x={}, y={}, w={}, h={})",
                layout.width, layout.height, pane.x, pane.y, pane.width, pane.height
            ));
        }
        if pane.kind == PaneKind::Browser && pane.url.is_none() {
            problems.push(format!("pane '{id}': browser panes require a `url`"));
        }
    }

    // ── Timeline (focus state machine + pane references) ──────────────────
    let kind_of = |id: &str| layout.panes.iter().find(|p| p.id == id).map(|p| p.kind);
    let mut focused: Option<&str> = None;
    for (i, step) in score.timeline.iter().enumerate() {
        let at = format!("timeline[{i}]");
        match step {
            Step::Focus { pane } => match kind_of(pane) {
                None => problems.push(format!("{at}: focus references unknown pane '{pane}'")),
                Some(_) => focused = Some(pane.as_str()),
            },
            Step::Type { .. } | Step::Keypress { .. } | Step::Secret { .. } => match focused {
                None => problems.push(format!(
                    "{at}: input with no focused pane (add a `focus` first)"
                )),
                Some(id) if kind_of(id) != Some(PaneKind::Terminal) => problems.push(format!(
                    "{at}: input requires a focused terminal pane ('{id}' is a browser)"
                )),
                Some(_) => {}
            },
            Step::WaitForStdout { pane, .. } => {
                check_target(
                    &mut problems,
                    &at,
                    "wait_for_stdout",
                    pane.as_deref().or(focused),
                    &kind_of,
                    PaneKind::Terminal,
                );
            }
            Step::Scroll { pane, .. } => {
                check_target(
                    &mut problems,
                    &at,
                    "scroll",
                    pane.as_deref().or(focused),
                    &kind_of,
                    PaneKind::Browser,
                );
            }
            Step::Wait { .. } | Step::WaitForQuiet { .. } | Step::Caption { .. } | Step::Terminate => {}
        }
    }

    problems
}

/// Shared check for steps that target a pane of an expected kind.
fn check_target(
    problems: &mut Vec<String>,
    at: &str,
    action: &str,
    target: Option<&str>,
    kind_of: &impl Fn(&str) -> Option<PaneKind>,
    want: PaneKind,
) {
    match target {
        None => problems.push(format!(
            "{at}: {action} needs a pane (none given, none focused)"
        )),
        Some(id) => match kind_of(id) {
            None => problems.push(format!("{at}: {action} references unknown pane '{id}'")),
            Some(k) if k != want => {
                problems.push(format!(
                    "{at}: {action} target '{id}' must be a {want:?} pane"
                ));
            }
            Some(_) => {}
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score_from(toml_str: &str) -> Score {
        toml::from_str(toml_str).expect("test score parses")
    }

    const VALID: &str = r##"
[demo]
name = "t"
[layout]
width = 200
height = 100
  [[layout.panes]]
  id = "c"
  type = "terminal"
  x = 0
  y = 0
  width = 100
  height = 100
  [[layout.panes]]
  id = "p"
  type = "browser"
  x = 100
  y = 0
  width = 100
  height = 100
  url = "file:///x.pdf"
[[timeline]]
action = "focus"
pane = "c"
[[timeline]]
action = "type"
text = "ls"
[[timeline]]
action = "keypress"
key = "enter"
[[timeline]]
action = "focus"
pane = "p"
[[timeline]]
action = "scroll"
direction = "down"
duration_ms = 1000
[[timeline]]
action = "terminate"
"##;

    #[test]
    fn valid_score_has_no_problems() {
        assert!(validate(&score_from(VALID)).is_empty());
    }

    #[test]
    fn flags_pane_outside_canvas() {
        let s = score_from(
            r#"
[demo]
name = "t"
[layout]
width = 100
height = 100
  [[layout.panes]]
  id = "c"
  type = "terminal"
  x = 50
  y = 0
  width = 100
  height = 100
"#,
        );
        assert!(validate(&s).iter().any(|p| p.contains("beyond the")));
    }

    #[test]
    fn flags_browser_without_url() {
        let s = score_from(
            r#"
[demo]
name = "t"
[layout]
width = 100
height = 100
  [[layout.panes]]
  id = "p"
  type = "browser"
  x = 0
  y = 0
  width = 100
  height = 100
"#,
        );
        assert!(validate(&s).iter().any(|p| p.contains("require a `url`")));
    }

    #[test]
    fn flags_input_without_focus() {
        let s = score_from(
            r#"
[demo]
name = "t"
[layout]
width = 100
height = 100
  [[layout.panes]]
  id = "c"
  type = "terminal"
  x = 0
  y = 0
  width = 100
  height = 100
[[timeline]]
action = "type"
text = "ls"
"#,
        );
        assert!(validate(&s).iter().any(|p| p.contains("no focused pane")));
    }

    #[test]
    fn flags_scroll_on_terminal() {
        let s = score_from(
            r#"
[demo]
name = "t"
[layout]
width = 100
height = 100
  [[layout.panes]]
  id = "c"
  type = "terminal"
  x = 0
  y = 0
  width = 100
  height = 100
[[timeline]]
action = "focus"
pane = "c"
[[timeline]]
action = "scroll"
direction = "down"
duration_ms = 1000
"#,
        );
        assert!(validate(&s).iter().any(|p| p.contains("must be a Browser")));
    }

    #[test]
    fn flags_missing_required_env() {
        let s = score_from(
            r#"
[demo]
name = "t"
[env]
requires = ["DEMOSTAGE_DEFINITELY_UNSET_VAR_XYZ"]
[layout]
width = 100
height = 100
  [[layout.panes]]
  id = "c"
  type = "terminal"
  x = 0
  y = 0
  width = 100
  height = 100
"#,
        );
        assert!(validate(&s)
            .iter()
            .any(|p| p.contains("DEMOSTAGE_DEFINITELY_UNSET")));
    }

    #[test]
    fn flags_duplicate_pane_id() {
        let s = score_from(
            r#"
[demo]
name = "t"
[layout]
width = 200
height = 100
  [[layout.panes]]
  id = "c"
  type = "terminal"
  x = 0
  y = 0
  width = 100
  height = 100
  [[layout.panes]]
  id = "c"
  type = "terminal"
  x = 100
  y = 0
  width = 100
  height = 100
"#,
        );
        assert!(validate(&s).iter().any(|p| p.contains("duplicate id")));
    }
}
