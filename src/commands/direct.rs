//! `demo direct` — interactive wizard for editing a demo score's timeline.
//!
//! Shows the full timeline as a multi-select list. The user picks which steps to
//! edit, then edits them one by one with full context.

use crate::cli::DirectArgs;
use crate::error::{Error, Result};
use crate::model::{Score, Step};

pub fn run(args: DirectArgs) -> Result<()> {
    let mut score = Score::load(&args.input)?;

    println!("{}\n", crate::BANNER);
    println!(
        "demo direct — {}\n  {} steps in timeline\n",
        args.input.display(),
        score.timeline.len()
    );

    // Show the full timeline and let the user multi-select which to edit.
    let labels: Vec<String> = score
        .timeline
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{:>3}. {}", i + 1, step_summary(s)))
        .collect();

    let selected = inquire::MultiSelect::new("Select steps to edit:", labels.clone())
        .with_help_message("↑↓ move, space toggle, enter confirm")
        .prompt()
        .map_err(|e| Error::Export(format!("direct wizard: {e}")))?;

    if selected.is_empty() {
        println!("no steps selected.");
        return Ok(());
    }

    // Map selected labels back to indices.
    let indices: Vec<usize> = selected
        .iter()
        .filter_map(|sel| labels.iter().position(|l| l == sel))
        .collect();

    // Edit each selected step (in order, adjusting for deletions).
    let mut offset: i32 = 0;
    for &orig_idx in &indices {
        let idx = (orig_idx as i32 + offset) as usize;
        if idx >= score.timeline.len() {
            break;
        }

        print_context(&score.timeline, idx, 3);

        match ask_action(&score.timeline[idx])? {
            EditAction::Keep => {}
            EditAction::WaitForQuiet => {
                let quiet = ask_u64("quiet_ms", current_ms(&score.timeline[idx]))?;
                score.timeline[idx] = Step::WaitForQuiet {
                    quiet_ms: quiet,
                    max_ms: None,
                };
            }
            EditAction::WaitForScreen => {
                let pattern = ask_string("match pattern")?;
                score.timeline[idx] = Step::WaitForScreen {
                    pattern,
                    timeout_ms: None,
                };
            }
            EditAction::WaitForStdout => {
                let pattern = ask_string("match pattern")?;
                score.timeline[idx] = Step::WaitForStdout {
                    pattern,
                    pane: None,
                };
            }
            EditAction::ChangeDuration => {
                let ms = ask_u64("duration_ms", current_ms(&score.timeline[idx]))?;
                score.timeline[idx] = Step::Wait { duration_ms: ms };
            }
            EditAction::EditUrl => {
                if let Some(new) = edit_open_step(&score, idx)? {
                    score.timeline[idx] = new;
                }
            }
            EditAction::SplitType => {
                let extra = split_type_step(&mut score.timeline, idx)?;
                offset += extra;
            }
            EditAction::Delete => {
                score.timeline.remove(idx);
                offset -= 1;
                println!("  ✓ deleted\n");
            }
        }
    }

    score.save(&args.input)?;
    println!("✓ saved → {}", args.input.display());
    Ok(())
}

fn current_ms(step: &Step) -> u64 {
    match step {
        Step::Wait { duration_ms } => *duration_ms,
        Step::WaitForQuiet { quiet_ms, .. } => *quiet_ms,
        _ => 500,
    }
}

/// Print context: up to `before` lines above and 1 line below the current step.
fn print_context(timeline: &[Step], idx: usize, before: usize) {
    let total = timeline.len();
    println!("\n─── Step {}/{total} ───", idx + 1);
    let start = idx.saturating_sub(before);
    for step in &timeline[start..idx] {
        println!("  │ {}", step_summary(step));
    }
    println!("  ▶ {}", step_summary(&timeline[idx]));
    if idx + 1 < total {
        println!("  │ {}", step_summary(&timeline[idx + 1]));
    }
    println!();
}

