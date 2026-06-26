//! Browser pane source: drive a headless Chromium to capture a web/PDF scene as
//! RGBA keyframes. Chromium is provisioned tectonic-style — a system install is
//! used if present, otherwise `headless_chrome`'s fetcher downloads a managed
//! build on first use.
//!
//! NOTE: this path needs a real Chromium and so is NOT exercised by the test
//! suite or in the restricted dev sandbox; it is verified on a machine with
//! Chromium available (or network to fetch it).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use headless_chrome::protocol::cdp::Emulation::{MediaFeature, SetEmulatedMedia};
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use headless_chrome::protocol::cdp::Page;
use headless_chrome::{Browser, LaunchOptions, Tab};

use super::provision;
use crate::error::{Error, Result};
use crate::model::{view_frames_dir, Pane};

/// Frame rate an interactive `--view` session is recorded at.
pub const VIEW_FPS: u32 = 8;

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

    // A `--view` scene is already recorded — play its frames back, no Chromium.
    if let Some(dir) = view_frames_dir(url) {
        return load_frames(Path::new(dir), w, h);
    }

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
            "launch chromium: {e}\n  \
             • If this says \"no available ports … for debugging\", you have the \
             *snap* Chromium (Ubuntu's default). Its sandbox blocks the remote-debug \
             port, so it can't drive headless. Install the non-snap Google Chrome \
             instead:\n      \
             wget https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb\n      \
             sudo apt install ./google-chrome-stable_current_amd64.deb\n    \
             (it's picked automatically over the snap).\n  \
             • Or run the export on a host with a browser — the `.rec` is portable, \
             so `demo export demo.rec` from Windows works.\n  \
             • Or capture the scene with `demo open --view` (records frames up front, \
             so export needs no browser)."
        ))
    })?;

    let tab = browser
        .new_tab()
        .map_err(|e| Error::Export(format!("open tab: {e}")))?;
    // Force the viewport to the exact pane dimensions.
    let _ = tab.set_bounds(headless_chrome::types::Bounds::Normal {
        left: Some(0),
        top: Some(0),
        width: Some(w as f64),
        height: Some(h as f64),
    });
    emulate_theme(&tab, pane.theme.as_deref());
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

/// Record an **interactive** browsing session: open a real (headed) browser at
/// `url`, let the user navigate, and save a PNG frame every `1/`[`VIEW_FPS`] until
/// they close the window. Frames are written `0001.png`, `0002.png`, … into
/// `out_dir`; returns how many were captured. Used by `demo open --view`.
pub fn record_view(
    url: &str,
    width: u32,
    height: u32,
    theme: Option<&str>,
    out_dir: &Path,
) -> Result<usize> {
    std::fs::create_dir_all(out_dir).map_err(|e| Error::io(out_dir, e))?;
    if provision::find_chromium().is_none() {
        eprintln!("demo: Chromium not found — fetching a managed copy (one time)…");
    }

    let options = LaunchOptions::default_builder()
        .headless(false)
        .sandbox(false)
        .window_size(Some((width, height)))
        .build()
        .map_err(|e| Error::Export(format!("chromium launch options: {e}")))?;
    let browser = Browser::new(options).map_err(|e| {
        Error::Export(format!(
            "launch a visible browser: {e}\n  \
             • `--view` opens a real (headed) browser, so it needs a graphical \
             display — on WSL that means WSLg (a recent Windows 11).\n  \
             • If it says \"no available ports … for debugging\", you have the \
             *snap* Chromium — its sandbox blocks the debug port `--view` drives it \
             through. Install the non-snap Google Chrome:\n      \
             wget https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb\n      \
             sudo apt install ./google-chrome-stable_current_amd64.deb"
        ))
    })?;

    let tab = browser
        .new_tab()
        .map_err(|e| Error::Export(format!("open tab: {e}")))?;
    emulate_theme(&tab, theme);
    tab.navigate_to(url)
        .and_then(|t| t.wait_until_navigated())
        .map_err(|e| Error::Export(format!("navigate to {url}: {e}")))?;

    eprintln!(
        "● recording the browser — navigate freely, then CLOSE THE WINDOW to finish the scene"
    );
    let delay = Duration::from_millis((1000 / VIEW_FPS.max(1)) as u64);
    let max_frames = VIEW_FPS as usize * 300; // 5-minute safety cap
    let mut n = 0usize;
    loop {
        // A failed screenshot means the tab/window was closed — that's the cue to
        // stop recording.
        let Ok(png) = tab.capture_screenshot(CaptureScreenshotFormatOption::Png, None, None, true)
        else {
            break;
        };
        let path = out_dir.join(format!("{:04}.png", n + 1));
        if std::fs::write(&path, &png).is_err() {
            break;
        }
        n += 1;
        if n >= max_frames {
            break;
        }
        std::thread::sleep(delay);
    }
    Ok(n)
}

/// Load a `--view` scene's pre-recorded PNG frames (`NNNN.png`, sorted) as a
/// time-spread keyframe sequence sized to `tw`×`th`.
fn load_frames(dir: &Path, tw: usize, th: usize) -> Result<Scene> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| Error::io(dir, e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(Error::Export(format!(
            "no recorded frames in {} (was the --view window closed before anything rendered?)",
            dir.display()
        )));
    }

    let count = files.len();
    let mut keyframes = Vec::with_capacity(count);
    for (i, f) in files.iter().enumerate() {
        let bytes = std::fs::read(f).map_err(|e| Error::io(f, e))?;
        let rgba = png_to_rgba(&bytes, tw, th)?;
        let progress = if count > 1 {
            i as f64 / (count - 1) as f64
        } else {
            0.0
        };
        keyframes.push((progress, rgba));
    }
    Ok(Scene {
        width: tw,
        height: th,
        keyframes,
    })
}

/// Emulate `prefers-color-scheme` (`light`/`dark`) so theme-aware pages render the
/// chosen theme. A no-op when `theme` is `None`; failures are non-fatal.
fn emulate_theme(tab: &Arc<Tab>, theme: Option<&str>) {
    if let Some(scheme) = theme {
        let _ = tab.call_method(SetEmulatedMedia {
            media: None,
            features: Some(vec![MediaFeature {
                name: "prefers-color-scheme".to_string(),
                value: scheme.to_string(),
            }]),
        });
    }
}

fn shot(tab: &Arc<Tab>, w: usize, h: usize) -> Result<Vec<u8>> {
    // clip with scale=1.0 produces exactly w×h pixels regardless of device DPI.
    let viewport = Page::Viewport {
        x: 0.0,
        y: 0.0,
        width: w as f64,
        height: h as f64,
        scale: 1.0,
    };
    let png = tab
        .capture_screenshot(CaptureScreenshotFormatOption::Png, None, Some(viewport), true)
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
