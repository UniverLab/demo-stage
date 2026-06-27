//! `demo scene` — define a scene composition from pre-defined sources.
//!
//! Scenes map layout strings (e.g. "main+google") to concrete compositions.
//! Run this before `demo capture` to pre-define your scenes.

use std::io::IsTerminal;
use std::path::Path;

use inquire::{validator::Validation, Text};

use crate::cli::SceneArgs;
use crate::error::{Error, Result};
use crate::model::{Scene, Score};

pub fn run(args: SceneArgs) -> Result<()> {
    let score_path = &args.score;

    // --list: show existing scenes
    if args.list {
        let score = load_score(score_path)?;
        if score.scenes.is_empty() {
            println!("No scenes defined. Run `demo scene` to add one.");
        } else {
            println!("Scenes in {}:\n", score_path.display());
            for s in &score.scenes {
                println!("  {:16} layout: {}", s.id, s.layout);
            }
        }
        return Ok(());
    }

    // --remove: remove a scene
    if let Some(id) = &args.remove {
        let mut score = load_score(score_path)?;
        let before = score.scenes.len();
        score.scenes.retain(|s| &s.id != id);
        if score.scenes.len() == before {
            return Err(Error::Export(format!("scene '{id}' not found")));
        }
        save_score(score_path, &score)?;
        println!("Removed scene '{id}'.");
        return Ok(());
    }

    // Add or update a scene
    let scene = if !std::io::stdin().is_terminal() || args.id.is_some() {
        resolve_from_args(&args)?
    } else {
        wizard(score_path)?
    };

    let mut score = load_score(score_path)?;
    // Replace existing scene with the same ID
    score.scenes.retain(|s| s.id != scene.id);
    score.scenes.push(scene.clone());
    save_score(score_path, &score)?;

    println!("✓ Scene '{}' added to {}", scene.id, score_path.display());
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

fn resolve_from_args(args: &SceneArgs) -> Result<Scene> {
    let id = args
        .id
        .clone()
        .ok_or_else(|| Error::Export("scene ID is required (or run interactively)".to_string()))?;
    let layout = args
        .layout
        .clone()
        .ok_or_else(|| Error::Export("layout string is required: --layout <LAYOUT>".to_string()))?;
    validate_layout(&layout)?;
    Ok(Scene { id, layout })
}

fn ask<T>(r: std::result::Result<T, inquire::InquireError>) -> Result<T> {
    r.map_err(|e| Error::Export(format!("wizard: {e}")))
}

fn wizard(score_path: &Path) -> Result<Scene> {
    println!("\n  demo scene — define a scene composition\n");

    let score = load_score(score_path)?;
    let source_ids: Vec<&str> = score.sources.iter().map(|s| s.id.as_str()).collect();

    if source_ids.is_empty() {
        println!("  ℹ  No sources defined yet. Define sources first with `demo source`.");
        println!("     You can still enter a layout string manually.\n");
    } else {
        println!("  Available sources: {}\n", source_ids.join(", "));
    }

    let id = ask(Text::new("Scene ID:")
        .with_help_message("unique identifier (e.g. 'solo', 'split', 'full_github')")
        .with_default("solo")
        .prompt())?;
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(Error::Export("scene ID cannot be empty".to_string()));
    }

    let layout: String = ask(Text::new("Layout string:")
        .with_help_message(
            "\"main\" = fullscreen, \"main+google\" = 50/50, \"main*2+google\" = weighted",
        )
        .with_default(if source_ids.is_empty() {
            "main"
        } else {
            source_ids[0]
        })
        .with_validator(|s: &str| {
            let s = s.trim();
            if s.is_empty() {
                Ok(Validation::Invalid("layout cannot be empty".into()))
            } else if !s.contains('+')
                && s.split('*').next().map(|s| !s.is_empty()).unwrap_or(false)
            {
                // Single source, no +, no weight — valid
                Ok(Validation::Valid)
            } else if s.contains('+') || s.contains('*') {
                // Has + or * — validate parts
                let parts: Vec<&str> = s.split('+').collect();
                for part in &parts {
                    let token = part.trim();
                    if token.is_empty() {
                        return Ok(Validation::Invalid("empty segment in layout".into()));
                    }
                    // token might be "id" or "id*N"
                    let name = if let Some(idx) = token.find('*') {
                        token[..idx].trim()
                    } else {
                        token
                    };
                    if name.is_empty() {
                        return Ok(Validation::Invalid("empty source name in layout".into()));
                    }
                }
                Ok(Validation::Valid)
            } else {
                // Single word, no + or * — valid
                Ok(Validation::Valid)
            }
        })
        .prompt())?;
    let layout = layout.trim().to_string();

    Ok(Scene { id, layout })
}

fn validate_layout(layout: &str) -> Result<()> {
    if layout.is_empty() {
        return Err(Error::Export("layout string cannot be empty".to_string()));
    }
    for part in layout.split('+') {
        let token = part.trim();
        if token.is_empty() {
            return Err(Error::Export("empty segment in layout".to_string()));
        }
        let name = if let Some(idx) = token.find('*') {
            token[..idx].trim()
        } else {
            token
        };
        if name.is_empty() {
            return Err(Error::Export("empty source name in layout".to_string()));
        }
    }
    Ok(())
}
