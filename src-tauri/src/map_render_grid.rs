//! Tile-coordinate grid and margin rulers for agent map renders.
//!
//! `map_render`/`map_draft_render` crops are downscaled (`32 / scale` image
//! pixels per tile), so a model reading the picture cannot tell which tile a
//! feature sits on. `annotate` draws grid lines on the crop and prints
//! absolute tile coordinates in a top and left margin, and returns the
//! metadata needed to convert any image pixel back to a tile.

use isom::RgbaImage;
use serde_json::{json, Value};

/// 3x5 digit glyphs, one row per byte, bit 2 = leftmost column.
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];
const GLYPH_SCALE: u32 = 2;
const GLYPH_WIDTH: u32 = 3 * GLYPH_SCALE;
const GLYPH_HEIGHT: u32 = 5 * GLYPH_SCALE;
const GLYPH_GAP: u32 = GLYPH_SCALE;
const MARGIN_PAD: u32 = 3;
/// Map tile coordinates are at most 255, so labels never exceed 3 digits.
const MAX_LABEL_DIGITS: u32 = 3;
pub const TOP_MARGIN: u32 = GLYPH_HEIGHT + 2 * MARGIN_PAD;
pub const LEFT_MARGIN: u32 =
    MAX_LABEL_DIGITS * GLYPH_WIDTH + (MAX_LABEL_DIGITS - 1) * GLYPH_GAP + 2 * MARGIN_PAD;
/// Minimum image-pixel distance between labelled (major) grid lines.
const LABEL_SPACING: u32 = 32;
/// Minimum image-pixel distance between minor grid lines.
const LINE_SPACING: u32 = 8;

const MARGIN_COLOR: [u8; 3] = [24, 24, 24];
const LABEL_COLOR: [u8; 3] = [235, 235, 235];
const MINOR_LINE: ([u8; 3], u8) = ([255, 255, 255], 64);
const MAJOR_LINE: ([u8; 3], u8) = ([255, 224, 0], 160);

/// Draw the grid and rulers around `image`, a render of the crop whose top-left
/// tile is (`tile_x`, `tile_y`) at `scale`. Returns the annotated image and the
/// `grid` metadata reported next to it.
pub fn annotate(
    image: &RgbaImage,
    tile_x: usize,
    tile_y: usize,
    scale: usize,
) -> (RgbaImage, Value) {
    debug_assert_eq!(
        image.rgba.len(),
        image.width as usize * image.height as usize * 4
    );
    let tile_pixels = 32 / scale.clamp(1, 32) as u32;
    let tiles_wide = image.width / tile_pixels;
    let tiles_high = image.height / tile_pixels;
    let line_every = (LINE_SPACING / tile_pixels).max(1);
    let label_every = (LABEL_SPACING / tile_pixels).max(1);

    let width = image.width + LEFT_MARGIN;
    let height = image.height + TOP_MARGIN;
    let mut out = RgbaImage {
        width,
        height,
        rgba: Vec::with_capacity(width as usize * height as usize * 4),
    };
    for y in 0..height {
        for x in 0..width {
            if x >= LEFT_MARGIN && y >= TOP_MARGIN {
                let source = ((y - TOP_MARGIN) * image.width + (x - LEFT_MARGIN)) as usize * 4;
                out.rgba.extend_from_slice(&image.rgba[source..source + 4]);
            } else {
                out.rgba.extend_from_slice(&[
                    MARGIN_COLOR[0],
                    MARGIN_COLOR[1],
                    MARGIN_COLOR[2],
                    255,
                ]);
            }
        }
    }

    for column in 0..tiles_wide {
        let tile = tile_x as u32 + column;
        if tile % line_every != 0 {
            continue;
        }
        let line = if tile % label_every == 0 {
            MAJOR_LINE
        } else {
            MINOR_LINE
        };
        let px = LEFT_MARGIN + column * tile_pixels;
        for py in TOP_MARGIN..height {
            blend(&mut out, px, py, line);
        }
    }
    for row in 0..tiles_high {
        let tile = tile_y as u32 + row;
        if tile % line_every != 0 {
            continue;
        }
        let line = if tile % label_every == 0 {
            MAJOR_LINE
        } else {
            MINOR_LINE
        };
        let py = TOP_MARGIN + row * tile_pixels;
        for px in LEFT_MARGIN..width {
            blend(&mut out, px, py, line);
        }
    }

    // A label that would run past the far edge is pulled back inside it, and is
    // dropped only when that would overlap the previous label.
    let mut previous_end = 0;
    for tile in labelled_tiles(tile_x as u32, tiles_wide, tile_pixels, label_every) {
        let text = tile.to_string();
        let label_width = text_width(&text);
        let px = (LEFT_MARGIN + (tile - tile_x as u32) * tile_pixels + 1)
            .min(width.saturating_sub(label_width))
            .max(LEFT_MARGIN);
        if previous_end != 0 && px < previous_end + GLYPH_GAP {
            continue;
        }
        draw_text(&mut out, px, MARGIN_PAD, &text);
        previous_end = px + label_width;
    }
    let mut previous_end = 0;
    for tile in labelled_tiles(tile_y as u32, tiles_high, tile_pixels, label_every) {
        let text = tile.to_string();
        let py = (TOP_MARGIN + (tile - tile_y as u32) * tile_pixels + 1)
            .min(height.saturating_sub(GLYPH_HEIGHT));
        if previous_end != 0 && py < previous_end + GLYPH_GAP {
            continue;
        }
        draw_text(
            &mut out,
            LEFT_MARGIN - MARGIN_PAD - text_width(&text),
            py,
            &text,
        );
        previous_end = py + GLYPH_HEIGHT;
    }

    let metadata = json!({
        "tileX": tile_x,
        "tileY": tile_y,
        "tilesWide": tiles_wide,
        "tilesHigh": tiles_high,
        "tilePixels": tile_pixels,
        "mapLeft": LEFT_MARGIN,
        "mapTop": TOP_MARGIN,
        "lineEveryTiles": line_every,
        "labelEveryTiles": label_every,
        "note": "Ruler numbers are absolute map tile coordinates; yellow lines fall on multiples of labelEveryTiles. If the image is shown downscaled, read coordinates from the ruler numbers. Image pixel (px, py) inside the map area is tile (tileX + floor((px - mapLeft) / tilePixels), tileY + floor((py - mapTop) / tilePixels)); a tile's map pixel origin is tile * 32.",
    });
    (out, metadata)
}

