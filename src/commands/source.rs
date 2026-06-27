//! `demo source` — define a content source for scene composition.
//!
//! Sources are the building blocks of scenes: each source represents a
//! terminal, browser, or file that can be placed in a layout. Run this
//! before `demo capture` to pre-define what goes into each scene.

use std::io::IsTerminal;
use std::path::Path;

use inquire::{Select, Text};

use crate::cli::{SourceArgs, SourceKindArg};
use crate::error::{Error, Result};
use crate::model::{Score, Source, SourceKind};

pub fn run(args: SourceArgs) -> Result<()> {
    let score_path = &args.score;

    // --list: show existing sources
    if args.list {
        let score = load_score(score_path)?;
        if score.sources.is_empty() {
            println!("No sources defined. Run `demo source` to add one.");
        } else {
            println!("Sources in {}:\n", score_path.display());
            for s in &score.sources {
                let kind = match s.kind {
                    SourceKind::Terminal => "terminal",
                    SourceKind::Browser => "browser",
                };
                let extra = match s.kind {
                    SourceKind::Browser => {
                        let theme = s.theme.as_deref().unwrap_or("default");
                        format!(" url={} theme={}", s.url.as_deref().unwrap_or(""), theme)
                    }
                    SourceKind::Terminal => String::new(),
                };
                println!("  {:12} {:10}{}", s.id, kind, extra);
            }
        }
        return Ok(());
    }

    // --remove: remove a source
    if let Some(id) = &args.remove {
        let mut score = load_score(score_path)?;
        let before = score.sources.len();
        score.sources.retain(|s| &s.id != id);
        if score.sources.len() == before {
            return Err(Error::Export(format!("source '{id}' not found")));
        }
        save_score(score_path, &score)?;
        println!("Removed source '{id}'.");
        return Ok(());
    }

    // Add or update a source
    let source = if !std::io::stdin().is_terminal() || has_minimal_args(&args) {
        resolve_from_args(&args)?
    } else {
        wizard()?
    };

    let mut score = load_score(score_path)?;
    // Replace existing source with the same ID
    score.sources.retain(|s| s.id != source.id);
    score.sources.push(source.clone());
    save_score(score_path, &score)?;

    println!("✓ Source '{}' added to {}", source.id, score_path.display());
    Ok(())
}

fn has_minimal_args(args: &SourceArgs) -> bool {
    args.id.is_some() && args.r#type.is_some()
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

fn resolve_from_args(args: &SourceArgs) -> Result<Source> {
    let id = args
        .id
        .clone()
        .ok_or_else(|| Error::Export("source ID is required (or run interactively)".to_string()))?;
    let kind = args.r#type.ok_or_else(|| {
        Error::Export("source type is required: --type terminal or --type browser".to_string())
    })?;

    match kind {
        SourceKindArg::Terminal => Ok(Source {
            id,
            kind: SourceKind::Terminal,
            url: None,
            theme: None,
        }),
        SourceKindArg::Browser => {
            let url = args.url.clone().ok_or_else(|| {
                Error::Export("browser source needs a URL: --url <URL>".to_string())
            })?;
            Ok(Source {
                id,
                kind: SourceKind::Browser,
                url: Some(normalize_url(&url)),
                theme: args.theme.map(|t| t.as_str().to_string()),
            })
        }
    }
}

fn ask<T>(r: std::result::Result<T, inquire::InquireError>) -> Result<T> {
    r.map_err(|e| Error::Export(format!("wizard: {e}")))
}

fn wizard() -> Result<Source> {
    println!("\n  demo source — define a content source\n");

    let id = ask(Text::new("Source ID:")
        .with_help_message("unique identifier (e.g. 'main', 'google', 'github')")
        .with_default("main")
        .prompt())?;
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(Error::Export("source ID cannot be empty".to_string()));
    }

    let kind = ask(Select::new("Source type:", vec!["terminal", "browser"]).prompt())?;
    let is_browser = kind == "browser";

    if is_browser {
        let url = ask(Text::new("URL:")
            .with_help_message("http, https, or file:// URL")
            .prompt())?;
        let url = normalize_url(url.trim());

        let theme = ask(Select::new(
            "Browser theme:",
            vec!["default (page preference)", "light", "dark"],
        )
        .prompt())?;
        let theme = match theme {
            "light" => Some("light".to_string()),
            "dark" => Some("dark".to_string()),
            _ => None,
        };

        Ok(Source {
            id,
            kind: SourceKind::Browser,
            url: Some(url),
            theme,
        })
    } else {
        Ok(Source {
            id,
            kind: SourceKind::Terminal,
            url: None,
            theme: None,
        })
    }
}

fn normalize_url(url: &str) -> String {
    let u = url.trim();
    if u.contains("://") {
        u.to_string()
    } else if u.starts_with("localhost") || u.starts_with("127.0.0.1") {
        format!("http://{u}")
    } else {
        format!("https://{u}")
    }
}
