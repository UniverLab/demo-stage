//! `demo focus` — switch focus to a scene during capture.
//!
//! Adds a `Step::Focus` entry to the timeline. When a scene is specified,
//! it resolves the scene's layout to pane positions at export time. Supports
//! deferred triggers (pattern match, command finish, timer).

use std::io::IsTerminal;
use std::path::Path;

use inquire::{Select, Text};

use crate::cli::FocusArgs;
use crate::error::{Error, Result};
use crate::model::{Score, Step};

pub fn run(args: FocusArgs) -> Result<()> {
    let score_path = &args.score;

    let (scene, trigger) = if !std::io::stdin().is_terminal() || args.scene.is_some() {
        resolve_from_args(&args)?
    } else {
        wizard(score_path)?
    };

    let mut score = load_score(score_path)?;

    // Build the Step::Focus with trigger info
    let step = build_focus_step(&scene, &trigger, &score)?;
    score.timeline.push(step);
    save_score(score_path, &score)?;

    println!(
        "✓ Focus on '{scene}' added to timeline in {}",
        score_path.display()
    );
    Ok(())
}

fn load_score(path: &Path) -> Result<Score> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::Export(format!("cannot read {}: {e}", path.display())))?;
    toml::from_str(&content)
        .map_err(|e| Error::Export(format!("invalid score {}: {e}", path.display())))
}

fn save_score(path: &Path, score: &Score) -> Result<()> {
    let content = toml::to_string_pretty(score)
        .map_err(|e| Error::Export(format!("serialize score: {e}")))?;
    std::fs::write(path, content)
        .map_err(|e| Error::Export(format!("write {}: {e}", path.display())))
}

fn resolve_from_args(args: &FocusArgs) -> Result<(String, Trigger)> {
    let scene = args
        .scene
        .clone()
        .ok_or_else(|| Error::Export("scene ID is required (or run interactively)".to_string()))?;

    let trigger = if let Some(pat) = &args.when {
        Trigger::When(pat.clone())
    } else if args.after {
        Trigger::After
    } else if let Some(ms) = args.after_ms {
        Trigger::AfterMs(ms)
    } else {
        Trigger::Now
    };

    Ok((scene, trigger))
}

enum Trigger {
    Now,
    When(String),
    After,
    AfterMs(u64),
}

fn ask<T>(r: std::result::Result<T, inquire::InquireError>) -> Result<T> {
    r.map_err(|e| Error::Export(format!("wizard: {e}")))
}

fn wizard(score_path: &Path) -> Result<(String, Trigger)> {
    println!("\n  demo focus — switch to a scene\n");

    let score = load_score(score_path)?;
    let scene_ids = score.scene_ids();

    if scene_ids.is_empty() {
        println!("  ℹ  No scenes defined yet. Define scenes first with `demo scene`.");
        println!("     You can still enter a scene ID manually.\n");
    } else {
        println!("  Available scenes: {}\n", scene_ids.join(", "));
    }

    let scene = ask(Text::new("Scene ID:")
        .with_help_message("which scene to focus")
        .prompt())?;
    let scene = scene.trim().to_string();
    if scene.is_empty() {
        return Err(Error::Export("scene ID cannot be empty".to_string()));
    }

    let trigger = ask(Select::new(
        "When:",
        vec![
            "now — focus immediately",
            "when a line appears in the output",
            "when the current command finishes",
            "after a delay (milliseconds)",
        ],
    )
    .prompt())?;

    let t = if trigger.starts_with("when a line") {
        let pat = ask(Text::new("Cue line (substring of terminal output):").prompt())?;
        let pat = pat.trim().to_string();
        if pat.is_empty() {
            Trigger::Now
        } else {
            Trigger::When(pat)
        }
    } else if trigger.starts_with("when the current") {
        Trigger::After
    } else if trigger.starts_with("after a delay") {
        let ms = ask(Text::new("Delay in milliseconds:")
            .with_default("0")
            .with_validator(|s: &str| match s.trim().parse::<u64>() {
                Ok(_) => Ok(inquire::validator::Validation::Valid),
                Err(_) => Ok(inquire::validator::Validation::Invalid(
                    "enter a number in milliseconds".into(),
                )),
            })
            .prompt())?;
        let ms: u64 = ms.trim().parse().unwrap_or(0);
        if ms == 0 {
            Trigger::Now
        } else {
            Trigger::AfterMs(ms)
        }
    } else {
        Trigger::Now
    };

    Ok((scene, t))
}

fn build_focus_step(scene: &str, trigger: &Trigger, score: &Score) -> Result<Step> {
    // Validate the scene exists
    if score.scene(scene).is_none() {
        return Err(Error::Export(format!(
            "unknown scene '{scene}' — define it with `demo scene` first"
        )));
    }

    // For now, all triggers produce a simple Step::Focus with scene set.
    // The trigger semantics (when/after/delay) will be implemented in Paso 5
    // when we update capture to handle deferred focus steps.
    // For immediate focus, we just add the step.
    match trigger {
        Trigger::Now => Ok(Step::Focus {
            pane: None,
            scene: Some(scene.to_string()),
        }),
        Trigger::When(pat) => {
            // TODO: store pattern in Step for deferred focus
            // For now, emit a warning and add as immediate
            eprintln!("⚠ deferred trigger 'when {pat}' — currently fires immediately");
            Ok(Step::Focus {
                pane: None,
                scene: Some(scene.to_string()),
            })
        }
        Trigger::After => {
            eprintln!("⚠ deferred trigger 'after' — currently fires immediately");
            Ok(Step::Focus {
                pane: None,
                scene: Some(scene.to_string()),
            })
        }
        Trigger::AfterMs(ms) => {
            eprintln!("⚠ deferred trigger 'after {ms}ms' — currently fires immediately");
            Ok(Step::Focus {
                pane: None,
                scene: Some(scene.to_string()),
            })
        }
    }
}
