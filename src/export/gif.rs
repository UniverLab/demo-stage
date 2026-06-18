//! GIF target (pure Rust, no ffmpeg): replay the captured output through a
//! vt100 parser at the score's fps, rasterize each frame with an embedded
//! monospace font, dedupe identical frames, and encode with `gif`.
//!
//! Covers printable ASCII; exotic glyphs (box-drawing, emoji) are skipped.

use std::collections::HashMap;
use std::path::Path;

use fontdue::{Font, FontSettings, Metrics};
use vt100::{Color, Parser};

use super::run::Recording;
use crate::error::{Error, Result};
use crate::model::Score;

/// Embedded monospace font (DejaVu Sans Mono, see assets/FONT-LICENSE).
const FONT: &[u8] = include_bytes!("../../assets/DejaVuSansMono.ttf");
const DEFAULT_FG: [u8; 3] = [200, 200, 200];

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

/// Run a terminal score and write an animated GIF to `path`.
pub fn write_gif(rec: &Recording, score: &Score, path: &Path) -> Result<()> {
    let font = Font::from_bytes(FONT, FontSettings::default())
        .map_err(|e| Error::Export(format!("font: {e}")))?;

    let cols = rec.cols as usize;
    let rows = rec.rows as usize;
    let px = score
        .layout
        .panes
        .iter()
        .find_map(|p| p.font_size)
        .unwrap_or(16) as f32;
    let cell_w = (px * 0.6).round().max(1.0) as usize;
    let cell_h = (px * 1.25).round().max(1.0) as usize;
    let (img_w, img_h) = (cols * cell_w, rows * cell_h);

    let default_bg = score
        .layout
        .background
        .as_deref()
        .and_then(parse_hex)
        .unwrap_or([11, 15, 20]);

    // Pre-rasterize printable ASCII once.
    let ascent = font
        .horizontal_line_metrics(px)
        .map(|m| m.ascent)
        .unwrap_or(px * 0.8);
    let mut glyphs: HashMap<char, (Metrics, Vec<u8>)> = HashMap::new();
    for code in 0x21u8..=0x7e {
        let ch = code as char;
        glyphs.insert(ch, font.rasterize(ch, px));
    }

    let fps = score.layout.fps.max(1) as f64;
    let dt = 1.0 / fps;
    let total = rec.duration.max(dt);
    let n_frames = (total / dt).ceil() as usize + 1;
    let frame_delay_cs = (dt * 100.0).round().max(2.0) as u16;

    let file = std::fs::File::create(path).map_err(|e| Error::io(path, e))?;
    let mut encoder = gif::Encoder::new(file, img_w as u16, img_h as u16, &[])
        .map_err(|e| Error::Export(format!("gif encoder: {e}")))?;
    encoder
        .set_repeat(gif::Repeat::Infinite)
        .map_err(|e| Error::Export(format!("gif repeat: {e}")))?;

    let mut parser = Parser::new(rec.rows, rec.cols, 0);
    let mut ev_idx = 0usize;
    // Hold the current distinct frame, accumulating delay until it changes.
    let mut held: Option<(Vec<u8>, u16)> = None;

    for f in 0..n_frames {
        let t = f as f64 * dt;
        while ev_idx < rec.events.len() && rec.events[ev_idx].0 <= t {
            parser.process(rec.events[ev_idx].1.as_bytes());
            ev_idx += 1;
        }

        let rgba = render_frame(
            &parser, &glyphs, cols, rows, cell_w, cell_h, ascent, default_bg,
        );

        match held.as_mut() {
            Some((prev, delay)) if *prev == rgba => {
                *delay = delay.saturating_add(frame_delay_cs);
            }
            _ => {
                if let Some((prev, delay)) = held.take() {
                    write_frame(&mut encoder, prev, img_w, img_h, delay)?;
                }
                held = Some((rgba, frame_delay_cs));
            }
        }
    }
    if let Some((prev, delay)) = held.take() {
        write_frame(&mut encoder, prev, img_w, img_h, delay)?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_frame(
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
            // Cell background.
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

            // Glyph.
            let Some(chr) = cell.and_then(|c| c.contents().chars().next()) else {
                continue;
            };
            let Some((m, cov)) = glyphs.get(&chr) else {
                continue; // non-ASCII / space → background only
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

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
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
        assert_eq!(xterm256(16), [0, 0, 0]); // cube origin
        assert_eq!(xterm256(231), [255, 255, 255]); // cube max
        assert_eq!(xterm256(232), [8, 8, 8]); // grayscale start
    }
}
