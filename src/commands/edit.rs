//! `demo edit` — interactive timeline editor.
//!
//! Shows the full timeline. Mark one or several steps with **space**, press
//! **enter** to apply an action to everything marked (edit, convert waits,
//! delete, …), **esc** when done. Edits are applied immediately so you can see
//! the result and re-edit if needed. Marking a group is how you do bulk work —
//! delete a wizard's leftover steps at once, or turn every `wait` of a section
//! into a `wait_for_quiet`.

use crate::cli::EditArgs;
use crate::commands::edit_reveal;
use crate::error::{Error, Result};
use crate::model::{Score, Step};

pub fn run(args: EditArgs) -> Result<()> {
    let mut score = Score::load(&args.input)?;

    println!("{}\n", crate::BANNER);
    println!(
        "demo edit — {}\n  {} steps · ↑↓ navigate · space=mark (several for a bulk edit) · enter=apply · esc=done\n",
        args.input.display(),
        score.timeline.len()
    );

    let mut cursor: usize = 0;

    while !score.timeline.is_empty() {
        let labels: Vec<String> = score
            .timeline
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{:>3}. {}", i + 1, step_summary(s)))
            .collect();

        let picked = inquire::MultiSelect::new("Timeline:", labels)
            .with_page_size(list_page_size())
            .with_starting_cursor(cursor.min(score.timeline.len().saturating_sub(1)))
            .with_help_message("space marks · enter applies to the marked steps · esc done")
            .prompt_skippable()
            .map_err(|e| Error::Export(format!("edit: {e}")))?;

        // Esc at the list (or enter with nothing marked) → ask if done.
        let indices = picked.map(|sel| selection_indices(&sel, score.timeline.len()));
        let Some(indices) = indices.filter(|idx| !idx.is_empty()) else {
            let done = inquire::Confirm::new("Done editing?")
                .with_default(true)
                .prompt()
                .unwrap_or(true);
            if done {
                break;
            }
            continue;
        };

        cursor = indices[0];
        println!();
        apply_action(&mut score, &indices)?;
        if cursor >= score.timeline.len() && cursor > 0 {
            cursor = score.timeline.len() - 1;
        }
    }

    score.save(&args.input)?;
    println!("✓ saved → {}", args.input.display());
    Ok(())
}

/// Fit the list to the terminal (leave room for the prompt/help lines), so a
/// long timeline shows as many steps as the screen allows instead of 7.
fn list_page_size() -> usize {
    let rows = crossterm::terminal::size().map(|(_, r)| r).unwrap_or(24) as usize;
    rows.saturating_sub(5).clamp(10, 40)
}

/// Map the marked labels back to timeline indices (from the `N.` prefix),
/// sorted and de-duplicated.
fn selection_indices(selected: &[String], len: usize) -> Vec<usize> {
    let mut indices: Vec<usize> = selected
        .iter()
        .filter_map(|s| {
            s.trim_start()
                .split('.')
                .next()
                .and_then(|n| n.trim().parse::<usize>().ok())
                .map(|n| n - 1)
        })
        .filter(|&i| i < len)
        .collect();
    indices.sort_unstable();
    indices.dedup();
    indices
}

/// Ask for one action and apply it to every selected step.
fn apply_action(score: &mut Score, indices: &[usize]) -> Result<()> {
    let all_type = indices
        .iter()
        .all(|&i| matches!(score.timeline[i], Step::Type { .. }));

    let single_browser_focus = indices.len() == 1 && edit_reveal::is_browser_focus(score, indices[0]);

    match ask_action(indices.len(), all_type, single_browser_focus)? {
        None | Some(EditAction::Keep) => {} // Esc = cancel
        Some(EditAction::EditReveal) => {
            edit_reveal::edit_browser_reveal(score, indices[0])?;
        }
        Some(EditAction::WaitForQuiet) => {
            let quiet = ask_u64("quiet_ms", current_ms(&score.timeline[indices[0]]))?;
            for &i in indices {
                score.timeline[i] = Step::WaitForQuiet {
                    quiet_ms: quiet,
                    max_ms: None,
                };
            }
            println!("  ✓ updated {}\n", plural(indices.len()));
        }
        Some(EditAction::WaitForScreen) => {
            let pattern = ask_string("match pattern")?;
            for &i in indices {
                score.timeline[i] = Step::WaitForScreen {
                    pattern: pattern.clone(),
                    timeout_ms: None,
                };
            }
            println!("  ✓ updated {}\n", plural(indices.len()));
        }
        Some(EditAction::WaitForStdout) => {
            let pattern = ask_string("match pattern")?;
            for &i in indices {
                score.timeline[i] = Step::WaitForStdout {
                    pattern: pattern.clone(),
                    pane: None,
                };
            }
            println!("  ✓ updated {}\n", plural(indices.len()));
        }
        Some(EditAction::ChangeDuration) => {
            let ms = ask_u64("duration_ms", current_ms(&score.timeline[indices[0]]))?;
            for &i in indices {
                score.timeline[i] = Step::Wait { duration_ms: ms };
            }
            println!("  ✓ updated {}\n", plural(indices.len()));
        }
        Some(EditAction::SplitType) => {
            split_type_step(&mut score.timeline, indices[0])?;
        }
        Some(EditAction::ReplaceInTypes) => {
            replace_in_types(&mut score.timeline, indices)?;
        }
        Some(EditAction::Delete) => {
            // Back to front so earlier indices stay valid while removing.
            for &i in indices.iter().rev() {
                score.timeline.remove(i);
            }
            println!("  ✓ deleted {}\n", plural(indices.len()));
        }
    }
    Ok(())
}

