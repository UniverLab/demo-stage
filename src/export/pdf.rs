//! Pure-Rust PDF pane capture: rasterize the pages with hayro, stack them on a
//! viewer-style backdrop, and slice viewport windows at even scroll offsets.
//! No Chromium, no temp files, no HTTP server — a PDF pane renders in-process,
//! so it starts instantly and scrolls through the whole document.

use std::path::Path;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, RenderCache, RenderSettings};

use super::browser::Scene;
use crate::error::{Error, Result};

/// Backdrop behind/between pages (Chrome PDF viewer gray).
const BACKDROP: [u8; 4] = [0x52, 0x56, 0x59, 0xff];
/// Vertical gap between pages and at the top/bottom, in px.
const GAP: usize = 12;
/// Page width as a fraction of the pane width (fit-width with side margins).
const PAGE_FRAC: f64 = 0.90;

/// Capture a PDF as a browser-pane [`Scene`]: keyframe 0 shows the top of the
/// document, and `scroll_keyframes` more pan evenly down to the last page.
pub fn capture_scene(
    pdf_path: &Path,
    pane_w: usize,
    pane_h: usize,
    scroll_keyframes: usize,
) -> Result<Scene> {
    let data = std::fs::read(pdf_path).map_err(|e| Error::io(pdf_path, e))?;
    let pdf = Pdf::new(data).map_err(|e| Error::Export(format!("read PDF: {e:?}")))?;

    let page_w = ((pane_w as f64) * PAGE_FRAC).round() as usize;
    let cache = RenderCache::new();
    let settings = InterpreterSettings::default();

    // Rasterize each page at fit-width scale.
    let mut pages: Vec<(usize, usize, Vec<u8>)> = Vec::new(); // (w, h, rgba)
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
        // Pages render on an opaque white base, so premultiplied == straight;
        // force alpha anyway for the compositor.
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

    // Stack the pages, centered, into one tall document image.
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

    // A viewport window of the document at vertical offset `off`.
    let slice_at = |off: usize| -> Vec<u8> {
        let mut out = vec![0u8; pane_w * pane_h * 4];
        for px in out.as_chunks_mut::<4>().0 {
            px.copy_from_slice(&BACKDROP);
        }
        let rows = pane_h.min(doc_h.saturating_sub(off));
        for row in 0..rows {
            let s = (off + row) * doc_w * 4;
            let d = row * pane_w * 4;
            out[d..d + doc_w * 4].copy_from_slice(&doc[s..s + doc_w * 4]);
        }
        out
    };

    let max_off = doc_h.saturating_sub(pane_h);
    let mut keyframes = vec![(0.0, slice_at(0))];
    for i in 0..scroll_keyframes {
        let frac = (i + 1) as f64 / scroll_keyframes as f64;
        let off = (max_off as f64 * frac).round() as usize;
        // Same progress mapping as browser scroll capture: hold the top for the
        // first half of the pane's on-screen window, then pan.
        keyframes.push((0.5 + 0.5 * frac, slice_at(off)));
    }

    Ok(Scene::from_keyframes(pane_w, pane_h, keyframes))
}