/// Tiles that get a ruler number: every multiple of `label_every` in the crop,
/// plus the first tile when the crop does not start on a multiple and the next
/// label is far enough away (so a narrow crop is never left without a number).
fn labelled_tiles(first: u32, count: u32, tile_pixels: u32, label_every: u32) -> Vec<u32> {
    let mut tiles: Vec<u32> = (first..first + count)
        .filter(|tile| tile % label_every == 0)
        .collect();
    let next = tiles.first().copied();
    if first % label_every != 0
        && next.is_none_or(|next| (next - first) * tile_pixels >= LABEL_SPACING)
    {
        tiles.insert(0, first);
    }
    tiles
}

fn text_width(text: &str) -> u32 {
    let digits = text.len() as u32;
    digits * GLYPH_WIDTH + digits.saturating_sub(1) * GLYPH_GAP
}

fn draw_text(image: &mut RgbaImage, left: u32, top: u32, text: &str) {
    for (index, digit) in text.bytes().enumerate() {
        let glyph = DIGITS[usize::from(digit - b'0')];
        let glyph_left = left + index as u32 * (GLYPH_WIDTH + GLYPH_GAP);
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..3 {
                if bits & (0b100 >> column) == 0 {
                    continue;
                }
                for dy in 0..GLYPH_SCALE {
                    for dx in 0..GLYPH_SCALE {
                        let x = glyph_left + column * GLYPH_SCALE + dx;
                        let y = top + row as u32 * GLYPH_SCALE + dy;
                        blend(image, x, y, (LABEL_COLOR, 255));
                    }
                }
            }
        }
    }
}