fn plural(n: usize) -> String {
    if n == 1 {
        "1 step".to_string()
    } else {
        format!("{n} steps")
    }
}

/// Find & replace across several `type` steps at once — the shared treatment
/// that makes sense when editing many texts together.
fn replace_in_types(timeline: &mut [Step], indices: &[usize]) -> Result<()> {
    let find = ask_string("find")?;
    let replace = inquire::Text::new("replace with:")
        .prompt()
        .map_err(|e| Error::Export(format!("edit: {e}")))?;
    let mut changed = 0;
    for &i in indices {
        if let Step::Type { text, human_salt } = &timeline[i] {
            if text.contains(&find) {
                timeline[i] = Step::Type {
                    text: text.replace(&find, &replace),
                    human_salt: *human_salt,
                };
                changed += 1;
            }
        }
    }
    println!("  ✓ replaced in {}\n", plural(changed));
    Ok(())
}

fn current_ms(step: &Step) -> u64 {
    match step {
        Step::Wait { duration_ms } => *duration_ms,
        Step::WaitForQuiet { quiet_ms, .. } => *quiet_ms,
        _ => 500,
    }
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
        Step::Focus { pane } => {
            let id = pane.as_deref().unwrap_or("?");
            format!("focus → {id} (reveal)")
        }
        Step::Caption { text } => format!("caption {:?}", text),
        Step::Secret { prompt } => format!("secret {:?}", prompt),
        Step::Scroll {
            direction,
            duration_ms,
            ..
        } => {
            format!("scroll {direction:?} {duration_ms}ms")
        }
        Step::Terminate => "terminate".to_string(),
    }
}

enum EditAction {
    Keep,
    EditReveal,
    WaitForQuiet,
    WaitForScreen,
    WaitForStdout,
    ChangeDuration,
    SplitType,
    ReplaceInTypes,
    Delete,
}

/// The action menu for `n` marked steps. Every action applies to all of them;
/// text editing appears when the whole selection is `type` steps (in-place
/// split/edit for one, find & replace across several).
fn ask_action(
    n: usize,
    all_type: bool,
    single_browser_focus: bool,
) -> Result<Option<EditAction>> {
    let mut opts = vec![
        "Keep as-is",
        "→ wait_for_quiet (silence-based)",
        "→ wait_for_screen (VT pattern)",
        "→ wait_for_stdout (raw output pattern)",
        "Change duration",
        "Delete",
    ];

    if single_browser_focus {
        opts.insert(1, "Edit reveal (placement, scroll, file/URL)");
    }

    if all_type {
        opts.push(if n == 1 {
            "Split/Edit text"
        } else {
            "Find & replace in texts"
        });
    }

    let prompt = if n == 1 {
        "Action:".to_string()
    } else {
        format!("Action for the {n} marked steps:")
    };
    let choice = inquire::Select::new(&prompt, opts)
        .prompt_skippable()
        .map_err(|e| Error::Export(format!("edit: {e}")))?;

    let Some(choice) = choice else {
        return Ok(None); // Esc = cancel
    };

    Ok(Some(match choice {
        "Keep as-is" => EditAction::Keep,
        "Edit reveal (placement, scroll, file/URL)" => EditAction::EditReveal,
        "→ wait_for_quiet (silence-based)" => EditAction::WaitForQuiet,
        "→ wait_for_screen (VT pattern)" => EditAction::WaitForScreen,
        "→ wait_for_stdout (raw output pattern)" => EditAction::WaitForStdout,
        "Change duration" => EditAction::ChangeDuration,
        "Split/Edit text" => EditAction::SplitType,
        "Find & replace in texts" => EditAction::ReplaceInTypes,
        "Delete" => EditAction::Delete,
        _ => EditAction::Keep,
    }))
}

fn split_type_step(timeline: &mut Vec<Step>, idx: usize) -> Result<()> {
    let (text, human_salt) = match &timeline[idx] {
        Step::Type { text, human_salt } => (text.clone(), *human_salt),
        _ => return Ok(()),
    };

    let display_text = text.replace('\n', "↵");
    println!("  current: {:?}", display_text);

    let action = inquire::Select::new("How:", vec!["Edit text (replace)", "Split by delimiter"])
        .prompt()
        .map_err(|e| Error::Export(format!("edit: {e}")))?;

    if action.starts_with("Edit") {
        let new = inquire::Text::new("new text:")
            .with_default(&text)
            .prompt()
            .map_err(|e| Error::Export(format!("edit: {e}")))?;
        let new = new.replace("↵", "\n");
        timeline[idx] = Step::Type {
            text: new,
            human_salt,
        };
        println!("  ✓ updated\n");
    } else {
        let delim = inquire::Select::new("Split on:", vec!["newline", "space", "custom"])
            .prompt()
            .map_err(|e| Error::Export(format!("edit: {e}")))?;

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

fn do_split(
    timeline: &mut Vec<Step>,
    idx: usize,
    delim: &str,
    human_salt: bool,
    text: &str,
) -> Result<()> {
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
            new_steps.push(Step::Type {
                text: t,
                human_salt,
            });
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
        .map_err(|e| Error::Export(format!("edit: {e}")))?;
    v.trim()
        .parse()
        .map_err(|_| Error::Export(format!("invalid number: {v}")))
}

fn ask_string(label: &str) -> Result<String> {
    let v = inquire::Text::new(&format!("{label}:"))
        .prompt()
        .map_err(|e| Error::Export(format!("edit: {e}")))?;
    let v = v.trim().to_string();
    if v.is_empty() {
        return Err(Error::Export("cannot be empty".to_string()));
    }
    Ok(v)
}
