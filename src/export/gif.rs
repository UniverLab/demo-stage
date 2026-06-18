//! GIF target (pure Rust): rasterize frames via [`raster`], dedupe identical
//! consecutive frames (accumulating their delay), and encode with `gif`. No
//! ffmpeg required.

use std::path::Path;

use super::raster;
use super::run::Recording;
use crate::error::{Error, Result};
use crate::model::Score;

/// Run a terminal score and write an animated GIF to `path`.
pub fn write_gif(rec: &Recording, score: &Score, path: &Path) -> Result<()> {
    let plan = raster::plan(rec, score);
    let (w, h) = (plan.width, plan.height);
    let frame_cs = (100.0 / plan.fps as f64).round().max(2.0) as u16;

    let file = std::fs::File::create(path).map_err(|e| Error::io(path, e))?;
    let mut encoder = gif::Encoder::new(file, w as u16, h as u16, &[])
        .map_err(|e| Error::Export(format!("gif encoder: {e}")))?;
    encoder
        .set_repeat(gif::Repeat::Infinite)
        .map_err(|e| Error::Export(format!("gif repeat: {e}")))?;

    // Hold the current distinct frame, accumulating delay until it changes.
    let mut held: Option<(Vec<u8>, u16)> = None;
    let mut err: Option<Error> = None;

    raster::render_frames(rec, score, |rgba| {
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
