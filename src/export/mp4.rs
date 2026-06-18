//! MP4 target (H.264 via ffmpeg). ffmpeg is auto-provisioned on first use
//! (see [`super::provision::ensure_ffmpeg`]); rasterized frames are piped to it.

use std::io::Write;
use std::path::Path;

use ffmpeg_sidecar::command::FfmpegCommand;

use super::run::Recording;
use super::{provision, raster};
use crate::error::{Error, Result};
use crate::model::Score;

/// Run a terminal score and encode it to an MP4 at `path`.
pub fn write_mp4(rec: &Recording, score: &Score, path: &Path) -> Result<()> {
    provision::ensure_ffmpeg()?;

    let plan = raster::plan(rec, score);
    let size = format!("{}x{}", plan.width, plan.height);
    let fps = plan.fps.to_string();
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
    raster::render_frames(rec, score, |frame| {
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