fn step_summary(step: &Step) -> String {
    match step {
        Step::Type { text, .. } => {
            let preview: String = text.chars().take(60).collect();
            let preview = preview.replace('\n', "↵");
            format!("type {:?}", preview)
        }
        Step::Keypress { key } => format!("keypress {key}"),
        Step::Wait { duration_ms } => format!("wait {duration_ms}ms"),
        Step::WaitForQuiet { quiet_ms, .. } => format!("wait_for_quiet {quiet_ms}ms"),
        Step::WaitForScreen { pattern, .. } => format!("wait_for_screen {:?}", pattern),
        Step::WaitForStdout { pattern, .. } => format!("wait_for_stdout {:?}", pattern),
        Step::Focus { pane } => format!("focus → {pane}"),
        Step::Caption { text } => format!("caption {:?}", text),
        Step::Secret { prompt } => format!("secret {:?}", prompt),
        Step::Scroll { direction, duration_ms, .. } => {
            format!("scroll {direction:?} {duration_ms}ms")
        }
        Step::Terminate => "terminate".to_string(),
    }
}

enum EditAction {
    Keep,
    WaitForQuiet,
    WaitForScreen,
    WaitForStdout,
    ChangeDuration,
    EditUrl,
    SplitType,
    Delete,
}

fn ask_action(step: &Step) -> Result<EditAction> {
    let mut opts = vec![
        "Keep as-is",
        "→ wait_for_quiet (silence-based)",
        "→ wait_for_screen (VT pattern)",
        "→ wait_for_stdout (raw output pattern)",
        "Change duration",
        "Delete step",
    ];

    // Add URL edit option for Focus steps pointing to browser panes.
    let is_focus = matches!(step, Step::Focus { .. });
    let is_wait_scene = matches!(step, Step::Wait { duration_ms } if *duration_ms >= 2000);
    if is_focus || is_wait_scene {
        opts.push("Edit scene (URL/hold)");
    }

    // Add split option for Type steps.
    if matches!(step, Step::Type { .. }) {
        opts.push("Split/Edit text");
    }

    let choice = inquire::Select::new("Action:", opts)
        .prompt()
        .map_err(|e| Error::Export(format!("direct wizard: {e}")))?;

    Ok(match choice {
        "Keep as-is" => EditAction::Keep,
        "→ wait_for_quiet (silence-based)" => EditAction::WaitForQuiet,
        "→ wait_for_screen (VT pattern)" => EditAction::WaitForScreen,
        "→ wait_for_stdout (raw output pattern)" => EditAction::WaitForStdout,
        "Change duration" => EditAction::ChangeDuration,
        "Edit scene (URL/hold)" => EditAction::EditUrl,
        "Split/Edit text" => EditAction::SplitType,
        "Delete step" => EditAction::Delete,
        _ => EditAction::Keep,
    })
}

/// Edit a browser pane's URL or hold time. Finds the pane in the layout and edits it.
fn edit_open_step(score: &Score, idx: usize) -> Result<Option<Step>> {
    let step = &score.timeline[idx];
    match step {
        Step::Focus { pane } => {
            // Find the pane in layout and offer to change URL.
            if let Some(p) = score.layout.panes.iter().find(|p| &p.id == pane) {
                if let Some(url) = &p.url {
                    println!("  current URL: {url}");
                    let new_url = ask_string_with_default("new URL", url)?;
                    // We can't modify the score layout from here without returning it.
                    // Instead, inform the user this needs a manual edit.
                    println!("  ℹ update the URL in demo.toml [layout.panes] → id=\"{pane}\"");
                    println!("    url = {:?}", new_url);
                    return Ok(None);
                }
            }
            Ok(None)
        }
        Step::Wait { duration_ms } => {
            let new = ask_u64("hold_ms", *duration_ms)?;
            Ok(Some(Step::Wait { duration_ms: new }))
        }
        _ => Ok(None),
    }
}

