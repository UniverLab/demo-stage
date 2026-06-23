//! Shared frame rasterizer for the pixel targets (gif, mp4, and the multi-scene
//! stage). Replays a recording through a vt100 parser at the score's fps and
//! renders each frame to RGBA with the embedded monospace font. Pure Rust;
//! covers printable ASCII, ANSI colours, and every other glyph the capture
//! actually prints (banners, box-drawing, arrows) as long as the font has it.

use std::collections::HashMap;

use fontdue::{Font, FontSettings, Metrics};
use vt100::{Color, Parser};

use super::run::Recording;
use crate::error::{Error, Result};
use crate::model::Score;

/// Embedded monospace font (DejaVu Sans Mono, see assets/FONT-LICENSE.md).
const FONT: &[u8] = include_bytes!("../../assets/DejaVuSansMono.ttf");
const DEFAULT_FG: [u8; 3] = [200, 200, 200];

/// Non-ASCII glyphs cached on top of printable ASCII so they render on the pixel
/// targets — a green-arrow prompt (`❯`) and a few common symbols people use in
/// prompts and captions, so a custom prompt still rasterizes.
const EXTRA_GLYPHS: &[char] = &[
    '❯', '❮', '›', '‹', '»', '«', '→', '←', '▶', '▸', '●', '•', '★', '✓', '✗', 'λ',
];

/// Standard xterm 16-colour ANSI palette.
const ANSI16: [[u8; 3]; 16] = [
    [0, 0, 0],
    [205, 0, 0],
    [0, 205, 0],
    [205, 205, 0],
    [0, 0, 238],
    [205, 0, 205],
    [0, 205, 205],
    [229, 229, 229],
    [127, 127, 127],
    [255, 0, 0],
    [0, 255, 0],
    [255, 255, 0],
    [92, 92, 255],
    [255, 0, 255],
    [0, 255, 255],
    [255, 255, 255],
];

/// The pixel geometry and rate of a render.
pub struct Plan {
    pub width: usize,
    pub height: usize,
    pub fps: u32,
}

fn cell_size(score: &Score) -> (usize, usize) {
    let px = score
        .layout
        .panes
        .iter()
        .find_map(|p| p.font_size)
        .unwrap_or(16) as f32;
    let line_height = score.layout.line_height.max(0.5);
    (
        (px * 0.6).round().max(1.0) as usize,
        (px * line_height).round().max(1.0) as usize,
    )
}

/// Pixel dimensions + fps for a recording, without rendering.
pub fn plan(rec: &Recording, score: &Score) -> Plan {
    let (cw, ch) = cell_size(score);
    Plan {
        width: rec.cols as usize * cw,
        height: rec.rows as usize * ch,
        fps: score.layout.fps.max(1),
    }
}

/// A stateful, frame-by-frame terminal renderer. Advancing it monotonically
/// replays the recording at the score's fps — used directly by gif/mp4 and, in
/// lockstep with other panes, by the multi-scene stage.
pub struct FrameSource<'a> {
    rec: &'a Recording,
    glyphs: HashMap<char, (Metrics, Vec<u8>)>,
    cols: usize,
    rows: usize,
    cell_w: usize,
    cell_h: usize,
    ascent: f32,
    default_bg: [u8; 3],
    parser: Parser,
    ev_idx: usize,
    dt: f64,
    frame: usize,
    n_frames: usize,
    caption: Option<CaptionOverlay>,
}

