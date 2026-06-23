//! `demo prepare` — scaffold a stage: the canvas, its panes, and the trigger
//! steps around a terminal anchor. `capture --into <stage>` then captures the
//! terminal flow and splices it in at the anchor (normalizing automatically) —
//! so the layout (e.g. a PDF beside the terminal) is authored once, not
//! re-recorded.
//!
//! Configure it with the flags, or run `demo prepare --wizard` for a guided,
//! ghScaff-style interactive setup. Both feed the same stage builder.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use inquire::{Select, Text};

use crate::cli::{PrepareArgs, Preset};
use crate::error::{Error, Result};
use crate::model::{
    DemoMeta, Layout, Pane, PaneKind, Score, ScrollDirection, Step, Typing, Velocity,
};

const TERM_ID: &str = "term";
const VIEW_ID: &str = "view";

/// Resolved stage options, gathered from CLI flags or the interactive wizard.
struct StageOpts {
    name: String,
    preset: Preset,
    /// Browser pane URL (split/stacked); `None` writes a placeholder.
    view_url: Option<String>,
    width: u32,
    height: u32,
    fps: u32,
    output: PathBuf,
}

pub fn run(args: PrepareArgs) -> Result<()> {
    // The wizard runs on `--wizard`, or by default when `prepare` is called with
    // no flags on an interactive terminal. With flags (or no TTY, e.g. CI), the
    // flags drive it non-interactively.
    let interactive = args.wizard || (bare_invocation() && std::io::stdin().is_terminal());
    let opts = if interactive {
        wizard(args.output.clone())?
    } else {
        from_args(args)
    };
    let score = build_stage(&opts);
    score.save(&opts.output)?;
    print_next(&opts);
    Ok(())
}

/// True when `prepare` was invoked with no arguments after it (`demo prepare`).
fn bare_invocation() -> bool {
    let mut args = std::env::args().skip_while(|a| a != "prepare");
    args.next(); // the "prepare" token itself
    args.next().is_none()
}

/// Resolve options straight from the CLI flags.
fn from_args(args: PrepareArgs) -> StageOpts {
    let view_url = args
        .url
        .clone()
        .or_else(|| args.pdf.as_deref().map(pdf_url));
    StageOpts {
        name: args.name,
        preset: args.preset,
        view_url,
        width: args.width,
        height: args.height,
        fps: args.fps,
        output: args.output,
    }
}

/// Build the stage score from resolved options.
fn build_stage(o: &StageOpts) -> Score {
    let panes = match o.preset {
        Preset::Single => vec![terminal(0, 0, o.width, o.height)],
        Preset::Split => {
            let tw = o.width * 3 / 5; // terminal ≈ 60% of the width
            vec![
                terminal(0, 0, tw, o.height),
                browser(tw, 0, o.width - tw, o.height, o.view_url.clone()),
            ]
        }
        Preset::Stacked => {
            let th = o.height * 3 / 5; // terminal ≈ 60% of the height
            vec![
                terminal(0, 0, o.width, th),
                browser(0, th, o.width, o.height - th, o.view_url.clone()),
            ]
        }
    };

    // The terminal `focus` is the anchor `record --into` fills. For multi-pane
    // presets, after the flow we settle, reveal the view, and pan it.
    let mut timeline = vec![Step::Focus {
        pane: TERM_ID.to_string(),
    }];
    if o.preset != Preset::Single {
        timeline.push(Step::Wait { duration_ms: 800 });
        timeline.push(Step::Focus {
            pane: VIEW_ID.to_string(),
        });
        timeline.push(Step::Scroll {
            direction: ScrollDirection::Down,
            velocity: Velocity::default(),
            duration_ms: 4000,
            pane: Some(VIEW_ID.to_string()),
        });
        timeline.push(Step::Wait { duration_ms: 600 });
    }
    timeline.push(Step::Terminate);

    Score {
        demo: DemoMeta {
            name: o.name.clone(),
            output_dir: "./dist".into(),
            prompt: None,
        },
        env: None,
        typing: Some(Typing::default()),
        layout: Layout {
            width: o.width,
            height: o.height,
            fps: o.fps,
            line_height: 1.0,
            background: Some("#0b0f14".to_string()),
            panes,
        },
        timeline,
    }
}

/// Map an inquire prompt result into our error type (a cancel reads cleanly).
fn ask<T>(r: std::result::Result<T, inquire::InquireError>) -> Result<T> {
    r.map_err(|e| Error::Export(format!("wizard: {e}")))
}

