//! `demo direct` — interactive timeline editor.
//!
//! Shows the full timeline. Navigate with arrows, press **space** to edit the
//! current step inline, press **enter** when done. Edits are applied immediately
//! so you can see the result and re-edit if needed.

use crate::cli::DirectArgs;
use crate::error::{Error, Result};
use crate::model::{Score, Step};

pub fn run(args: DirectArgs) -> Result<()> {
    let mut score = Score::load(&args.input)?;

    println!("{}\n", crate::BANNER);
    println!(
        "demo direct — {}\n  {} steps · navigate ↑↓ · space=edit · enter=done\n",
        args.input.display(),
        score.timeline.len()
    );

    // Show the timeline and let user pick one at a time to edit (loop until Esc).
    let mut cursor: usize = 0;
    loop {
        let labels: Vec<String> = score
            .timeline
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{:>3}. {}", i + 1, step_summary(s)))
            .collect();

        let selection = inquire::Select::new("Timeline (enter=done):", labels)
            .with_starting_cursor(cursor.min(score.timeline.len().saturating_sub(1)))
            .prompt_skippable()
            .map_err(|e| Error::Export(format!("direct: {e}")))?;

        let Some(selected) = selection else {
            break; // enter with no selection or Esc → done
        };

        // Find the index from the label prefix.
        let idx = selected
            .trim_start()
            .split('.')
            .next()
            .and_then(|n| n.trim().parse::<usize>().ok())
            .map(|n| n - 1)
            .unwrap_or(0);

        if idx >= score.timeline.len() {
            continue;
        }

        cursor = idx;
        print_context(&score.timeline, idx);

        match ask_action(&score.timeline[idx])? {
            EditAction::Keep => {}
            EditAction::WaitForQuiet => {
                let quiet = ask_u64("quiet_ms", current_ms(&score.timeline[idx]))?;
                score.timeline[idx] = Step::WaitForQuiet {
                    quiet_ms: quiet,
                    max_ms: None,
                };
                println!("  ✓ updated\n");
            }
            EditAction::WaitForScreen => {
                let pattern = ask_string("match pattern")?;
                score.timeline[idx] = Step::WaitForScreen {
                    pattern,
                    timeout_ms: None,
                };
                println!("  ✓ updated\n");
            }
            EditAction::WaitForStdout => {
                let pattern = ask_string("match pattern")?;
                score.timeline[idx] = Step::WaitForStdout {
                    pattern,
                    pane: None,
                };
                println!("  ✓ updated\n");
            }
            EditAction::ChangeDuration => {
                let ms = ask_u64("duration_ms", current_ms(&score.timeline[idx]))?;
                score.timeline[idx] = Step::Wait { duration_ms: ms };
                println!("  ✓ updated\n");
            }
            EditAction::SplitType => {
                split_type_step(&mut score.timeline, idx)?;
            }
            EditAction::Delete => {
                score.timeline.remove(idx);
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

fn print_context(timeline: &[Step], idx: usize) {
    let total = timeline.len();
    println!("\n─── Step {}/{total} ───", idx + 1);
    let start = idx.saturating_sub(3);
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

    if matches!(step, Step::Type { .. }) {
        opts.push("Split/Edit text");
    }

    let choice = inquire::Select::new("Action:", opts)
        .prompt()
        .map_err(|e| Error::Export(format!("direct: {e}")))?;

    Ok(match choice {
        "Keep as-is" => EditAction::Keep,
        "→ wait_for_quiet (silence-based)" => EditAction::WaitForQuiet,
        "→ wait_for_screen (VT pattern)" => EditAction::WaitForScreen,
        "→ wait_for_stdout (raw output pattern)" => EditAction::WaitForStdout,
        "Change duration" => EditAction::ChangeDuration,
        "Split/Edit text" => EditAction::SplitType,
        "Delete step" => EditAction::Delete,
        _ => EditAction::Keep,
    })
}

fn split_type_step(timeline: &mut Vec<Step>, idx: usize) -> Result<()> {
    let (text, human_salt) = match &timeline[idx] {
        Step::Type { text, human_salt } => (text.clone(), *human_salt),
        _ => return Ok(()),
    };

    let display_text = text.replace('\n', "↵");
    println!("  current: {:?}", display_text);

    let action = inquire::Select::new(
        "How:",
        vec!["Edit text (replace)", "Split by delimiter"],
    )
    .prompt()
    .map_err(|e| Error::Export(format!("direct: {e}")))?;

    if action.starts_with("Edit") {
        let new = inquire::Text::new("new text:")
            .with_default(&text)
            .prompt()
            .map_err(|e| Error::Export(format!("direct: {e}")))?;
        let new = new.replace("↵", "\n");
        timeline[idx] = Step::Type {
            text: new,
            human_salt,
        };
        println!("  ✓ updated\n");
    } else {
        let delim = inquire::Select::new("Split on:", vec!["newline", "space", "custom"])
            .prompt()
            .map_err(|e| Error::Export(format!("direct: {e}")))?;

        let delim_str = match delim {
            "newline" => "\n",
            "space" => " ",
            _ => {
                let d = ask_string("delimiter")?;
                return do_split(timeline, idx, &d, human_salt, &text);
            }
        };
        do_split(timeline, idx, delim_str, human_salt, &text)?;
    }
    Ok(())
}

fn do_split(timeline: &mut Vec<Step>, idx: usize, delim: &str, human_salt: bool, text: &str) -> Result<()> {
    let parts: Vec<&str> = text.split(delim).collect();
    if parts.len() <= 1 {
        println!("  (delimiter not found — no split)");
        return Ok(());
    }
    let mut new_steps: Vec<Step> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let mut t = part.to_string();
        if i < parts.len() - 1 {
            t.push_str(delim);
        }
        if !t.is_empty() {
            new_steps.push(Step::Type { text: t, human_salt });
        }
    }
    let n = new_steps.len();
    timeline.splice(idx..=idx, new_steps);
    println!("  ✓ split into {n} parts\n");
    Ok(())
}

fn ask_u64(label: &str, default: u64) -> Result<u64> {
    let v = inquire::Text::new(&format!("{label}:"))
        .with_default(&default.to_string())
        .prompt()
        .map_err(|e| Error::Export(format!("direct: {e}")))?;
    v.trim()
        .parse()
        .map_err(|_| Error::Export(format!("invalid number: {v}")))
}

fn ask_string(label: &str) -> Result<String> {
    let v = inquire::Text::new(&format!("{label}:"))
        .prompt()
        .map_err(|e| Error::Export(format!("direct: {e}")))?;
    let v = v.trim().to_string();
    if v.is_empty() {
        return Err(Error::Export("cannot be empty".to_string()));
    }
    Ok(v)
}