impl<'a> FrameSource<'a> {
    pub fn new(rec: &'a Recording, score: &Score) -> Result<Self> {
        let font = Font::from_bytes(FONT, FontSettings::default())
            .map_err(|e| Error::Export(format!("font: {e}")))?;
        let px = score
            .layout
            .panes
            .iter()
            .find_map(|p| p.font_size)
            .unwrap_or(16) as f32;
        let (cell_w, cell_h) = cell_size(score);
        let default_bg = score
            .layout
            .background
            .as_deref()
            .and_then(parse_hex)
            .unwrap_or([11, 15, 20]);
        let ascent = font
            .horizontal_line_metrics(px)
            .map(|m| m.ascent)
            .unwrap_or(px * 0.8);

        let mut glyphs = HashMap::new();
        for code in 0x21u8..=0x7e {
            let ch = code as char;
            glyphs.insert(ch, font.rasterize(ch, px));
        }
        // Cache every non-ASCII glyph the capture actually prints (box-drawing,
        // block art like a banner, arrows, accents, …) so it renders here too —
        // not just the printable-ASCII range. Without this, e.g. a banner drawn
        // with `█`/`░` is invisible on the pixel targets while fine in html.
        for (_, chunk) in &rec.events {
            for ch in chunk.chars() {
                if !ch.is_control() && !ch.is_whitespace() {
                    glyphs.entry(ch).or_insert_with(|| font.rasterize(ch, px));
                }
            }
        }

        let fps = score.layout.fps.max(1) as f64;
        let dt = 1.0 / fps;
        let total = rec.duration.max(dt);
        let n_frames = (total / dt).ceil() as usize + 1;

        let caption = if rec.captions.is_empty() {
            None
        } else {
            Some(CaptionOverlay::new(rec.captions.clone(), 18.0)?)
        };

        Ok(FrameSource {
            rec,
            glyphs,
            cols: rec.cols as usize,
            rows: rec.rows as usize,
            cell_w,
            cell_h,
            ascent,
            default_bg,
            parser: Parser::new(rec.rows, rec.cols, 0),
            ev_idx: 0,
            dt,
            frame: 0,
            n_frames,
            caption,
        })
    }

    pub fn n_frames(&self) -> usize {
        self.n_frames
    }

    pub fn dims(&self) -> (usize, usize) {
        (self.cols * self.cell_w, self.rows * self.cell_h)
    }

    /// Render the next frame, or `None` once exhausted.
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        if self.frame >= self.n_frames {
            return None;
        }
        let t = self.frame as f64 * self.dt;
        while self.ev_idx < self.rec.events.len() && self.rec.events[self.ev_idx].0 <= t {
            self.parser
                .process(self.rec.events[self.ev_idx].1.as_bytes());
            self.ev_idx += 1;
        }
        self.frame += 1;
        let mut img = render_cells(
            &self.parser,
            &self.glyphs,
            self.cols,
            self.rows,
            self.cell_w,
            self.cell_h,
            self.ascent,
            self.default_bg,
        );
        // For a single-terminal score the pane frame is the whole canvas, so the
        // caption is drawn here. (The stage clears captions from its terminal
        // source and draws them on the composited canvas instead.)
        if let Some(caption) = &self.caption {
            caption.draw(
                &mut img,
                self.cols * self.cell_w,
                self.rows * self.cell_h,
                t,
            );
        }
        Some(img)
    }
}

/// Render every frame, invoking `on_frame` with each RGBA buffer in order.
pub fn render_frames(
    rec: &Recording,
    score: &Score,
    mut on_frame: impl FnMut(&[u8]),
) -> Result<Plan> {
    let mut source = FrameSource::new(rec, score)?;
    while let Some(frame) = source.next_frame() {
        on_frame(&frame);
    }
    Ok(plan(rec, score))
}

