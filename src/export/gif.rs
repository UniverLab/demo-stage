//! GIF target (pure Rust): dedupe identical consecutive frames, then encode
//! only the region that changed between frames (delta frames with `Keep`
//! disposal) with `gif`. `encode` is source-agnostic, so both the
//! single-terminal fast path and the multi-scene stage feed it. No ffmpeg.

use std::path::Path;

use super::raster;
use super::run::Recording;
use crate::error::{Error, Result};
use crate::model::Score;

/// Encode a GIF at `path` from frames produced by `render` (each `w`×`h` RGBA).
pub fn encode(
    path: &Path,
    w: usize,
    h: usize,
    fps: u32,
    render: impl FnOnce(&mut dyn FnMut(&[u8])) -> Result<()>,
) -> Result<()> {
    let file = std::fs::File::create(path).map_err(|e| Error::io(path, e))?;
    let mut encoder = gif::Encoder::new(file, w as u16, h as u16, &[])
        .map_err(|e| Error::Export(format!("gif encoder: {e}")))?;
    encoder
        .set_repeat(gif::Repeat::Infinite)
        .map_err(|e| Error::Export(format!("gif repeat: {e}")))?;

    let mut clock = DelayClock::new(fps);
    // The canvas as written so far — new frames are encoded as a diff against it.
    let mut written: Option<Vec<u8>> = None;
    // The pending frame and how many source frames it has absorbed (identical
    // consecutive frames become one GIF frame with a longer delay).
    let mut held: Option<(Vec<u8>, u64)> = None;
    let mut err: Option<Error> = None;

    render(&mut |rgba: &[u8]| {
        if err.is_some() {
            return;
        }
        match held.as_mut() {
            Some((prev, frames)) if prev.as_slice() == rgba => *frames += 1,
            _ => {
                if let Some((frame, frames)) = held.take() {
                    let delay = clock.delay(frames);
                    if let Err(e) = write_frame(&mut encoder, written.as_deref(), &frame, w, delay)
                    {
                        err = Some(e);
                        return;
                    }
                    written = Some(frame);
                }
                held = Some((rgba.to_vec(), 1));
            }
        }
    })?;

    if let Some(e) = err {
        return Err(e);
    }
    if let Some((frame, frames)) = held.take() {
        let delay = clock.delay(frames);
        write_frame(&mut encoder, written.as_deref(), &frame, w, delay)?;
    }
    Ok(())
}

/// Single-terminal fast path: encode a GIF straight from a recording.
pub fn write_gif(rec: &Recording, score: &Score, path: &Path) -> Result<()> {
    let plan = raster::plan(rec, score);
    encode(path, plan.width, plan.height, plan.fps, |emit| {
        raster::render_frames(rec, score, |f| emit(f)).map(|_| ())
    })
}

/// Per-frame GIF delays (centiseconds) that track the configured fps exactly
/// on average. GIF timing is centisecond-quantized (15 fps → 6.67 cs), so a
/// fixed rounded delay would drift playback ~5% slow; carrying the fractional
/// error keeps the total duration true to the recording.
struct DelayClock {
    per_frame_cs: f64,
    ideal_cs: f64,
    written_cs: u64,
}

impl DelayClock {
    fn new(fps: u32) -> Self {
        Self {
            per_frame_cs: 100.0 / fps.max(1) as f64,
            ideal_cs: 0.0,
            written_cs: 0,
        }
    }

    /// The delay for a GIF frame that spans `frames` source frames.
    fn delay(&mut self, frames: u64) -> u16 {
        self.ideal_cs += frames as f64 * self.per_frame_cs;
        // Browsers clamp delays under 2 cs, so that's the floor.
        let d = ((self.ideal_cs - self.written_cs as f64).round() as i64).clamp(2, u16::MAX as i64);
        self.written_cs += d as u64;
        d as u16
    }
}

/// Write one frame: the first is the full canvas; every other is only the
/// bounding rectangle that changed since the previously written frame, drawn
/// over it (`Keep` disposal). A terminal demo mostly changes a few text rows
/// per frame, so the deltas are a fraction of the canvas.
fn write_frame(
    encoder: &mut gif::Encoder<std::fs::File>,
    written: Option<&[u8]>,
    rgba: &[u8],
    w: usize,
    delay_cs: u16,
) -> Result<()> {
    let (left, top, mut pixels, bw, bh) = match written {
        Some(prev) => {
            // Upstream dedup guarantees a difference; a 1×1 corner is the
            // degenerate fallback if it ever lies.
            let (x0, y0, x1, y1) = diff_rect(prev, rgba, w).unwrap_or((0, 0, 1, 1));
            (x0, y0, crop_rgba(rgba, w, x0, y0, x1, y1), x1 - x0, y1 - y0)
        }
        None => (0, 0, rgba.to_vec(), w, rgba.len() / 4 / w.max(1)),
    };
    let mut frame = gif::Frame::from_rgba_speed(bw as u16, bh as u16, &mut pixels, 10);
    frame.left = left as u16;
    frame.top = top as u16;
    frame.delay = delay_cs;
    frame.dispose = gif::DisposalMethod::Keep;
    encoder
        .write_frame(&frame)
        .map_err(|e| Error::Export(format!("gif frame: {e}")))
}

