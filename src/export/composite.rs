//! Multi-scene compositor (the "Stage Matrix", SPEC §4): blit each pane's
//! RGBA sub-frame onto the shared canvas at its position. Pure and
//! dependency-free, so the layout maths is fully unit-tested independently of
//! any pane source (terminal raster, browser screenshots, …).

/// One pane's contribution to a composited frame: an RGBA buffer of `w`×`h`
/// placed at `(x, y)` on the canvas.
pub struct Layer<'a> {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    /// Row-major RGBA, expected length `w * h * 4` (shorter is clipped).
    pub rgba: &'a [u8],
}

/// Composite `layers` onto a `canvas_w`×`canvas_h` canvas filled with `bg`.
/// Layers are drawn in order (later layers on top) and clipped to the canvas.
pub fn composite(canvas_w: usize, canvas_h: usize, bg: [u8; 3], layers: &[Layer]) -> Vec<u8> {
    let mut img = vec![0u8; canvas_w * canvas_h * 4];
    for px in img.chunks_exact_mut(4) {
        px[0] = bg[0];
        px[1] = bg[1];
        px[2] = bg[2];
        px[3] = 255;
    }

    for layer in layers {
        for ry in 0..layer.h {
            let cy = layer.y + ry;
            if cy >= canvas_h {
                break;
            }
            for rx in 0..layer.w {
                let cx = layer.x + rx;
                if cx >= canvas_w {
                    break;
                }
                let s = (ry * layer.w + rx) * 4;
                if s + 4 > layer.rgba.len() {
                    continue;
                }
                let d = (cy * canvas_w + cx) * 4;
                img[d] = layer.rgba[s];
                img[d + 1] = layer.rgba[s + 1];
                img[d + 2] = layer.rgba[s + 2];
                img[d + 3] = 255;
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(img: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
        let p = (y * w + x) * 4;
        [img[p], img[p + 1], img[p + 2]]
    }

    #[test]
    fn fills_background_when_no_layers() {
        let img = composite(2, 2, [10, 20, 30], &[]);
        assert_eq!(at(&img, 2, 0, 0), [10, 20, 30]);
        assert_eq!(at(&img, 2, 1, 1), [10, 20, 30]);
    }

    #[test]
    fn blits_two_panes_side_by_side() {
        // 4x2 canvas; left 2x2 red, right 2x2 green.
        let red = [255, 0, 0, 255].repeat(4); // 2x2
        let green = [0, 255, 0, 255].repeat(4);
        let img = composite(
            4,
            2,
            [0, 0, 0],
            &[
                Layer {
                    x: 0,
                    y: 0,
                    w: 2,
                    h: 2,
                    rgba: &red,
                },
                Layer {
                    x: 2,
                    y: 0,
                    w: 2,
                    h: 2,
                    rgba: &green,
                },
            ],
        );
        assert_eq!(at(&img, 4, 0, 0), [255, 0, 0]);
        assert_eq!(at(&img, 4, 1, 1), [255, 0, 0]);
        assert_eq!(at(&img, 4, 2, 0), [0, 255, 0]);
        assert_eq!(at(&img, 4, 3, 1), [0, 255, 0]);
    }

    #[test]
    fn clips_layers_outside_the_canvas() {
        // 2x2 layer placed so half hangs off the right/bottom edge.
        let white = [255, 255, 255, 255].repeat(4);
        let img = composite(
            2,
            2,
            [0, 0, 0],
            &[Layer {
                x: 1,
                y: 1,
                w: 2,
                h: 2,
                rgba: &white,
            }],
        );
        // only (1,1) is inside the canvas
        assert_eq!(at(&img, 2, 0, 0), [0, 0, 0]);
        assert_eq!(at(&img, 2, 1, 1), [255, 255, 255]);
    }

    #[test]
    fn later_layers_paint_over_earlier() {
        let red = [255u8, 0, 0, 255];
        let blue = [0u8, 0, 255, 255];
        let img = composite(
            1,
            1,
            [0, 0, 0],
            &[
                Layer {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1,
                    rgba: &red,
                },
                Layer {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1,
                    rgba: &blue,
                },
            ],
        );
        assert_eq!(at(&img, 1, 0, 0), [0, 0, 255]);
    }

    #[test]
    fn clips_short_rgba_buffer() {
        // A layer with a buffer shorter than w*h*4 should not panic.
        let short_buf = [255u8, 128, 0, 255, 255, 128, 0, 255]; // only 2 pixels for a 2x2 layer
        let img = composite(
            3,
            3,
            [0, 0, 0],
            &[Layer {
                x: 0,
                y: 0,
                w: 2,
                h: 2,
                rgba: &short_buf,
            }],
        );
        // First pixel should be painted, the rest should stay background.
        assert_eq!(at(&img, 3, 0, 0), [255, 128, 0]);
    }

    #[test]
    fn layer_completely_outside_canvas() {
        let red = [255u8, 0, 0, 255].repeat(4);
        let img = composite(
            2,
            2,
            [10, 10, 10],
            &[Layer {
                x: 10,
                y: 10,
                w: 2,
                h: 2,
                rgba: &red,
            }],
        );
        // Canvas should remain all background.
        assert_eq!(at(&img, 2, 0, 0), [10, 10, 10]);
    }
}
