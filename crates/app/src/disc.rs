//! A coloured disc with a few letters on it, drawn here for Windows, which only takes pixels.

use ab_glyph::{Font, FontRef, Glyph, OutlinedGlyph, PxScale, Rect, ScaleFont, point};

const FONT: &[u8] = include_bytes!("../ui/fonts/BeVietnamPro-SemiBold.ttf");
/// The widest the text may be, as a share of the disc.
const ROOM: f32 = 0.82;

/// A line of text at a size, its glyphs placed from the origin. A letter the font lacks is left
/// out, as its placeholder box would look like a fault.
fn outlines(font: &FontRef, text: &str, px: f32, x: f32, y: f32) -> Vec<OutlinedGlyph> {
    let scaled = font.as_scaled(PxScale::from(px));
    let mut caret = x;
    text.chars()
        .filter_map(|c| {
            let id = font.glyph_id(c);
            let glyph = Glyph { id, scale: PxScale::from(px), position: point(caret, y) };
            if id.0 == 0 {
                return None;
            }
            caret += scaled.h_advance(id);
            font.outline_glyph(glyph)
        })
        .collect()
}

fn bounds(glyphs: &[OutlinedGlyph]) -> Option<Rect> {
    glyphs.iter().map(OutlinedGlyph::px_bounds).reduce(|a, b| Rect {
        min: point(a.min.x.min(b.min.x), a.min.y.min(b.min.y)),
        max: point(a.max.x.max(b.max.x), a.max.y.max(b.max.y)),
    })
}

/// The disc as straight-alpha RGBA, `size` pixels across, with text set at `scale` of that. The
/// text shrinks to fit, then is centred by its ink rather than its line box, which would sit a
/// digit visibly off the middle.
pub fn draw(text: &str, size: u32, back: [u8; 3], ink: [u8; 3], scale: f32) -> Vec<[u8; 4]> {
    let font = FontRef::try_from_slice(FONT).expect("the bundled font");
    let side = size as f32;
    let mut px = side * scale;
    if let Some(bounds) = bounds(&outlines(&font, text, px, 0.0, 0.0)) {
        px *= (side * ROOM / bounds.width()).min(1.0);
    }
    let box_of_ink = bounds(&outlines(&font, text, px, 0.0, 0.0)).unwrap_or_default();
    let x = (side - box_of_ink.width()) / 2.0 - box_of_ink.min.x;
    let y = (side - box_of_ink.height()) / 2.0 - box_of_ink.min.y;

    let mut coverage = vec![0f32; (size * size) as usize];
    for glyph in outlines(&font, text, px, x, y) {
        let origin = glyph.px_bounds().min;
        glyph.draw(|gx, gy, amount| {
            let (px, py) = (gx as i32 + origin.x as i32, gy as i32 + origin.y as i32);
            if (0..size as i32).contains(&px) && (0..size as i32).contains(&py) {
                let cell = &mut coverage[py as usize * size as usize + px as usize];
                *cell = (*cell + amount).min(1.0);
            }
        });
    }

    let radius = side / 2.0;
    let mix = |channel: usize, amount: f32| {
        let (from, to) = (f32::from(back[channel]), f32::from(ink[channel]));
        (from + (to - from) * amount).round() as u8
    };
    (0..size * size)
        .map(|i| {
            let (px, py) = ((i % size) as f32 + 0.5, (i / size) as f32 + 0.5);
            let disc = (radius - (px - radius).hypot(py - radius) + 0.5).clamp(0.0, 1.0);
            let amount = coverage[i as usize];
            [mix(0, amount), mix(1, amount), mix(2, amount), (disc * 255.0).round() as u8]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 3] = [0xd8, 0x0d, 0x20];
    const WHITE: [u8; 3] = [255; 3];

    fn at(pixels: &[[u8; 4]], size: u32, x: u32, y: u32) -> [u8; 4] {
        pixels[(y * size + x) as usize]
    }

    #[test]
    fn the_disc_is_round_with_ink_inside_it() {
        for size in [16, 20, 24, 32] {
            for text in ["1", "8", "47", "99+"] {
                let pixels = draw(text, size, RED, WHITE, 0.7);
                assert_eq!(pixels.len(), (size * size) as usize);
                assert_eq!(at(&pixels, size, 0, 0)[3], 0, "the corner is not transparent");
                assert_eq!(at(&pixels, size, size - 1, size - 1)[3], 0);
                let ink = pixels.iter().filter(|p| p[1] > 200 && p[3] > 0).count();
                assert!(ink > 0, "{text} drew no ink at {size}");
                assert!(pixels.iter().any(|p| p[..3] == RED && p[3] == 255), "no solid red at {size}");
            }
        }
    }

    #[test]
    fn the_text_sits_in_the_middle() {
        let size = 32;
        let pixels = draw("8", size, RED, WHITE, 0.7);
        let inked: Vec<_> = (0..size * size).filter(|&i| pixels[i as usize][1] > 128).collect();
        let centre = |axis: fn(u32) -> u32| inked.iter().map(|&i| f64::from(axis(i))).sum::<f64>() / inked.len() as f64;
        assert!((centre(|i| i % 32) - 15.5).abs() < 1.0);
        assert!((centre(|i| i / 32) - 15.5).abs() < 1.0);
    }

    #[test]
    fn letters_the_font_lacks_leave_a_plain_disc() {
        let pixels = draw("李", 32, RED, WHITE, 0.4);
        assert!(pixels.iter().all(|p| p[..3] == RED), "a placeholder box was drawn");
    }
}
