//! Pure-Rust PDF pane capture: rasterize the pages with hayro, stack them on a
//! viewer-style backdrop, and slice viewport windows at per-frame offsets.
//! No Chromium, no temp files, no HTTP server — a PDF pane renders in-process,
//! so it starts instantly and scrolls through the whole document.

use std::path::Path;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, RenderCache, RenderSettings};

use crate::error::{Error, Result};
use crate::model::{ScrollDirection, Velocity};

use super::stage::{ease, ScrollParams};

/// Backdrop behind/between pages (Chrome PDF viewer gray).
const BACKDROP: [u8; 4] = [0x52, 0x56, 0x59, 0xff];
/// Vertical gap between pages and at the top/bottom, in px.
const GAP: usize = 12;

/// The fastest a PDF pane may pan, in pixels per second.
///
/// Continuity was never the hard part — one offset per output frame is a
/// memcpy. Legibility is: panning a 10-page document across a 9.7 s window
/// measured 124 px per frame, replacing the whole viewport every 0.58 s, which
/// reads as a blur however smooth it is. The speed used to fall out of the
/// document's length, so a longer PDF panned *faster*. This caps it, and when
/// the cap and a `scroll` step's `duration_ms` disagree, the cap wins: the pan
/// covers less of the document rather than speeding up to finish it.
const MAX_PAN_SPEED_PX_PER_SEC: f64 = 600.0;
/// Page width as a fraction of the pane width (fit-width with side margins).
const PAGE_FRAC: f64 = 0.90;
/// Lead-in at the top of the document before panning starts, in ms.
const LEAD_IN_MS: f64 = 400.0;
/// Maximum fraction of the pane's on-screen window the lead-in may occupy.
const LEAD_IN_MAX_FRAC: f64 = 0.15;

/// A PDF scene that computes viewport slices on demand, one per output frame.
/// Holds the rasterized tall document image and produces a viewport window at
/// the offset appropriate for the current progress, without precomputing every
/// frame up front.
pub struct PdfScene {
    width: usize,
    height: usize,
    doc: Vec<u8>,
    doc_w: usize,
    doc_h: usize,
    max_off: usize,
    /// Where the pan starts. Zero panning down; for an upward pan of a document
    /// too long to cross at the capped speed, the bottom of the travel window,
    /// so `up` still starts at the end of the document.
    base_off: usize,
    /// Output frames the pane is on screen. Progress maps onto these.
    window_frames: usize,
    /// Output frames the pan itself lasts, from the `scroll` step's duration.
    /// Frames past it hold the last offset.
    output_frames: usize,
    lead_in: usize,
    direction: ScrollDirection,
    velocity: Velocity,
    cached_frame: Vec<u8>,
    cached_offset: Option<usize>,
}

impl PdfScene {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Return the viewport frame for the given timeline progress in `[0, 1]`.
    pub fn frame_at(&mut self, progress: f64) -> &[u8] {
        let idx = self.frame_index(progress);
        let off = self.base_off
            + offset_for_frame(
                idx,
                self.lead_in,
                self.output_frames,
                self.max_off,
                self.direction,
                self.velocity,
            );
        if self.cached_offset == Some(off) {
            return &self.cached_frame;
        }
        self.cached_frame = self.slice_at(off);
        self.cached_offset = Some(off);
        &self.cached_frame
    }

    fn frame_index(&self, progress: f64) -> usize {
        if self.window_frames <= 1 {
            return 0;
        }
        let p = progress.clamp(0.0, 1.0);
        (p * (self.window_frames - 1) as f64).round() as usize
    }

    fn slice_at(&self, off: usize) -> Vec<u8> {
        let mut out = vec![0u8; self.width * self.height * 4];
        for px in out.as_chunks_mut::<4>().0 {
            px.copy_from_slice(&BACKDROP);
        }
        let rows = self.height.min(self.doc_h.saturating_sub(off));
        for row in 0..rows {
            let s = (off + row) * self.doc_w * 4;
            let d = row * self.width * 4;
            out[d..d + self.doc_w * 4].copy_from_slice(&self.doc[s..s + self.doc_w * 4]);
        }
        out
    }
}

