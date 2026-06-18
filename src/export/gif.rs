//! GIF target (pure Rust): dedupe identical consecutive frames and encode with
//! `gif`. `encode` is source-agnostic, so both the single-terminal fast path and
//! the multi-scene stage feed it. No ffmpeg.

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
    let frame_cs = (100.0 / fps as f64).round().max(2.0) as u16;
    let file = std::fs::File::create(path).map_err(|e| Error::io(path, e))?;
    let mut encoder = gif::Encoder::new(file, w as u16, h as u16, &[])
        .map_err(|e| Error::Export(format!("gif encoder: {e}")))?;
    encoder
        .set_repeat(gif::Repeat::Infinite)
        .map_err(|e| Error::Export(format!("gif repeat: {e}")))?;

    let mut held: Option<(Vec<u8>, u16)> = None;
    let mut err: Option<Error> = None;

    render(&mut |rgba: &[u8]| {
        if err.is_some() {
            return;
        }
        match held.as_mut() {
            Some((prev, delay)) if prev.as_slice() == rgba => {
                *delay = delay.saturating_add(frame_cs);
            }
            _ => {
                if let Some((prev, delay)) = held.take() {
                    if let Err(e) = write_frame(&mut encoder, prev, w, h, delay) {
                        err = Some(e);
                        return;
                    }
                }
                held = Some((rgba.to_vec(), frame_cs));
            }
        }
    })?;

    if let Some(e) = err {
        return Err(e);
    }
    if let Some((prev, delay)) = held.take() {
        write_frame(&mut encoder, prev, w, h, delay)?;
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

fn write_frame(
    encoder: &mut gif::Encoder<std::fs::File>,
    mut rgba: Vec<u8>,
    w: usize,
    h: usize,
    delay_cs: u16,
) -> Result<()> {
    let mut frame = gif::Frame::from_rgba_speed(w as u16, h as u16, &mut rgba, 10);
    frame.delay = delay_cs;
    encoder
        .write_frame(&frame)
        .map_err(|e| Error::Export(format!("gif frame: {e}")))
}