/// Guided, ghScaff-style interactive setup. Prepares the *recording* (the
/// `demo.toml`); the output path comes from `-o` (default `demo.toml`), not a
/// prompt — export to any format is a separate, later step.
fn wizard(output: PathBuf) -> Result<StageOpts> {
    println!("\n  demo prepare — configure the recording\n");

    let name = ask(Text::new("Demo name:").with_default("demo").prompt())?;

    let layout = ask(Select::new(
        "Layout:",
        vec![
            "single — one terminal pane",
            "split — terminal + browser pane (e.g. a PDF)",
            "stacked — terminal above a browser pane",
        ],
    )
    .prompt())?;
    let preset = if layout.starts_with("split") {
        Preset::Split
    } else if layout.starts_with("stacked") {
        Preset::Stacked
    } else {
        Preset::Single
    };

    let view_url = if preset == Preset::Single {
        None
    } else {
        let src = ask(Select::new(
            "Browser pane shows:",
            vec!["a PDF file", "a URL", "set later (placeholder)"],
        )
        .prompt())?;
        if src.starts_with("a PDF") {
            let p = ask(Text::new("PDF path:")
                .with_help_message("turned into a file:// URL; it need not exist yet")
                .prompt())?;
            let p = p.trim();
            (!p.is_empty()).then(|| pdf_url(Path::new(p)))
        } else if src.starts_with("a URL") {
            let u = ask(Text::new("URL:")
                .with_default("http://localhost:8080")
                .prompt())?;
            let u = u.trim();
            (!u.is_empty()).then(|| u.to_string())
        } else {
            None
        }
    };

    let size = ask(Select::new(
        "Canvas size:",
        vec!["1280×720 (720p)", "1920×1080 (1080p)", "1600×900", "custom"],
    )
    .prompt())?;
    let (width, height) = match size {
        "1920×1080 (1080p)" => (1920, 1080),
        "1600×900" => (1600, 900),
        "custom" => {
            let w = ask(Text::new("Width:").with_default("1280").prompt())?
                .trim()
                .parse::<u32>()
                .unwrap_or(1280);
            let h = ask(Text::new("Height:").with_default("720").prompt())?
                .trim()
                .parse::<u32>()
                .unwrap_or(720);
            (w, h)
        }
        _ => (1280, 720),
    };

    let fps = ask(Text::new("FPS:").with_default("15").prompt())?
        .trim()
        .parse::<u32>()
        .unwrap_or(15);

    Ok(StageOpts {
        name,
        preset,
        view_url,
        width,
        height,
        fps,
        output,
    })
}

fn print_next(o: &StageOpts) {
    let out = o.output.display();
    println!("prepared {} stage → {out}", preset_name(o.preset));
    println!("  next:  demo capture --into {out}   (splice your terminal session in)");
    println!("  then:  demo record {out}  →  demo export   (re-run, then render)");
    if o.preset != Preset::Single {
        if o.view_url.is_none() {
            println!("  note:  set the browser pane `url` (pass --pdf <file> or --url <url>)");
        }
        println!(
            "  tip:   to reveal the view off a build line, replace the wait after the\n         terminal with: action = \"wait_for_stdout\", match = \"<line>\", pane = \"{TERM_ID}\""
        );
    }
}

fn terminal(x: u32, y: u32, width: u32, height: u32) -> Pane {
    Pane {
        id: TERM_ID.to_string(),
        kind: PaneKind::Terminal,
        x,
        y,
        width,
        height,
        font_family: Some("monospace".to_string()),
        font_size: Some(16),
        url: None,
    }
}

fn browser(x: u32, y: u32, width: u32, height: u32, url: Option<String>) -> Pane {
    Pane {
        id: VIEW_ID.to_string(),
        kind: PaneKind::Browser,
        x,
        y,
        width,
        height,
        font_family: None,
        font_size: None,
        // A placeholder keeps the stage valid; the printed note nudges the user.
        url: Some(url.unwrap_or_else(|| "file:///PATH/TO/output.pdf".to_string())),
    }
}

/// Turn a local path into an absolute `file://` URL (not canonicalized — the
/// artifact may not exist until the demo builds it).
fn pdf_url(p: &Path) -> String {
    let abs = std::env::current_dir()
        .map(|cwd| cwd.join(p))
        .unwrap_or_else(|_| p.to_path_buf());
    format!("file://{}", abs.display())
}

fn preset_name(p: Preset) -> &'static str {
    match p {
        Preset::Single => "single",
        Preset::Split => "split",
        Preset::Stacked => "stacked",
    }
}