/// Compute the vertical offset for a given output frame index.
///
/// Frames `[0, lead_in)` sit at the start position (top for `Down`, bottom for
/// `Up`). After the lead-in, the offset runs from 0 to `max_off` (or `max_off`
/// to 0 for `Up`) across the remaining frames, with easing applied per the
/// velocity curve.
pub fn offset_for_frame(
    frame_idx: usize,
    lead_in: usize,
    output_frames: usize,
    max_off: usize,
    direction: ScrollDirection,
    velocity: Velocity,
) -> usize {
    if max_off == 0 || output_frames <= 1 {
        return match direction {
            ScrollDirection::Down => 0,
            ScrollDirection::Up => max_off,
        };
    }
    if frame_idx < lead_in {
        return match direction {
            ScrollDirection::Down => 0,
            ScrollDirection::Up => max_off,
        };
    }
    let pan_frames = output_frames.saturating_sub(lead_in);
    if pan_frames <= 1 {
        return match direction {
            ScrollDirection::Down => 0,
            ScrollDirection::Up => max_off,
        };
    }
    let i = frame_idx - lead_in;
    if i >= pan_frames - 1 {
        return match direction {
            ScrollDirection::Down => max_off,
            ScrollDirection::Up => 0,
        };
    }
    let t = i as f64 / (pan_frames - 1) as f64;
    let eased = ease(t, velocity);
    let offset = (max_off as f64 * eased).round() as usize;
    match direction {
        ScrollDirection::Down => offset,
        ScrollDirection::Up => max_off.saturating_sub(offset),
    }
}

/// Compute the lead-in (in frames) before panning starts.
///
/// 400 ms worth of frames, clamped to at most 15% of the pane's on-screen
/// window so a brief pane does not spend most of its life motionless.
pub fn lead_in_frames(output_frames: usize, fps: f64) -> usize {
    let lead_400 = (LEAD_IN_MS / 1000.0 * fps).round() as usize;
    let max_lead = (LEAD_IN_MAX_FRAC * output_frames as f64).round() as usize;
    lead_400.min(max_lead)
}

/// Capture a PDF as a [`PdfScene`]: the document is rasterized once into a
/// tall image, and viewport slices are produced on demand — one distinct
/// offset per output frame — rather than as a fixed set of keyframes.
///
/// `output_frames` is how many output frames the pane will be on screen.
/// `should_scroll` is whether a scroll step is directed at this pane; when
/// false, the scene is a single static frame at the top of the document.
/// `direction` and `velocity` control the scroll behavior.
#[allow(clippy::too_many_arguments)]
pub fn capture_scene(
    pdf_path: &Path,
    pane_w: usize,
    pane_h: usize,
    output_frames: usize,
    fps: f64,
    pan: Option<ScrollParams>,
) -> Result<PdfScene> {
    let data = std::fs::read(pdf_path).map_err(|e| Error::io(pdf_path, e))?;
    let pdf = Pdf::new(data).map_err(|e| Error::Export(format!("read PDF: {e:?}")))?;

    let page_w = ((pane_w as f64) * PAGE_FRAC).round() as usize;
    let cache = RenderCache::new();
    let settings = InterpreterSettings::default();

    let mut pages: Vec<(usize, usize, Vec<u8>)> = Vec::new();
    for page in pdf.pages().iter() {
        let (pw, _) = page.render_dimensions();
        let scale = page_w as f32 / pw.max(1.0);
        let pix = render(
            page,
            &cache,
            &settings,
            &RenderSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: WHITE,
                ..Default::default()
            },
        );
        let (w, h) = (pix.width() as usize, pix.height() as usize);
        let mut rgba = pix.data_as_u8_slice().to_vec();
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        pages.push((w, h, rgba));
    }
    if pages.is_empty() {
        return Err(Error::Export(format!(
            "{}: PDF has no pages",
            pdf_path.display()
        )));
    }

    let doc_w = pane_w;
    let doc_h = GAP + pages.iter().map(|(_, h, _)| h + GAP).sum::<usize>();
    let mut doc = vec![0u8; doc_w * doc_h * 4];
    for px in doc.as_chunks_mut::<4>().0 {
        px.copy_from_slice(&BACKDROP);
    }
    let mut y = GAP;
    for (w, h, rgba) in &pages {
        let x0 = doc_w.saturating_sub(*w) / 2;
        let cw = (*w).min(doc_w - x0);
        for row in 0..*h {
            let s = row * w * 4;
            let d = ((y + row) * doc_w + x0) * 4;
            doc[d..d + cw * 4].copy_from_slice(&rgba[s..s + cw * 4]);
        }
        y += h + GAP;
    }

    let doc_max_off = doc_h.saturating_sub(pane_h);
    let plan = pan.filter(|_| doc_max_off > 0).map(|p| {
        let frames = pan_frames(p.seconds, output_frames.max(1), fps);
        let lead = if frames <= 1 {
            0
        } else {
            lead_in_frames(frames, fps)
        };
        let travel = capped_travel(doc_max_off, frames.saturating_sub(lead), fps);
        (p, frames, lead, travel)
    });

    let (max_off, base_off, effective_frames, lead, direction, velocity) = match plan {
        Some((p, frames, lead, travel)) => {
            let base = match p.direction {
                ScrollDirection::Down => 0,
                // An upward pan still starts at the end of the document, even
                // when the cap means it cannot reach the beginning.
                ScrollDirection::Up => doc_max_off - travel,
            };
            (travel, base, frames, lead, p.direction, p.velocity)
        }
        None => (0, 0, 1, 0, ScrollDirection::Down, Velocity::Constant),
    };

    Ok(PdfScene {
        width: pane_w,
        height: pane_h,
        doc,
        doc_w,
        doc_h,
        max_off,
        base_off,
        window_frames: output_frames.max(1),
        output_frames: effective_frames,
        lead_in: lead,
        direction,
        velocity,
        cached_frame: Vec::new(),
        cached_offset: None,
    })
}

