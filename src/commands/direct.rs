//! `demo direct` — interactive wizard for editing a demo score's timeline.
//!
//! Walks through each step showing context (the step before and after) and
//! offers quick actions: keep, replace with a different wait strategy, change
//! duration, delete, or insert a new step.

use crate::cli::DirectArgs;
use crate::error::{Error, Result};
use crate::model::{Score, Step};

pub fn run(args: DirectArgs) -> Result<()> {
    let mut score = Score::load(&args.input)?;
    let original_len = score.timeline.len();

    println!(
        "demo direct — editing {}\n  {} steps in timeline\n",
        args.input.display(),
        original_len
    );

    let mut i = 0;
    while i < score.timeline.len() {
        let step = &score.timeline[i];
        // Only offer editing for timing/wait steps — skip structural ones.
        if !is_editable(step) {
            i += 1;
            continue;
        }

        print_context(&score.timeline, i);

        match ask_action()? {
            EditAction::Keep => {}
            EditAction::WaitForQuiet => {
                let quiet = ask_u64("quiet_ms", 500)?;
                score.timeline[i] = Step::WaitForQuiet {
                    quiet_ms: quiet,
                    max_ms: None,
                };
            }
            EditAction::WaitForScreen => {
                let pattern = ask_string("match pattern")?;
                score.timeline[i] = Step::WaitForScreen {
                    pattern,
                    timeout_ms: None,
                };
            }
            EditAction::WaitForStdout => {
                let pattern = ask_string("match pattern")?;
                score.timeline[i] = Step::WaitForStdout {
                    pattern,
                    pane: None,
                };
            }
            EditAction::ChangeDuration => {
                let ms = ask_u64("duration_ms", current_duration(step))?;
                score.timeline[i] = Step::Wait { duration_ms: ms };
            }
            EditAction::Delete => {
                score.timeline.remove(i);
                println!("  ✓ deleted\n");
                continue; // don't increment i
            }
        }
        i += 1;
    }

    let changed = score.timeline.len() != original_len
        || score.to_toml()? != Score::load(&args.input)?.to_toml()?;

    if changed {
        score.save(&args.input)?;
        println!("✓ saved → {}", args.input.display());
    } else {
        println!("no changes made.");
    }
    Ok(())
}

fn is_editable(step: &Step) -> bool {
    matches!(
        step,
        Step::Wait { .. } | Step::WaitForQuiet { .. } | Step::WaitForScreen { .. }
    )
}

fn current_duration(step: &Step) -> u64 {
    match step {
        Step::Wait { duration_ms } => *duration_ms,
        Step::WaitForQuiet { quiet_ms, .. } => *quiet_ms,
        _ => 500,
    }
}

fn print_context(timeline: &[Step], idx: usize) {
    let total = timeline.len();
    println!("─── Step {}/{total} ───", idx + 1);
    if idx > 0 {
        println!("  ← {}", step_summary(&timeline[idx - 1]));
    }
    println!("  ▶ {}", step_summary(&timeline[idx]));
    if idx + 1 < total {
        println!("  → {}", step_summary(&timeline[idx + 1]));
    }
    println!();
}

fn step_summary(step: &Step) -> String {
    match step {
        Step::Type { text, .. } => {
            let preview = text.chars().take(50).collect::<String>();
            format!("type {:?}", preview)
        }
        Step::Keypress { key } => format!("keypress {key}"),
        Step::Wait { duration_ms } => format!("wait {duration_ms}ms"),
        Step::WaitForQuiet { quiet_ms, .. } => format!("wait_for_quiet {quiet_ms}ms"),
        Step::WaitForScreen { pattern, .. } => format!("wait_for_screen {:?}", pattern),
        Step::WaitForStdout { pattern, .. } => format!("wait_for_stdout {:?}", pattern),
        Step::Focus { pane } => format!("focus {pane}"),
        Step::Caption { text } => format!("caption {:?}", text),
        Step::Secret { prompt } => format!("secret {:?}", prompt),
        Step::Scroll { direction, .. } => format!("scroll {direction:?}"),
        Step::Terminate => "terminate".to_string(),
    }
}

enum EditAction {
    Keep,
    WaitForQuiet,
    WaitForScreen,
    WaitForStdout,
    ChangeDuration,
    Delete,
}

fn ask_action() -> Result<EditAction> {
    let choice = inquire::Select::new(
        "Action:",
        vec![
            "Keep as-is",
            "→ wait_for_quiet (silence-based)",
            "→ wait_for_screen (VT pattern)",
            "→ wait_for_stdout (raw output pattern)",
            "Change duration",
            "Delete step",
        ],
    )
    .prompt()
    .map_err(|e| Error::Export(format!("direct wizard: {e}")))?;

    Ok(match choice {
        "Keep as-is" => EditAction::Keep,
        "→ wait_for_quiet (silence-based)" => EditAction::WaitForQuiet,
        "→ wait_for_screen (VT pattern)" => EditAction::WaitForScreen,
        "→ wait_for_stdout (raw output pattern)" => EditAction::WaitForStdout,
        "Change duration" => EditAction::ChangeDuration,
        "Delete step" => EditAction::Delete,
        _ => EditAction::Keep,
    })
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
