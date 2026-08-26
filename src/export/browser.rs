//! Browser pane source: drive a headless Chromium to capture a web/PDF scene as
//! RGBA keyframes. Chromium is provisioned tectonic-style — a system install is
//! used if present, otherwise `headless_chrome`'s fetcher downloads a managed
//! build on first use.
//!
//! NOTE: this path needs a real Chromium and so is NOT exercised by the test
//! suite or in the restricted dev sandbox; it is verified on a machine with
//! Chromium available (or network to fetch it).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use headless_chrome::protocol::cdp::Emulation::{MediaFeature, SetEmulatedMedia};
use headless_chrome::protocol::cdp::Page::{CaptureScreenshotFormatOption, Navigate};
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

/// A frame source backed by a directory of PNGs decoded on demand.
/// Keeps at most one frame decoded at a time; the consumer reads progress
/// monotonically, so a one-frame cache plus re-decode is sufficient.
#[derive(Debug)]
pub struct DirScene {
    width: usize,
    height: usize,
    files: Vec<PathBuf>,
    progresses: Vec<f64>,
    cached_index: Option<usize>,
    cached_frame: Vec<u8>,
    #[cfg(test)]
    decode_count: usize,
}

impl DirScene {
    fn new(width: usize, height: usize, files: Vec<PathBuf>, progresses: Vec<f64>) -> Self {
        Self {
            width,
            height,
            files,
            progresses,
            cached_index: None,
            cached_frame: Vec::new(),
            #[cfg(test)]
            decode_count: 0,
        }
    }

    fn frame_at(&mut self, progress: f64) -> &[u8] {
        let idx = self.pick_index(progress);
        if self.cached_index != Some(idx) {
            let bytes = std::fs::read(&self.files[idx])
                .unwrap_or_else(|e| panic!("read {}: {e}", self.files[idx].display()));
            let rgba = png_to_rgba(&bytes, self.width, self.height)
                .unwrap_or_else(|e| panic!("decode {}: {e}", self.files[idx].display()));
            self.cached_index = Some(idx);
            self.cached_frame = rgba;
            #[cfg(test)]
            {
                self.decode_count += 1;
            }
        }
        &self.cached_frame
    }

    fn pick_index(&self, progress: f64) -> usize {
        let mut chosen = 0;
        for (i, p) in self.progresses.iter().enumerate() {
            if *p <= progress {
                chosen = i;
            } else {
                break;
            }
        }
        chosen
    }

    #[cfg(test)]
    fn decode_count(&self) -> usize {
        self.decode_count
    }
}

/// A browser scene backed by either in-memory keyframes, a directory of
/// PNGs decoded on demand, or a native PDF scene that computes viewport
/// slices on demand.
pub enum AnyScene {
    Keyframe(Scene),
    Directory(DirScene),
    Pdf(super::pdf::PdfScene),
}

impl AnyScene {
    pub fn width(&self) -> usize {
        match self {
            Self::Keyframe(s) => s.width,
            Self::Directory(d) => d.width,
            Self::Pdf(p) => p.width(),
        }
    }

    pub fn height(&self) -> usize {
        match self {
            Self::Keyframe(s) => s.height,
            Self::Directory(d) => d.height,
            Self::Pdf(p) => p.height(),
        }
    }

    pub fn frame_at(&mut self, progress: f64) -> &[u8] {
        match self {
            Self::Keyframe(s) => s.frame_at(progress),
            Self::Directory(d) => d.frame_at(progress),
            Self::Pdf(p) => p.frame_at(progress),
        }
    }
}

impl From<Scene> for AnyScene {
    fn from(s: Scene) -> Self {
        Self::Keyframe(s)
    }
}