/// How many output frames the pan itself lasts: the `scroll` step's declared
/// duration, never longer than the pane is on screen and never shorter than one
/// frame. Frames after it hold the last offset.
fn pan_frames(seconds: f64, output_frames: usize, fps: f64) -> usize {
    if seconds <= 0.0 {
        return output_frames;
    }
    ((seconds * fps.max(1.0)).round() as usize).clamp(1, output_frames)
}

/// How far the pan may travel: the whole document, unless crossing it would
/// exceed [`MAX_PAN_SPEED_PX_PER_SEC`], in which case it covers only what fits
/// at that speed.
fn capped_travel(doc_max_off: usize, panning_frames: usize, fps: f64) -> usize {
    let seconds = panning_frames as f64 / fps.max(1.0);
    let budget = (MAX_PAN_SPEED_PX_PER_SEC * seconds).round() as usize;
    doc_max_off.min(budget.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The measured case that motivated the cap: 10 pages (~17 400 px of travel)
    /// across a 9.7 s window at 15 fps panned 124 px per frame — a new viewport
    /// every 0.58 s, unreadable however smooth. The cap must cut the travel, not
    /// the time.
    #[test]
    fn a_long_document_pans_at_the_capped_speed_not_faster() {
        let (doc_max_off, fps) = (17_392usize, 15.0);
        let frames = pan_frames(8.0, 146, fps); // duration_ms = 8000 in the score
        let lead = lead_in_frames(frames, fps);
        let travel = capped_travel(doc_max_off, frames - lead, fps);
        let seconds = (frames - lead) as f64 / fps;
        let speed = travel as f64 / seconds;
        assert!(
            speed <= MAX_PAN_SPEED_PX_PER_SEC + 1.0,
            "panned at {speed:.0} px/s, cap is {MAX_PAN_SPEED_PX_PER_SEC}"
        );
        assert!(
            travel < doc_max_off,
            "a long document must not be crossed whole"
        );
        let per_frame = travel as f64 / (frames - lead) as f64;
        assert!(per_frame < 45.0, "{per_frame:.0} px/frame is still a blur");
    }

    /// A document short enough to cross under the cap is crossed whole — the cap
    /// is a ceiling, not a target.
    #[test]
    fn a_short_document_is_still_crossed_completely() {
        let travel = capped_travel(1_200, 90, 15.0); // 6 s of budget = 3600 px
        assert_eq!(travel, 1_200);
    }

    /// The cap wins over the declared duration: asking for the whole document in
    /// less time buys less document, never more speed.
    #[test]
    fn the_cap_wins_when_it_collides_with_the_declared_duration() {
        let fps = 15.0;
        let long = capped_travel(17_392, pan_frames(8.0, 146, fps), fps);
        let longer = capped_travel(17_392, pan_frames(20.0, 300, fps), fps);
        assert!(longer > long, "more time must buy more document");
        assert!(long < 17_392 && longer < 17_392);
    }

    /// `duration_ms` bounds the pan, and the pane window bounds `duration_ms`.
    #[test]
    fn pan_never_outlasts_the_pane_window() {
        assert_eq!(pan_frames(8.0, 146, 15.0), 120);
        assert_eq!(pan_frames(60.0, 146, 15.0), 146, "clamped to the window");
        assert_eq!(
            pan_frames(0.0, 146, 15.0),
            146,
            "no duration means the window"
        );
        assert_eq!(pan_frames(8.0, 1, 15.0), 1);
    }

    #[test]
    fn offset_first_frame_is_zero_last_is_max_off() {
        let offsets: Vec<usize> = (0..20)
            .map(|i| offset_for_frame(i, 3, 20, 500, ScrollDirection::Down, Velocity::Constant))
            .collect();
        assert_eq!(offsets[0], 0);
        assert_eq!(offsets[19], 500);
        for i in 1..20 {
            assert!(offsets[i] >= offsets[i - 1], "non-monotonic at {i}");
        }
        for i in 4..20 {
            assert!(
                offsets[i] > offsets[i - 1],
                "strictly increasing after lead-in at {i}"
            );
        }
    }

    #[test]
    fn lead_in_long_window_yields_400ms() {
        let fps = 30.0;
        let output_frames = 300;
        let lead = lead_in_frames(output_frames, fps);
        assert_eq!(lead, 12);
    }

    #[test]
    fn lead_in_short_window_yields_15_percent() {
        let fps = 30.0;
        let output_frames = 10;
        let lead = lead_in_frames(output_frames, fps);
        assert_eq!(lead, 2);
    }

    #[test]
    fn max_off_zero_yields_static() {
        for i in 0..10 {
            assert_eq!(
                offset_for_frame(i, 2, 10, 0, ScrollDirection::Down, Velocity::Constant),
                0
            );
        }
    }

    #[test]
    fn no_half_hold() {
        let offsets: Vec<usize> = (0..100)
            .map(|i| {
                offset_for_frame(
                    i,
                    lead_in_frames(100, 30.0),
                    100,
                    1000,
                    ScrollDirection::Down,
                    Velocity::Constant,
                )
            })
            .collect();
        let first_nonzero = offsets.iter().position(|&o| o > 0);
        let lead = lead_in_frames(100, 30.0);
        assert!(
            first_nonzero.unwrap() <= lead + 1,
            "panning should start shortly after lead-in ({lead} frames), \
             but first nonzero offset is at frame {first_nonzero:?}"
        );
    }

    #[test]
    fn single_frame_no_panic() {
        assert_eq!(
            offset_for_frame(0, 0, 1, 500, ScrollDirection::Down, Velocity::Constant),
            0
        );
    }

    #[test]
    fn lead_in_zero_frames() {
        assert_eq!(lead_in_frames(0, 30.0), 0);
    }

    #[test]
    fn offset_up_constant_starts_at_max_ends_at_zero() {
        let offsets: Vec<usize> = (0..20)
            .map(|i| offset_for_frame(i, 3, 20, 500, ScrollDirection::Up, Velocity::Constant))
            .collect();
        assert_eq!(offsets[0], 500);
        assert_eq!(offsets[19], 0);
        for i in 1..20 {
            assert!(offsets[i] <= offsets[i - 1], "non-increasing at {i}");
        }
        for i in 4..20 {
            assert!(
                offsets[i] < offsets[i - 1],
                "strictly decreasing after lead-in at {i}"
            );
        }
    }

    #[test]
    fn offset_up_ease_in_out_starts_at_max_ends_at_zero() {
        let offsets: Vec<usize> = (0..20)
            .map(|i| offset_for_frame(i, 3, 20, 500, ScrollDirection::Up, Velocity::EaseInOut))
            .collect();
        assert_eq!(offsets[0], 500);
        assert_eq!(offsets[19], 0);
        for i in 1..20 {
            assert!(offsets[i] <= offsets[i - 1], "non-increasing at {i}");
        }
    }
}
