//! `demo export` — render a recording to one or more formats. Pure playback:
//! it replays a recording (a `.cast` from `demo record`, or a raw `capture`)
//! and never executes the demo.

use crate::cli::{all_targets, ExportArgs};
use crate::error::{Error, Result};
use crate::export::{recording, render, scale_pane_windows, scale_recording};
use crate::model::Score;

pub fn run(args: ExportArgs) -> Result<()> {
    let (mut rec, mut score, faithful) = recording::read(&args.input)?;

    // A faithful capture renders the real session as-is — typing and spacing are
    // exactly as recorded, NOT humanized. Require `--force` to render one, so the
    // clean path (`demo record`) is the default; but keep it possible, since
    // interactive / side-effecting demos (ghScaff, secrets) can't be re-executed.
    if faithful && !args.force {
        return Err(Error::Export(format!(
            "{} is a faithful capture (typing/idle as recorded, not re-humanized).\n  \
             • To render it as-is, add `--force`. This is the right path for \
             interactive or side-effecting demos — a wizard, anything that needs \
             secrets or creates real resources (e.g. ghScaff) — which `demo record` \
             would RE-RUN and break.\n  \
             • Only for a deterministic demo with no side effects: `demo record` \
             re-executes `demo.toml` for a humanized take, then export its `.rec`.",
            args.input.display()
        )));
    }

    scale_recording(&mut rec, args.speed);
    scale_pane_windows(&mut score, args.speed);

    // Apply resolution override if specified
    if let Some((new_w, new_h)) =
        resolve_export_resolution(&args, score.layout.width, score.layout.height)?
    {
        rescale_layout(&mut score, new_w, new_h);
        eprintln!(
            "note: overriding resolution to {}x{} (capture was {}x{})",
            new_w, new_h, score.layout.width, score.layout.height
        );
    }

    if faithful {
        eprintln!(
            "note: rendering a faithful capture as-is (--force) — typing/idle are \
             as recorded; `demo record` would re-humanize them."
        );
    }

    // No target given → build every supported format.
    let targets = args.targets.map(|t| t.0).unwrap_or_else(all_targets);
    for target in targets {
        let path = render(&rec, &score, target)?;
        println!("exported {} → {}", args.input.display(), path.display());
    }
    Ok(())
}

/// Compute the export resolution from flags, or `None` to keep the capture-time resolution.
fn resolve_export_resolution(
    args: &ExportArgs,
    _default_w: u32,
    _default_h: u32,
) -> Result<Option<(u32, u32)>> {
    if let Some(r) = &args.resolution {
        return parse_resolution_override(r).map(Some);
    }
    if let Some(a) = &args.aspect {
        let q = args.quality.as_deref().unwrap_or("fullhd");
        return canvas_from_aspect_quality(a, q).map(Some);
    }
    if let Some(q) = &args.quality {
        return canvas_from_aspect_quality("16:9", q).map(Some);
    }
    Ok(None)
}

/// Parse a `--resolution` value for export: `WxH` or a preset name.
fn parse_resolution_override(s: &str) -> Result<(u32, u32)> {
    let v = s.trim().to_ascii_lowercase();
    // Try preset names
    const PRESETS: [(&str, u32, u32); 5] = [
        ("landscape", 1920, 1080),
        ("portrait", 1080, 1920),
        ("square", 1080, 1080),
        ("standard", 1280, 720),
        ("fullhd", 1920, 1080),
    ];
    if let Some(&(_, w, h)) = PRESETS.iter().find(|(name, ..)| *name == v) {
        return Ok((w, h));
    }
    // Try WxH
    if let Some((w, h)) = v.split_once(['x', '×']) {
        if let (Ok(w), Ok(h)) = (w.trim().parse::<u32>(), h.trim().parse::<u32>()) {
            if w > 0 && h > 0 {
                return Ok((w, h));
            }
        }
    }
    Err(Error::Export(format!(
        "invalid resolution '{s}' — try a WxH pair (e.g. 1920x1080) or a preset (landscape, portrait, square, standard, fullhd)"
    )))
}

/// Aspect ratios for export.
const ASPECTS: [(&str, u32, u32); 4] = [
    ("16:9", 16, 9),
    ("9:16", 9, 16),
    ("4:3", 4, 3),
    ("1:1", 1, 1),
];

/// Quality tiers — the short side of the canvas, in pixels.
const QUALITIES: [(&str, u32); 2] = [("fullhd", 1080), ("hd", 720)];

/// Compute the canvas `(width, height)` for an aspect ratio + quality.
fn canvas_from_aspect_quality(aspect: &str, quality: &str) -> Result<(u32, u32)> {
    let av = aspect.trim().to_ascii_lowercase();
    let &(_, a, b) = ASPECTS
        .iter()
        .find(|(name, ..)| *name == av)
        .ok_or_else(|| {
            Error::Export(format!(
                "invalid aspect '{aspect}' — try 16:9, 9:16, 4:3, or 1:1"
            ))
        })?;
    let qv = quality.trim().to_ascii_lowercase();
    let base = QUALITIES
        .iter()
        .find(|(name, _)| *name == qv)
        .map(|(_, b)| *b)
        .ok_or_else(|| Error::Export(format!("invalid quality '{quality}' — try fullhd or hd")))?;
    let short = a.min(b);
    Ok((a * base / short, b * base / short))
}

/// Rescale the layout and all panes to a new canvas size.
fn rescale_layout(score: &mut Score, new_w: u32, new_h: u32) {
    let old_w = score.layout.width;
    let old_h = score.layout.height;
    if old_w == new_w && old_h == new_h {
        return;
    }
    let scale_x = new_w as f64 / old_w as f64;
    let scale_y = new_h as f64 / old_h as f64;

    score.layout.width = new_w;
    score.layout.height = new_h;

    for pane in &mut score.layout.panes {
        pane.x = (pane.x as f64 * scale_x).round() as u32;
        pane.y = (pane.y as f64 * scale_y).round() as u32;
        pane.width = (pane.width as f64 * scale_x).round() as u32;
        pane.height = (pane.height as f64 * scale_y).round() as u32;
    }
}