#[allow(clippy::too_many_arguments)]
fn render_cells(
    parser: &Parser,
    glyphs: &HashMap<char, (Metrics, Vec<u8>)>,
    cols: usize,
    rows: usize,
    cell_w: usize,
    cell_h: usize,
    ascent: f32,
    default_bg: [u8; 3],
) -> Vec<u8> {
    let (w, h) = (cols * cell_w, rows * cell_h);
    let mut img = vec![0u8; w * h * 4];
    let screen = parser.screen();

    for row in 0..rows {
        for col in 0..cols {
            let cell = screen.cell(row as u16, col as u16);
            let bg = cell
                .map(|c| resolve(c.bgcolor(), default_bg))
                .unwrap_or(default_bg);
            let fg = cell
                .map(|c| resolve(c.fgcolor(), DEFAULT_FG))
                .unwrap_or(DEFAULT_FG);

            let (x0, y0) = (col * cell_w, row * cell_h);
            for y in 0..cell_h {
                let base = ((y0 + y) * w + x0) * 4;
                for x in 0..cell_w {
                    let p = base + x * 4;
                    img[p] = bg[0];
                    img[p + 1] = bg[1];
                    img[p + 2] = bg[2];
                    img[p + 3] = 255;
                }
            }

            let Some(chr) = cell.and_then(|c| c.contents().chars().next()) else {
                continue;
            };
            let Some((m, cov)) = glyphs.get(&chr) else {
                continue;
            };
            if m.width == 0 || m.height == 0 {
                continue;
            }
            let ox = x0 as i32 + ((cell_w as i32 - m.width as i32) / 2).max(0);
            let top = y0 as i32 + (ascent.round() as i32) - (m.height as i32 + m.ymin);
            for gy in 0..m.height {
                let py = top + gy as i32;
                if py < 0 || py as usize >= h {
                    continue;
                }
                for gx in 0..m.width {
                    let pxc = ox + gx as i32;
                    if pxc < 0 || pxc as usize >= w {
                        continue;
                    }
                    let a = cov[gy * m.width + gx] as u32;
                    if a == 0 {
                        continue;
                    }
                    let p = (py as usize * w + pxc as usize) * 4;
                    for k in 0..3 {
                        let dst = img[p + k] as u32;
                        let src = fg[k] as u32;
                        img[p + k] = ((src * a + dst * (255 - a)) / 255) as u8;
                    }
                }
            }
        }
    }
    img
}

/// Map a vt100 colour to RGB.
fn resolve(c: Color, default: [u8; 3]) -> [u8; 3] {
    match c {
        Color::Default => default,
        Color::Idx(i) if (i as usize) < 16 => ANSI16[i as usize],
        Color::Idx(i) => xterm256(i),
        Color::Rgb(r, g, b) => [r, g, b],
    }
}

/// xterm 256-colour cube + grayscale ramp (indices 16..=255).
fn xterm256(i: u8) -> [u8; 3] {
    if i < 16 {
        return ANSI16[i as usize];
    }
    if i >= 232 {
        let v = 8 + (i as u16 - 232) * 10;
        return [v as u8, v as u8, v as u8];
    }
    let i = i as u16 - 16;
    let lvl = |c: u16| if c == 0 { 0u8 } else { (55 + c * 40) as u8 };
    [lvl(i / 36), lvl((i % 36) / 6), lvl(i % 6)]
}

/// Parse a `#rrggbb` colour.
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

/// Alpha-blend one rasterized glyph (`cov` coverage, `m` metrics) in `fg` onto
/// the RGBA `img` at `(ox, top)`, clipped to the canvas.
#[allow(clippy::too_many_arguments)]
fn blit_glyph(
    img: &mut [u8],
    w: usize,
    h: usize,
    ox: i32,
    top: i32,
    m: &Metrics,
    cov: &[u8],
    fg: [u8; 3],
) {
    for gy in 0..m.height {
        let py = top + gy as i32;
        if py < 0 || py as usize >= h {
            continue;
        }
        for gx in 0..m.width {
            let pxc = ox + gx as i32;
            if pxc < 0 || pxc as usize >= w {
                continue;
            }
            let a = cov[gy * m.width + gx] as u32;
            if a == 0 {
                continue;
            }
            let p = (py as usize * w + pxc as usize) * 4;
            for k in 0..3 {
                let dst = img[p + k] as u32;
                img[p + k] = ((fg[k] as u32 * a + dst * (255 - a)) / 255) as u8;
            }
        }
    }
}

/// An on-canvas caption track: a bottom bar with centered text, switching to the
/// latest caption that is active at the current time.
pub struct CaptionOverlay {
    captions: Vec<(f64, String)>,
    glyphs: HashMap<char, (Metrics, Vec<u8>)>,
    px: f32,
    cell_w: usize,
}