fn blend(image: &mut RgbaImage, x: u32, y: u32, (color, alpha): ([u8; 3], u8)) {
    if x >= image.width || y >= image.height {
        return;
    }
    let index = (y * image.width + x) as usize * 4;
    let alpha = u32::from(alpha);
    for channel in 0..3 {
        let base = u32::from(image.rgba[index + channel]);
        image.rgba[index + channel] =
            ((u32::from(color[channel]) * alpha + base * (255 - alpha) + 127) / 255) as u8;
    }
    image.rgba[index + 3] = 255;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32) -> RgbaImage {
        RgbaImage {
            width,
            height,
            rgba: [10, 20, 30, 255].repeat((width * height) as usize),
        }
    }

    fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let index = (y * image.width + x) as usize * 4;
        image.rgba[index..index + 4].try_into().unwrap()
    }

    fn has_label_pixel(image: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> bool {
        (y0..y1).any(|y| (x0..x1).any(|x| pixel(image, x, y)[..3] == LABEL_COLOR))
    }

    #[test]
    fn annotate_adds_margins_and_keeps_map_pixels_between_lines() {
        // scale 4: 8px tiles, 10x6 tile crop starting at tile (3, 5).
        let (out, meta) = annotate(&solid(80, 48), 3, 5, 4);
        assert_eq!((out.width, out.height), (80 + LEFT_MARGIN, 48 + TOP_MARGIN));
        assert_eq!(meta["tilePixels"], 8);
        assert_eq!(meta["tilesWide"], 10);
        assert_eq!(meta["tilesHigh"], 6);
        assert_eq!(meta["mapLeft"], LEFT_MARGIN);
        assert_eq!(meta["mapTop"], TOP_MARGIN);
        assert_eq!(meta["lineEveryTiles"], 1);
        assert_eq!(meta["labelEveryTiles"], 4);
        // Interior of a tile is the untouched source pixel.
        assert_eq!(
            pixel(&out, LEFT_MARGIN + 3, TOP_MARGIN + 3),
            [10, 20, 30, 255]
        );
        // Margin corner is the margin colour.
        assert_eq!(pixel(&out, 0, 0)[..3], MARGIN_COLOR);
    }

    #[test]
    fn major_lines_sit_on_absolute_multiples_of_label_every() {
        // Crop starts at tile 3; tile 4 is the first multiple of 4 -> column 1.
        let (out, _) = annotate(&solid(80, 48), 3, 5, 4);
        let major_x = LEFT_MARGIN + 8;
        let minor_x = LEFT_MARGIN + 16;
        let y = TOP_MARGIN + 3;
        assert_ne!(pixel(&out, major_x, y), pixel(&out, minor_x, y));
        assert_ne!(pixel(&out, minor_x, y), [10, 20, 30, 255]);
        // Tile 5 is the first row and is not a multiple of 4: minor; tile 8 (row 3) is major.
        let x = LEFT_MARGIN + 3;
        assert_eq!(pixel(&out, x, TOP_MARGIN + 3 * 8), pixel(&out, major_x, y));
        assert_eq!(pixel(&out, x, TOP_MARGIN), pixel(&out, minor_x, y));
    }

    #[test]
    fn rulers_print_absolute_tile_numbers() {
        let (out, _) = annotate(&solid(80, 48), 3, 5, 4);
        // Column ruler: label for tile 4 starts just right of its line.
        let label_x = LEFT_MARGIN + 8 + 1;
        assert!(has_label_pixel(
            &out,
            label_x,
            MARGIN_PAD,
            label_x + GLYPH_WIDTH,
            MARGIN_PAD + GLYPH_HEIGHT
        ));
        // Row ruler: label for tile 8 (row 3) in the left margin.
        let label_y = TOP_MARGIN + 24 + 1;
        assert!(has_label_pixel(
            &out,
            0,
            label_y,
            LEFT_MARGIN,
            label_y + GLYPH_HEIGHT
        ));
    }

    #[test]
    fn line_and_label_spacing_adapt_to_scale() {
        for (scale, tile_pixels, line_every, label_every) in
            [(1, 32, 1, 1), (2, 16, 1, 2), (4, 8, 1, 4), (8, 4, 2, 8)]
        {
            let (_, meta) = annotate(&solid(tile_pixels * 16, tile_pixels * 16), 0, 0, scale);
            assert_eq!(meta["tilePixels"], tile_pixels, "scale {scale}");
            assert_eq!(meta["lineEveryTiles"], line_every, "scale {scale}");
            assert_eq!(meta["labelEveryTiles"], label_every, "scale {scale}");
        }
    }

    #[test]
    fn narrow_crop_without_a_multiple_still_labels_its_first_tile() {
        assert_eq!(labelled_tiles(5, 2, 8, 4), vec![5]);
        // First tile is labelled when the next multiple is a full label spacing away.
        assert_eq!(labelled_tiles(4, 8, 8, 4), vec![4, 8]);
        assert_eq!(labelled_tiles(0, 9, 8, 4), vec![0, 4, 8]);
        // Dropped when the next multiple would overlap it.
        assert_eq!(labelled_tiles(7, 4, 8, 4), vec![8]);
    }

    #[test]
    fn edge_labels_are_pulled_inside_instead_of_dropped() {
        // scale 2: 16px tiles, tiles 98..=100. "100" (22px) would overrun the
        // right edge from its line at column 2, so it is drawn flush right.
        let (out, _) = annotate(&solid(48, 32), 98, 98, 2);
        let right = out.width;
        assert!(has_label_pixel(
            &out,
            right - 4,
            MARGIN_PAD,
            right,
            MARGIN_PAD + GLYPH_HEIGHT
        ));
        // Row ruler: tile 99 is not labelled (label_every 2) and tile 98's
        // label fits; a 2-tile crop at scale 8 (8px) still gets its number.
        let (out, _) = annotate(&solid(8, 8), 5, 5, 8);
        assert!(has_label_pixel(&out, 0, 0, LEFT_MARGIN, out.height));
        assert!(has_label_pixel(&out, LEFT_MARGIN, 0, out.width, TOP_MARGIN));
    }

    #[test]
    fn three_digit_labels_fit_the_left_margin() {
        assert_eq!(text_width("255") + 2 * MARGIN_PAD, LEFT_MARGIN);
        let (out, _) = annotate(&solid(32, 32), 252, 252, 8);
        let label_y = TOP_MARGIN + 4 * 4 + 1;
        assert!(has_label_pixel(
            &out,
            MARGIN_PAD,
            label_y,
            LEFT_MARGIN,
            label_y + GLYPH_HEIGHT
        ));
    }
}