/// Render a browser pane's `url`, emitting a scene that covers `output_frames`
/// of output. `scroll_keyframes` is the duration-derived Chromium scroll count
/// (unchanged from before this spec). `should_scroll` controls whether a native
/// PDF pane pans through its document (a pane with no scroll step stays static).
pub fn capture(
    pane: &Pane,
    scroll_keyframes: usize,
    output_frames: usize,
    should_scroll: bool,
    fps: f64,
) -> Result<AnyScene> {
    let url = pane
        .url
        .as_deref()
        .ok_or_else(|| Error::Export(format!("browser pane '{}' has no url", pane.id)))?;
    let (w, h) = (pane.width as usize, pane.height as usize);

    // A `--view` scene is already recorded — play its frames back, no Chromium.
    if let Some(dir) = view_frames_dir(url) {
        return load_frames(Path::new(dir), w, h).map(AnyScene::Directory);
    }

    let url = crate::paths::resolve_browser_url(url)?;

    // PDFs render natively (hayro) — no Chromium launch, no blank-viewer risk,
    // and the scene starts instantly. Chrome's viewer is only a fallback.
    if url.to_lowercase().ends_with(".pdf") {
        match local_file_path(&url)
            .and_then(|p| super::pdf::capture_scene(&p, w, h, output_frames, should_scroll, fps))
        {
            Ok(scene) => return Ok(AnyScene::Pdf(scene)),
            Err(e) => {
                eprintln!("demo: native PDF render failed ({e}), falling back to Chrome viewer");
            }
        }
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

    let is_pdf = url.to_lowercase().ends_with(".pdf");

    if is_pdf {
        // Fallback: Chrome's PDF viewer (native hayro render failed above; the
        // viewer may still paint blank in headless mode on some systems).
        let _ = tab.call_method(Navigate {
            url: url.to_string(),
            referrer: None,
            transition_Type: None,
            frame_id: None,
            referrer_policy: None,
        });
        std::thread::sleep(Duration::from_millis(3000));
    } else {
        tab.navigate_to(&url)
            .and_then(|t| t.wait_until_navigated())
            .map_err(|e| Error::Export(format!("navigate to {url}: {e}")))?;
        // Give the page a moment to paint.
        std::thread::sleep(Duration::from_millis(900));
    }

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

    Ok(AnyScene::Keyframe(Scene {
        width: w,
        height: h,
        keyframes,
    }))
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
    let url = crate::paths::resolve_browser_url(url)?;

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
    // Force the viewport to the exact requested dimensions (matches `capture`).
    let _ = tab.set_bounds(headless_chrome::types::Bounds::Normal {
        left: Some(0),
        top: Some(0),
        width: Some(width as f64),
        height: Some(height as f64),
    });
    emulate_theme(&tab, theme);

    let is_pdf = url.to_lowercase().ends_with(".pdf");

    if is_pdf {
        let _ = tab.call_method(Navigate {
            url: url.to_string(),
            referrer: None,
            transition_Type: None,
            frame_id: None,
            referrer_policy: None,
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            if let Ok(val) =
                tab.evaluate("!!document.querySelector('embed, object, iframe')", false)
            {
                if val.value.as_ref().and_then(|v| v.as_bool()) == Some(true) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        std::thread::sleep(Duration::from_millis(1500));
    } else {
        tab.navigate_to(&url)
            .and_then(|t| t.wait_until_navigated())
            .map_err(|e| Error::Export(format!("navigate to {url}: {e}")))?;
    }

    eprintln!(
        "● recording the browser — navigate freely, then CLOSE THE WINDOW to finish the scene"
    );
    let delay = Duration::from_millis((1000 / VIEW_FPS.max(1)) as u64);
    let max_frames = VIEW_FPS as usize * 300; // 5-minute safety cap
    let mut n = 0usize;
    loop {
        // A failed screenshot means the tab/window was closed — that's the cue to
        // stop recording. No clip: clip x/y are document coordinates, so once the
        // user scrolls, a (0,0) clip would capture unrasterized background.
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
/// directory-backed frame source sized to `tw`×`th`. Frames are decoded on
/// demand; only one frame is held decoded at a time.
fn load_frames(dir: &Path, tw: usize, th: usize) -> Result<DirScene> {
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
    let progresses: Vec<f64> = (0..count)
        .map(|i| {
            if count > 1 {
                i as f64 / (count - 1) as f64
            } else {
                0.0
            }
        })
        .collect();
    Ok(DirScene::new(tw, th, files, progresses))
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

/// Extract the local filesystem path from a `file:///path` or
/// `http://127.0.0.1:PORT/path` URL (the local server's root is `/`, so its
/// request path *is* the filesystem path).
fn local_file_path(url: &str) -> Result<std::path::PathBuf> {
    let local_path = if let Some(rest) = url.strip_prefix("file://") {
        rest.to_string()
    } else if let Some(rest) = url.strip_prefix("http://127.0.0.1:") {
        match rest.find('/') {
            Some(slash) => rest[slash..].to_string(),
            None => {
                return Err(Error::Export(format!("can't extract path from URL: {url}")));
            }
        }
    } else {
        return Err(Error::Export(format!("can't extract path from URL: {url}")));
    };
    let path = std::path::PathBuf::from(&local_path);
    if !path.exists() {
        return Err(Error::Export(format!("file not found: {local_path}")));
    }
    Ok(path)
}

fn shot(tab: &Arc<Tab>, w: usize, h: usize) -> Result<Vec<u8>> {
    // No clip: a clip's x/y are DOCUMENT coordinates, so after a scroll the
    // clipped region is off-screen and Chrome returns unrasterized background.
    // The viewport is already forced to w×h; png_to_rgba crops/pads any excess.
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

    #[test]
    fn frame_at_single_keyframe() {
        let scene = Scene {
            width: 1,
            height: 1,
            keyframes: vec![(0.0, vec![42])],
        };
        assert_eq!(scene.frame_at(0.0), &[42]);
        assert_eq!(scene.frame_at(1.0), &[42]);
    }

    #[test]
    fn frame_at_boundary_between_keyframes() {
        let scene = Scene {
            width: 1,
            height: 1,
            keyframes: vec![(0.0, vec![1]), (0.5, vec![2])],
        };
        assert_eq!(scene.frame_at(0.5), &[2]);
        assert_eq!(scene.frame_at(0.499), &[1]);
    }

    #[test]
    fn local_file_path_extracts_from_file_url() {
        // Non-existent file should error (file doesn't exist)
        let result = super::local_file_path("file:///tmp/nonexistent_test_file_12345.pdf");
        assert!(result.is_err());
    }

    #[test]
    fn local_file_path_extracts_from_localhost_url() {
        // Non-existent file should error
        let result = super::local_file_path("http://127.0.0.1:8080/nonexistent.pdf");
        assert!(result.is_err());
    }

    #[test]
    fn local_file_path_rejects_non_local_url() {
        let result = super::local_file_path("https://example.com/page.html");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("can't extract path"));
    }

    #[test]
    fn local_file_path_rejects_localhost_without_path() {
        let result = super::local_file_path("http://127.0.0.1:8080");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("can't extract path"));
    }

    #[test]
    fn local_file_path_rejects_file_url_without_path() {
        let result = super::local_file_path("file://");
        assert!(result.is_err());
    }

    #[test]
    fn local_file_path_rejects_ftp_url() {
        let result = super::local_file_path("ftp://example.com/file.pdf");
        assert!(result.is_err());
    }

    #[test]
    fn from_keyframes_builds_scene() {
        let scene = Scene {
            width: 100,
            height: 200,
            keyframes: vec![(0.0, vec![1, 2, 3])],
        };
        assert_eq!(scene.width, 100);
        assert_eq!(scene.height, 200);
        assert_eq!(scene.frame_at(0.5), &[1, 2, 3]);
    }

    #[test]
    fn from_keyframes_empty_keyframes() {
        let scene = Scene {
            width: 10,
            height: 10,
            keyframes: vec![],
        };
        assert_eq!(scene.width, 10);
        assert_eq!(scene.height, 10);
    }

    #[test]
    fn png_to_rgba_rejects_invalid_png() {
        let result = super::png_to_rgba(&[0, 1, 2, 3], 10, 10);
        assert!(result.is_err());
    }

    #[test]
    fn png_to_rgba_converts_valid_png() {
        // Create a minimal 2x2 RGBA PNG using png crate properly
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut encoder = png::Encoder::new(&mut buf, 2, 2);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let pixels: Vec<u8> = vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ];
            writer.write_image_data(&pixels).unwrap();
            writer.finish().unwrap();
        }
        let png_bytes = buf.into_inner();

        let rgba = super::png_to_rgba(&png_bytes, 2, 2).unwrap();
        assert_eq!(rgba.len(), 2 * 2 * 4);
        // Top-left pixel should be red
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
        // Top-right pixel should be green
        assert_eq!(&rgba[4..8], &[0, 255, 0, 255]);
    }

    #[test]
    fn png_to_rgba_crops_to_target_size() {
        // Create a 4x4 RGB PNG
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut encoder = png::Encoder::new(&mut buf, 4, 4);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let pixels: Vec<u8> = (0..4 * 4 * 3).map(|i| i as u8).collect();
            writer.write_image_data(&pixels).unwrap();
            writer.finish().unwrap();
        }
        let png_bytes = buf.into_inner();

        // Target is 2x2 — should crop to top-left
        let rgba = super::png_to_rgba(&png_bytes, 2, 2).unwrap();
        assert_eq!(rgba.len(), 2 * 2 * 4);
        // All alpha should be 255
        for px in rgba.as_chunks::<4>().0 {
            assert_eq!(px[3], 255);
        }
    }

    #[test]
    fn png_to_rgba_pads_when_target_larger() {
        // Create a 1x1 RGB PNG
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut encoder = png::Encoder::new(&mut buf, 1, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[255, 128, 64]).unwrap();
            writer.finish().unwrap();
        }
        let png_bytes = buf.into_inner();

        // Target is 3x3 — should have the pixel at (0,0) and zeros elsewhere
        let rgba = super::png_to_rgba(&png_bytes, 3, 3).unwrap();
        assert_eq!(rgba.len(), 3 * 3 * 4);
        assert_eq!(&rgba[0..4], &[255, 128, 64, 255]);
        // Second pixel should be black
        assert_eq!(&rgba[4..8], &[0, 0, 0, 0]);
    }

    #[test]
    fn png_to_rgba_rejects_unsupported_color_type() {
        // Create a 1x1 grayscale PNG
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut encoder = png::Encoder::new(&mut buf, 1, 1);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[128]).unwrap();
            writer.finish().unwrap();
        }
        let png_bytes = buf.into_inner();

        let result = super::png_to_rgba(&png_bytes, 1, 1);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unsupported screenshot format"));
    }

    #[test]
    fn view_fps_constant() {
        assert_eq!(super::VIEW_FPS, 8);
    }

    #[test]
    fn scene_from_keyframes_preserves_dimensions() {
        let scene = Scene {
            width: 640,
            height: 480,
            keyframes: vec![],
        };
        assert_eq!(scene.width, 640);
        assert_eq!(scene.height, 480);
    }

    #[test]
    fn frame_at_negative_progress_returns_first() {
        let scene = Scene {
            width: 1,
            height: 1,
            keyframes: vec![(0.0, vec![10]), (0.5, vec![20])],
        };
        assert_eq!(scene.frame_at(-1.0), &[10]);
    }

    fn make_test_png(r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut encoder = png::Encoder::new(&mut buf, 1, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[r, g, b]).unwrap();
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    struct TmpDir(std::path::PathBuf);
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_dir_scene(n: usize) -> (TmpDir, super::DirScene) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "demostage_test_dir_scene_{}_{}",
            std::process::id(),
            id
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..n {
            let png = make_test_png(i as u8, 0, 0);
            let path = dir.join(format!("{:04}.png", i + 1));
            std::fs::write(&path, &png).unwrap();
        }
        let scene = super::load_frames(&dir, 1, 1).unwrap();
        (TmpDir(dir), scene)
    }

    #[test]
    fn dir_scene_frame_at_several_progress_values() {
        let (_dir, mut scene) = make_dir_scene(10);
        // progress 0.0 → first frame (index 0, red=0)
        let f = scene.frame_at(0.0);
        assert_eq!(f[0], 0);
        // progress at exact boundary (index 5, progress = 5/9 ≈ 0.555)
        let f = scene.frame_at(5.0 / 9.0);
        assert_eq!(f[0], 5);
        // progress beyond 1.0 → last frame (index 9, red=9)
        let f = scene.frame_at(2.0);
        assert_eq!(f[0], 9);
        // progress just before a boundary
        let f = scene.frame_at(0.5);
        // 0.5 < 5/9 ≈ 0.555, so index 4 (progress = 4/9 ≈ 0.444)
        assert_eq!(f[0], 4);
    }

    #[test]
    fn dir_scene_decode_count_is_bounded() {
        let (_dir, mut scene) = make_dir_scene(100);
        // Walk progress monotonically from 0 to 1 in 50 steps.
        // We land on at most 50 unique frame indices, so decode count <= 50.
        // Crucially, it is NOT 100 (we don't decode every frame).
        for step in 0..50 {
            let progress = step as f64 / 49.0;
            let _ = scene.frame_at(progress);
        }
        let decodes = scene.decode_count();
        assert!(
            decodes <= 50,
            "expected at most 50 decodes for 50 steps, got {decodes}"
        );
        assert!(decodes > 0, "expected at least one decode");
    }

    #[test]
    fn dir_scene_empty_directory_errors() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "demostage_test_empty_{}_{}",
            std::process::id(),
            id
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let result = super::load_frames(&dir, 100, 100);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no recorded frames"));
        assert!(err.contains("--view"));
    }

    #[test]
    fn dir_scene_single_frame() {
        let (_dir, mut scene) = make_dir_scene(1);
        // Single frame: progress is always 0.0
        let f = scene.frame_at(0.0);
        assert_eq!(f[0], 0);
        let f = scene.frame_at(1.0);
        assert_eq!(f[0], 0);
    }
}
