//! Browser pane source: drive a headless Chromium to capture a web/PDF scene as
//! RGBA keyframes. Chromium is provisioned tectonic-style — a system install is
//! used if present, otherwise `headless_chrome`'s fetcher downloads a managed
//! build on first use.
//!
//! NOTE: this path needs a real Chromium and so is NOT exercised by the test
//! suite or in the restricted dev sandbox; it is verified on a machine with
//! Chromium available (or network to fetch it).

use std::sync::Arc;
use std::time::Duration;

use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use headless_chrome::{Browser, LaunchOptions, Tab};

use super::provision;
use crate::error::{Error, Result};
use crate::model::Pane;

/// A captured browser scene: RGBA keyframes keyed by timeline progress in
/// `[0.0, 1.0]`. Frames between keyframes hold the latest one.
pub struct Scene {
    pub width: usize,
    pub height: usize,
    keyframes: Vec<(f64, Vec<u8>)>,
}

impl Scene {
    /// The frame to show at `progress` (the latest keyframe at or before it).
    pub fn frame_at(&self, progress: f64) -> &[u8] {
        let mut chosen = &self.keyframes[0].1;
        for (p, f) in &self.keyframes {
            if *p <= progress {
                chosen = f;
            } else {
                break;
            }
        }
        chosen
    }
}

/// Render a browser pane's `url`, capturing `scroll_keyframes` extra frames while
/// scrolling down (0 = a single static frame).
pub fn capture(pane: &Pane, scroll_keyframes: usize) -> Result<Scene> {
    let url = pane
        .url
        .as_deref()
        .ok_or_else(|| Error::Export(format!("browser pane '{}' has no url", pane.id)))?;
    let (w, h) = (pane.width as usize, pane.height as usize);

    if provision::find_chromium().is_none() {
        eprintln!("demo: Chromium not found — fetching a managed copy (one time)…");
    }

    let options = LaunchOptions::default_builder()
        .headless(true)
        // Headless WSL/CI hosts usually run without a working Chromium sandbox,
        // where it refuses to start — disable it.
        .sandbox(false)
        .window_size(Some((pane.width, pane.height)))
        .build()
        .map_err(|e| Error::Export(format!("chromium launch options: {e}")))?;
    let browser = Browser::new(options).map_err(|e| {
        Error::Export(format!(
            "launch chromium: {e}\n  A browser scene needs a working headless \
             Chromium, which commonly fails on a headless WSL/server host. Fixes:\n  \
             • install Chromium (e.g. `sudo apt install chromium`), or\n  \
             • run the export on a host with a browser — the `.rec` is portable, so \
             `demo export demo.rec` from Windows works."
        ))
    })?;

    let tab = browser
        .new_tab()
        .map_err(|e| Error::Export(format!("open tab: {e}")))?;
    tab.navigate_to(url)
        .and_then(|t| t.wait_until_navigated())
        .map_err(|e| Error::Export(format!("navigate to {url}: {e}")))?;
    // Give the page (or PDF viewer) a moment to paint.
    std::thread::sleep(Duration::from_millis(900));

    let mut keyframes = vec![(0.0, shot(&tab, w, h)?)];

    for i in 0..scroll_keyframes {
        // window.scrollBy for web pages; PageDown also drives Chrome's PDF viewer.
        let _ = tab.evaluate(
            "window.scrollBy(0, Math.round(window.innerHeight * 0.85));",
            false,
        );
        let _ = tab.press_key("PageDown");
        std::thread::sleep(Duration::from_millis(350));
        let progress = 0.5 + 0.5 * ((i + 1) as f64 / scroll_keyframes as f64);
        keyframes.push((progress, shot(&tab, w, h)?));
    }

    Ok(Scene {
        width: w,
        height: h,
        keyframes,
    })
}

fn shot(tab: &Arc<Tab>, w: usize, h: usize) -> Result<Vec<u8>> {
    let png = tab
        .capture_screenshot(CaptureScreenshotFormatOption::Png, None, None, true)
        .map_err(|e| Error::Export(format!("screenshot: {e}")))?;
    png_to_rgba(&png, w, h)
}

/// Decode a PNG screenshot into a `tw`×`th` RGBA buffer (cropping/padding to fit).
fn png_to_rgba(bytes: &[u8], tw: usize, th: usize) -> Result<Vec<u8>> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder
        .read_info()
        .map_err(|e| Error::Export(format!("png: {e}")))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| Error::Export(format!("png: {e}")))?;

    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        other => {
            return Err(Error::Export(format!(
                "unsupported screenshot format: {other:?}"
            )))
        }
    };
    let (sw, sh) = (info.width as usize, info.height as usize);

    let mut out = vec![0u8; tw * th * 4];
    for y in 0..th.min(sh) {
        for x in 0..tw.min(sw) {
            let s = (y * sw + x) * channels;
            let d = (y * tw + x) * 4;
            out[d] = buf[s];
            out[d + 1] = buf[s + 1];
            out[d + 2] = buf[s + 2];
            out[d + 3] = 255;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::Scene;

    #[test]
    fn frame_at_picks_the_latest_keyframe() {
        let scene = Scene {
            width: 1,
            height: 1,
            keyframes: vec![(0.0, vec![1]), (0.5, vec![2]), (0.9, vec![3])],
        };
        assert_eq!(scene.frame_at(0.0), &[1]);
        assert_eq!(scene.frame_at(0.49), &[1]);
        assert_eq!(scene.frame_at(0.5), &[2]);
        assert_eq!(scene.frame_at(0.95), &[3]);
        assert_eq!(scene.frame_at(2.0), &[3]);
    }
}
