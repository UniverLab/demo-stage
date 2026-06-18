//! MP4 target (H.264 via ffmpeg, auto-provisioned). `encode` is source-agnostic,
//! so both the single-terminal fast path and the multi-scene stage feed it.

use std::io::Write;
use std::path::Path;

use ffmpeg_sidecar::command::FfmpegCommand;

use super::run::Recording;
use super::{provision, raster};
use crate::error::{Error, Result};
use crate::model::Score;

/// Encode an MP4 at `path` from frames produced by `render` (each `w`×`h` RGBA).
/// ffmpeg is provisioned on first use.
pub fn encode(
    path: &Path,
    w: usize,
    h: usize,
    fps: u32,
    render: impl FnOnce(&mut dyn FnMut(&[u8])) -> Result<()>,
) -> Result<()> {
    provision::ensure_ffmpeg()?;

    let size = format!("{w}x{h}");
    let fps = fps.to_string();
    let out = path.to_string_lossy().to_string();

    let mut child = FfmpegCommand::new()
        .args([
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            &size,
            "-framerate",
            &fps,
            "-i",
            "-",
        ])
        .args([
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-y",
            &out,
        ])
        .spawn()
        .map_err(|e| Error::Export(format!("spawn ffmpeg: {e}")))?;

    let mut stdin = child
        .take_stdin()
        .ok_or_else(|| Error::Export("ffmpeg stdin unavailable".to_string()))?;

    let mut write_err: Option<Error> = None;
    render(&mut |frame: &[u8]| {
        if write_err.is_some() {
            return;
        }
        if let Err(e) = stdin.write_all(frame) {
            write_err = Some(Error::Export(format!("piping frame to ffmpeg: {e}")));
        }
    })?;
    drop(stdin);

    let status = child
        .wait()
        .map_err(|e| Error::Export(format!("ffmpeg: {e}")))?;
    if let Some(e) = write_err {
        return Err(e);
    }
    if !status.success() {
        return Err(Error::Export("ffmpeg failed to encode the mp4".to_string()));
    }
    Ok(())
}

/// Single-terminal fast path: encode an MP4 straight from a recording.
pub fn write_mp4(rec: &Recording, score: &Score, path: &Path) -> Result<()> {
    let plan = raster::plan(rec, score);
    encode(path, plan.width, plan.height, plan.fps, |emit| {
        raster::render_frames(rec, score, |f| emit(f)).map(|_| ())
    })
}