/// Bounding rectangle `(x0, y0, x1, y1)` (exclusive ends) of the pixels that
/// differ between two same-sized RGBA canvases, or `None` when identical.
fn diff_rect(prev: &[u8], cur: &[u8], w: usize) -> Option<(usize, usize, usize, usize)> {
    let row_len = w * 4;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (y, (pr, cr)) in prev
        .chunks_exact(row_len)
        .zip(cur.chunks_exact(row_len))
        .enumerate()
    {
        if pr == cr {
            continue;
        }
        let first = pr.iter().zip(cr).position(|(a, b)| a != b).unwrap_or(0) / 4;
        let last = row_len
            - 1
            - pr.iter()
                .rev()
                .zip(cr.iter().rev())
                .position(|(a, b)| a != b)
                .unwrap_or(0);
        y0 = y0.min(y);
        y1 = y + 1;
        x0 = x0.min(first);
        x1 = x1.max(last / 4 + 1);
    }
    (y1 > 0).then_some((x0, y0, x1, y1))
}

/// Copy the `(x0, y0)..(x1, y1)` sub-rectangle of a `w`-wide RGBA canvas.
fn crop_rgba(rgba: &[u8], w: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        let row = y * w * 4;
        out.extend_from_slice(&rgba[row + x0 * 4..row + x1 * 4]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_rect_finds_the_changed_region() {
        // 4×3 canvas; change pixels (1,1) and (2,2).
        let w = 4;
        let prev = vec![0u8; w * 3 * 4];
        let mut cur = prev.clone();
        cur[(w + 1) * 4] = 255; // (1,1)
        cur[(2 * w + 2) * 4] = 255; // (2,2)
        assert_eq!(diff_rect(&prev, &cur, w), Some((1, 1, 3, 3)));
        assert_eq!(diff_rect(&prev, &prev, w), None);
    }

    #[test]
    fn crop_extracts_the_subrect() {
        // 3×2 canvas with pixel values = index.
        let w = 3;
        let rgba: Vec<u8> = (0..w * 2 * 4).map(|i| i as u8).collect();
        let out = crop_rgba(&rgba, w, 1, 1, 3, 2);
        // Row 1, pixels 1..3 → bytes 16..24.
        assert_eq!(out, (16..24).map(|i| i as u8).collect::<Vec<_>>());
    }

    #[test]
    fn delay_clock_matches_the_fps_on_average() {
        // 15 fps → 6.67 cs/frame; 15 frames must span 100 cs, not 105.
        let mut c = DelayClock::new(15);
        let total: u64 = (0..15).map(|_| c.delay(1) as u64).sum();
        assert_eq!(total, 100);
        // A held frame spanning 30 source frames is exactly 2 s.
        let mut c = DelayClock::new(15);
        assert_eq!(c.delay(30), 200);
    }

    #[test]
    fn diff_rect_identical_frames() {
        let w = 4;
        let frame = vec![128u8; w * 3 * 4];
        assert_eq!(diff_rect(&frame, &frame, w), None);
    }

    #[test]
    fn diff_rect_single_pixel_change() {
        let w = 2;
        let prev = vec![0u8; w * 2 * 4];
        let mut cur = prev.clone();
        cur[0] = 255; // (0,0)
        assert_eq!(diff_rect(&prev, &cur, w), Some((0, 0, 1, 1)));
    }

    #[test]
    fn diff_rect_full_width_change() {
        let w = 3;
        let prev = vec![0u8; w * 1 * 4];
        let mut cur = prev.clone();
        // Change all pixels in row 0
        for i in 0..w {
            cur[i * 4] = 255;
        }
        assert_eq!(diff_rect(&prev, &cur, w), Some((0, 0, 3, 1)));
    }

    #[test]
    fn crop_rgba_full_canvas() {
        let w = 2;
        let rgba: Vec<u8> = (0..w * 2 * 4).map(|i| i as u8).collect();
        let out = crop_rgba(&rgba, w, 0, 0, w, 2);
        assert_eq!(out, rgba);
    }

    #[test]
    fn crop_rgba_single_pixel() {
        let w = 3;
        let rgba: Vec<u8> = (0..w * 2 * 4).map(|i| i as u8).collect();
        let out = crop_rgba(&rgba, w, 1, 1, 2, 2);
        // Row 1, pixel 1 → bytes 16..20
        assert_eq!(out, vec![16, 17, 18, 19]);
    }

    #[test]
    fn delay_clock_single_frame() {
        let mut c = DelayClock::new(10);
        assert_eq!(c.delay(1), 10);
    }

    #[test]
    fn delay_clock_zero_fps_uses_one() {
        // fps=0 should be treated as fps=1
        let mut c = DelayClock::new(0);
        let d = c.delay(1);
        assert!(d > 0);
    }
}
