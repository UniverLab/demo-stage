//! `demo prepare` — scaffold a stage: the canvas, its panes, and the trigger
//! steps around a terminal anchor. `record --into <stage>` then captures the
//! terminal flow, and `normalize` splices it in at the anchor — so the layout
//! (e.g. a PDF shown beside the terminal) is authored once, not re-recorded.

use std::path::Path;

use crate::cli::{PrepareArgs, Preset};
use crate::error::Result;
use crate::model::{
    DemoMeta, Layout, Pane, PaneKind, Score, ScrollDirection, Step, Typing, Velocity,
};

const TERM_ID: &str = "term";
const VIEW_ID: &str = "view";

pub fn run(args: PrepareArgs) -> Result<()> {
    // Browser pane source (split/stacked): --url wins, else --pdf as a file URL.
    let view_url = args
        .url
        .clone()
        .or_else(|| args.pdf.as_deref().map(pdf_url));

    let panes = match args.preset {
        Preset::Single => vec![terminal(0, 0, args.width, args.height)],
        Preset::Split => {
            let tw = args.width * 3 / 5; // terminal ≈ 60% of the width
            vec![
                terminal(0, 0, tw, args.height),
                browser(tw, 0, args.width - tw, args.height, view_url.clone()),
            ]
        }
        Preset::Stacked => {
            let th = args.height * 3 / 5; // terminal ≈ 60% of the height
            vec![
                terminal(0, 0, args.width, th),
                browser(0, th, args.width, args.height - th, view_url.clone()),
            ]
        }
    };

    // The terminal `focus` is the anchor `record --into` fills. For multi-pane
    // presets, after the flow we settle, reveal the view, and pan it.
    let mut timeline = vec![Step::Focus {
        pane: TERM_ID.to_string(),
    }];
    if args.preset != Preset::Single {
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

    let score = Score {
        demo: DemoMeta {
            name: args.name.clone(),
            output_dir: "./dist".into(),
        },
        env: None,
        typing: Some(Typing::default()),
        layout: Layout {
            width: args.width,
            height: args.height,
            fps: args.fps,
            background: Some("#0b0f14".to_string()),
            panes,
        },
        timeline,
    };

    score.save(&args.output)?;

    let out = args.output.display();
    println!("prepared {} stage → {out}", preset_name(args.preset));
    println!("  next:  demo record --into {out}");
    println!("         demo normalize macro.raw.toml -o {out}");
    if args.preset != Preset::Single {
        if view_url.is_none() {
            println!("  note:  set the browser pane `url` (pass --pdf <file> or --url <url>)");
        }
        println!(
            "  tip:   to reveal the view off a build line, replace the wait after the\n         terminal with: action = \"wait_for_stdout\", match = \"<line>\", pane = \"{TERM_ID}\""
        );
    }
    Ok(())
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