fn ask_u64(label: &str, default: u64) -> Result<u64> {
    let v = inquire::Text::new(&format!("{label}:"))
        .with_default(&default.to_string())
        .prompt()
        .map_err(|e| Error::Export(format!("direct wizard: {e}")))?;
    v.trim()
        .parse()
        .map_err(|_| Error::Export(format!("invalid number: {v}")))
}

fn ask_string(label: &str) -> Result<String> {
    let v = inquire::Text::new(&format!("{label}:"))
        .prompt()
        .map_err(|e| Error::Export(format!("direct wizard: {e}")))?;
    let v = v.trim().to_string();
    if v.is_empty() {
        return Err(Error::Export("pattern cannot be empty".to_string()));
    }
    Ok(v)
}

fn ask_string_with_default(label: &str, default: &str) -> Result<String> {
    let v = inquire::Text::new(&format!("{label}:"))
        .with_default(default)
        .prompt()
        .map_err(|e| Error::Export(format!("direct wizard: {e}")))?;
    let v = v.trim().to_string();
    if v.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(v)
    }
}

/// Split or edit a Type step's text. Shows the current text and lets the user
/// either edit it in place or split it into multiple Type steps (separated by a
/// delimiter they choose, e.g. newline or a custom string).
/// Returns the number of extra steps inserted (offset adjustment).
fn split_type_step(timeline: &mut Vec<Step>, idx: usize) -> Result<i32> {
    let (text, human_salt) = match &timeline[idx] {
        Step::Type { text, human_salt } => (text.clone(), *human_salt),
        _ => return Ok(0),
    };

    let display_text = text.replace('\n', "↵");
    println!("  current text: {:?}", display_text);

    let action = inquire::Select::new(
        "How to edit:",
        vec![
            "Edit text (replace)",
            "Split into parts (by delimiter)",
        ],
    )
    .prompt()
    .map_err(|e| Error::Export(format!("direct wizard: {e}")))?;

    if action.starts_with("Edit") {
        let new = inquire::Text::new("new text:")
            .with_default(&text)
            .prompt()
            .map_err(|e| Error::Export(format!("direct wizard: {e}")))?;
        // Convert ↵ back to \n for convenience.
        let new = new.replace("↵", "\n");
        timeline[idx] = Step::Type {
            text: new,
            human_salt,
        };
        println!("  ✓ updated\n");
        Ok(0)
    } else {
        // Split by delimiter.
        let delim = inquire::Select::new(
            "Split on:",
            vec!["newline (\\n)", "space", "custom string"],
        )
        .prompt()
        .map_err(|e| Error::Export(format!("direct wizard: {e}")))?;

        let delim_str = match delim {
            "newline (\\n)" => "\n",
            "space" => " ",
            _ => {
                let d = ask_string("delimiter")?;
                // Leak into a 'static str for the split — fine, it's short-lived.
                return split_with_delim(timeline, idx, &d, human_salt, &text);
            }
        };
        split_with_delim(timeline, idx, delim_str, human_salt, &text)
    }
}

fn split_with_delim(
    timeline: &mut Vec<Step>,
    idx: usize,
    delim: &str,
    human_salt: bool,
    text: &str,
) -> Result<i32> {
    let parts: Vec<&str> = text.split(delim).collect();
    if parts.len() <= 1 {
        println!("  (delimiter not found — no split performed)");
        return Ok(0);
    }

    // Replace original with first part, insert rest after.
    let mut new_steps: Vec<Step> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let mut t = part.to_string();
        // Re-add the delimiter between parts (except the last).
        if i < parts.len() - 1 {
            t.push_str(delim);
        }
        if !t.is_empty() {
            new_steps.push(Step::Type {
                text: t,
                human_salt,
            });
        }
    }

    let extra = new_steps.len() as i32 - 1;
    timeline.splice(idx..=idx, new_steps);
    println!("  ✓ split into {} parts\n", extra + 1);
    Ok(extra)
}