impl CaptionOverlay {
    pub fn new(captions: Vec<(f64, String)>, px: f32) -> Result<Self> {
        let font = Font::from_bytes(FONT, FontSettings::default())
            .map_err(|e| Error::Export(format!("font: {e}")))?;
        let mut glyphs = HashMap::new();
        for code in 0x20u8..=0x7e {
            let ch = code as char;
            glyphs.insert(ch, font.rasterize(ch, px));
        }
        for &ch in EXTRA_GLYPHS {
            glyphs.insert(ch, font.rasterize(ch, px));
        }
        Ok(CaptionOverlay {
            captions,
            glyphs,
            px,
            cell_w: (px * 0.6).round().max(1.0) as usize,
        })
    }

    /// The caption text active at time `t` (latest with start ≤ t), if non-empty.
    pub fn active(&self, t: f64) -> Option<&str> {
        let mut chosen: Option<&str> = None;
        for (start, text) in &self.captions {
            if *start <= t {
                chosen = Some(text.as_str());
            } else {
                break;
            }
        }
        chosen.filter(|s| !s.is_empty())
    }

    /// Draw the active caption onto `img` (`w`×`h` RGBA) at time `t`.
    pub fn draw(&self, img: &mut [u8], w: usize, h: usize, t: f64) {
        let Some(text) = self.active(t) else {
            return;
        };
        let bar_h = (self.px * 2.2).round() as usize;
        if bar_h == 0 || bar_h >= h || w == 0 {
            return;
        }
        let y0 = h - bar_h;
        // Darken the bar to ~40% so light text reads on top.
        for y in y0..h {
            for x in 0..w {
                let p = (y * w + x) * 4;
                img[p] = (img[p] as u32 * 2 / 5) as u8;
                img[p + 1] = (img[p + 1] as u32 * 2 / 5) as u8;
                img[p + 2] = (img[p + 2] as u32 * 2 / 5) as u8;
            }
        }
        let text_w = text.chars().count() * self.cell_w;
        let start_x = w.saturating_sub(text_w) / 2;
        let baseline = y0 + bar_h / 2 + (self.px * 0.35) as usize;
        let fg = [235u8, 235, 235];
        let mut cx = start_x;
        for ch in text.chars() {
            if let Some((m, cov)) = self.glyphs.get(&ch) {
                let ox = cx as i32 + ((self.cell_w as i32 - m.width as i32) / 2).max(0);
                let top = baseline as i32 - (m.height as i32 + m.ymin);
                blit_glyph(img, w, h, ox, top, m, cov, fg);
            }
            cx += self.cell_w;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_hex("#0b0f14"), Some([11, 15, 20]));
        assert_eq!(parse_hex("ffffff"), Some([255, 255, 255]));
        assert_eq!(parse_hex("bad"), None);
    }

    #[test]
    fn resolves_ansi_and_rgb() {
        assert_eq!(resolve(Color::Idx(1), DEFAULT_FG), ANSI16[1]);
        assert_eq!(resolve(Color::Rgb(10, 20, 30), DEFAULT_FG), [10, 20, 30]);
        assert_eq!(resolve(Color::Default, [1, 2, 3]), [1, 2, 3]);
    }

    #[test]
    fn xterm_cube_endpoints() {
        assert_eq!(xterm256(16), [0, 0, 0]);
        assert_eq!(xterm256(231), [255, 255, 255]);
        assert_eq!(xterm256(232), [8, 8, 8]);
    }

    #[test]
    fn caption_overlay_picks_latest_nonempty() {
        let c = CaptionOverlay::new(
            vec![
                (0.0, "step 1".into()),
                (1.0, "step 2".into()),
                (2.0, String::new()),
            ],
            18.0,
        )
        .unwrap();
        assert_eq!(c.active(0.0), Some("step 1"));
        assert_eq!(c.active(0.9), Some("step 1"));
        assert_eq!(c.active(1.5), Some("step 2"));
        assert_eq!(c.active(2.5), None); // cleared by the empty caption
    }
}
