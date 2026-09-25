//! StarCraft GRP sheets and the palette they are drawn with.
//!
//! The DAT wiki shows the same pictures EUD Editor 3's DAT Editor shows:
//! command icons, the three wireframe sheets, and the unit/sprite graphics
//! `images.dat` names. Every byte comes from the installed StarCraft through
//! [`isom::game_asset`], which hands the archive's file back verbatim; like
//! `iscript_info`, the parsing happens here in Rust rather than adding a second
//! interpretation of StarCraft data to the native shim.
//!
//! Each palette is derived the way the engine derives it. StarCraft keeps one
//! palette per screen element in a `game\t*.pcx`, next to a remap row saying
//! which entry each shade of that element is drawn with, so a picture is only
//! readable through its own file: a command icon is a 16-shade ramp coloured
//! through `game\ticon.pcx`, and a wireframe uses `game\twire.pcx` as it
//! stands, its remap rows being the yellow and red damage tints. A unit
//! graphic instead uses a tileset `.wpe` for indices 0-7 and 16-255 with
//! indices 8-15 remapped through `game\tunit.pcx` for the owner's colour,
//! exactly as the comment on `Sc::Sprite::PixelLine` in
//! `native/isom/MappingCoreLib/Sc.h` describes. Nothing is bundled, so the
//! wiki never ships a stale copy of Blizzard's data.

use std::io::Cursor;

/// The command-icon sheet: one 36x34 frame per icon id.
pub const CMDICONS_ASSET: &str = r"unit\cmdicons\cmdicons.grp";
/// The unit wireframe sheet (228 frames, one per unit id).
pub const WIREFRAM_ASSET: &str = r"unit\wirefram\wirefram.grp";
/// The group wireframe sheet (131 frames).
pub const GRPWIRE_ASSET: &str = r"unit\wirefram\grpwire.grp";
/// The transport wireframe sheet (106 frames).
pub const TRANWIRE_ASSET: &str = r"unit\wirefram\tranwire.grp";
/// The player-colour remap: 16 players of 8 palette indices.
pub const UNIT_REMAP_ASSET: &str = r"game\tunit.pcx";
/// The command-icon palette, and the ramps the console draws a button with.
/// A `cmdicons` frame is a 16-shade ramp rather than a full-colour picture,
/// so it is unreadable until this file says what the shades are.
pub const ICON_PALETTE_ASSET: &str = r"game\ticon.pcx";
/// How many shades one icon ramp covers; `ticon.pcx` holds six of them, and
/// the first is the gold an available button is drawn in.
pub const ICON_RAMP: usize = 16;
/// The wireframe palette. Its remap rows are the yellow/red damage tints, so
/// a reference view uses the palette as it stands: the full-health wireframe.
pub const WIRE_PALETTE_ASSET: &str = r"game\twire.pcx";
/// The palette every picture starts from. Badlands is the reference tileset;
/// it and every other tileset agree on all 188 indices a unit graphic uses
/// outside the player-colour range.
pub const PALETTE_ASSET: &str = r"tileset\badlands.wpe";

/// Unit GRPs live under this archive directory, named by `arr\images.tbl`.
pub const UNIT_GRP_PREFIX: &str = r"unit\";

/// The palette range a GRP fills with the owning player's colour.
const PLAYER_RANGE: std::ops::Range<usize> = 8..16;
/// One player's share of `tunit.pcx`.
const PLAYER_SHADES: usize = 8;

/// A 256-colour palette, opaque except for index 0.
#[derive(Clone, Debug)]
pub struct Palette {
    colors: [[u8; 3]; 256],
}

impl Palette {
    /// Reads a `.wpe` (256 RGBX quads) or a raw 768-byte RGB palette.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let stride = match bytes.len() {
            1024 => 4,
            768 => 3,
            other => {
                return Err(format!(
                    "a palette must be 768 or 1024 bytes, this one is {other}"
                ))
            }
        };
        let mut colors = [[0_u8; 3]; 256];
        for (index, color) in colors.iter_mut().enumerate() {
            let at = index * stride;
            color.copy_from_slice(&bytes[at..at + 3]);
        }
        Ok(Self { colors })
    }

    /// Replaces indices 8-15 with `player`'s colour ramp out of `tunit.pcx`.
    pub fn with_player(&self, remap: &[u8], player: u8) -> Result<Self, String> {
        let first = usize::from(player) * PLAYER_SHADES;
        let ramp = remap
            .get(first..first + PLAYER_SHADES)
            .ok_or_else(|| format!("the unit remap has no player {player}"))?;
        self.remapped(ramp, PLAYER_RANGE.start)
    }

    /// Replaces the indices from `first` with the colours `ramp` points at.
    /// StarCraft's `game\t*.pcx` files are exactly these rows: each says which
    /// palette entry a screen element's shade is drawn with.
    pub fn remapped(&self, ramp: &[u8], first: usize) -> Result<Self, String> {
        if first + ramp.len() > self.colors.len() {
            return Err(format!(
                "a {}-entry ramp does not fit the palette at index {first}",
                ramp.len()
            ));
        }
        let mut colors = self.colors;
        for (shade, index) in ramp.iter().enumerate() {
            colors[first + shade] = self.colors[usize::from(*index)];
        }
        Ok(Self { colors })
    }
}

const PCX_HEADER: usize = 128;
const PCX_TRAILING_PALETTE: usize = 769;

impl Palette {
    /// The 256-colour palette an 8-bit PCX carries after its pixels. This is
    /// where StarCraft keeps the palette each screen element is drawn with:
    /// `game\ticon.pcx` for the command icons, `game\twire.pcx` for the
    /// wireframes, `game\tunit.pcx` for units.
    pub fn from_pcx(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < PCX_TRAILING_PALETTE || bytes[bytes.len() - PCX_TRAILING_PALETTE] != 0x0C {
            return Err("this PCX carries no trailing palette".into());
        }
        Self::parse(&bytes[bytes.len() - PCX_TRAILING_PALETTE + 1..])
    }
}

/// Decodes the pixels of an 8-bit run-length PCX, which is how StarCraft
/// stores its remap tables (`tunit.pcx` is 128x1: 16 players of 8 shades).
pub fn pcx_pixels(bytes: &[u8]) -> Result<Vec<u8>, String> {
    const HEADER: usize = PCX_HEADER;
    const TRAILING_PALETTE: usize = PCX_TRAILING_PALETTE;
    if bytes.len() <= HEADER + TRAILING_PALETTE {
        return Err("a PCX remap table is truncated".into());
    }
    if bytes[0] != 0x0A || bytes[3] != 8 || bytes[65] != 1 {
        return Err("only an 8-bit single-plane PCX is a remap table".into());
    }
    let read = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let width = usize::from(read(8).wrapping_sub(read(4))) + 1;
    let height = usize::from(read(10).wrapping_sub(read(6))) + 1;
    let stride = usize::from(read(66));
    let expected = stride * height;
    if expected == 0 || width > stride {
        return Err("a PCX remap table has no usable extent".into());
    }
    let encoded = &bytes[HEADER..bytes.len() - TRAILING_PALETTE];
    let mut pixels = Vec::with_capacity(expected);
    let mut at = 0;
    while at < encoded.len() && pixels.len() < expected {
        let control = encoded[at];
        at += 1;
        if control < 0xC0 {
            pixels.push(control);
            continue;
        }
        let run = usize::from(control & 0x3F);
        let value = *encoded.get(at).ok_or("a PCX run ends without its colour")?;
        at += 1;
        pixels.resize(pixels.len() + run, value);
    }
    if pixels.len() < expected {
        return Err("a PCX remap table decodes to fewer pixels than it declares".into());
    }
    // Rows are padded to `stride`; the wiki only ever wants the declared width.
    Ok((0..height)
        .flat_map(|row| pixels[row * stride..row * stride + width].to_vec())
        .collect())
}

#[derive(Clone, Copy)]
struct FrameHeader {
    x_offset: u8,
    y_offset: u8,
    width: u8,
    height: u8,
    offset: u32,
}

/// One GRP sheet, kept as its archive bytes so a frame is decoded on demand.
pub struct Grp {
    width: u16,
    height: u16,
    frames: Vec<FrameHeader>,
    bytes: Vec<u8>,
}

impl Grp {
    /// Reads the sheet header. A frame's pixel lines are validated when that
    /// frame is drawn, so one damaged frame never hides the rest of the sheet.
    pub fn parse(bytes: Vec<u8>) -> Result<Self, String> {
        const FILE_HEADER: usize = 6;
        const FRAME_HEADER: usize = 8;
        if bytes.len() < FILE_HEADER {
            return Err("a GRP shorter than its file header is not a GRP".into());
        }
        let read = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let count = usize::from(read(0));
        let width = read(2);
        let height = read(4);
        if count == 0 || width == 0 || height == 0 {
            return Err("a GRP declares no frames or no extent".into());
        }
        let headers_end = FILE_HEADER + count * FRAME_HEADER;
        if bytes.len() < headers_end {
            return Err(format!(
                "a GRP declares {count} frames but holds only {} bytes",
                bytes.len()
            ));
        }
        let frames = (0..count)
            .map(|index| {
                let at = FILE_HEADER + index * FRAME_HEADER;
                FrameHeader {
                    x_offset: bytes[at],
                    y_offset: bytes[at + 1],
                    width: bytes[at + 2],
                    height: bytes[at + 3],
                    offset: u32::from_le_bytes([
                        bytes[at + 4],
                        bytes[at + 5],
                        bytes[at + 6],
                        bytes[at + 7],
                    ]),
                }
            })
            .collect();
        Ok(Self {
            width,
            height,
            frames,
            bytes,
        })
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    /// Draws one frame onto the sheet's full canvas, so every frame of a sheet
    /// lines up the way the game lines them up. Untouched pixels stay
    /// transparent rather than becoming palette index 0.
    pub fn frame_rgba(&self, index: usize, palette: &Palette) -> Result<Vec<u8>, String> {
        let frame = *self
            .frames
            .get(index)
            .ok_or_else(|| format!("this GRP has no frame {index}"))?;
        let canvas_width = usize::from(self.width);
        let canvas_height = usize::from(self.height);
        let mut rgba = vec![0_u8; canvas_width * canvas_height * 4];
        if frame.width == 0 || frame.height == 0 {
            return Ok(rgba);
        }
        let frame_width = usize::from(frame.width);
        let frame_height = usize::from(frame.height);
        let base = usize::try_from(frame.offset).map_err(|_| "a GRP frame offset is absurd")?;
        // Every row offset below is read out of this table, so it has to be
        // inside the file before the first one is trusted.
        if base
            .checked_add(frame_height * 2)
            .map(|end| end > self.bytes.len())
            .unwrap_or(true)
        {
            return Err("a GRP frame's row table runs past the file".into());
        }
        for row in 0..frame_height {
            let entry = base + row * 2;
            let row_offset = usize::from(u16::from_le_bytes([
                self.bytes[entry],
                self.bytes[entry + 1],
            ]));
            let mut at = base + row_offset;
            let mut column = 0;
            while column < frame_width {
                let header = *self
                    .bytes
                    .get(at)
                    .ok_or("a GRP pixel row ends past the file")?;
                at += 1;
                // The three line kinds of `Sc::Sprite::PixelLine`.
                if header & 0x80 != 0 {
                    column += usize::from(header & 0x7F);
                    continue;
                }
                let solid = header & 0x40 != 0;
                let length = usize::from(if solid { header & 0x3F } else { header });
                if length == 0 {
                    break;
                }
                let pixels = if solid { 1 } else { length };
                let line = self
                    .bytes
                    .get(at..at + pixels)
                    .ok_or("a GRP pixel line ends past the file")?;
                at += pixels;
                for pixel in 0..length {
                    if column >= frame_width {
                        break;
                    }
                    let entry = if solid { line[0] } else { line[pixel] };
                    let x = usize::from(frame.x_offset) + column;
                    let y = usize::from(frame.y_offset) + row;
                    column += 1;
                    if x >= canvas_width || y >= canvas_height {
                        continue;
                    }
                    let color = palette.colors[usize::from(entry)];
                    let at = (y * canvas_width + x) * 4;
                    rgba[at..at + 3].copy_from_slice(&color);
                    rgba[at + 3] = 255;
                }
            }
        }
        Ok(rgba)
    }

    /// One frame as a PNG, ready for an `<img>` source.
    pub fn frame_png(&self, index: usize, palette: &Palette) -> Result<Vec<u8>, String> {
        let rgba = self.frame_rgba(index, palette)?;
        encode_png(self.width.into(), self.height.into(), &rgba)
    }
}

/// Writes 8-bit RGBA as a PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(Cursor::new(&mut out), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| format!("a wiki picture could not be started: {error}"))?;
        writer
            .write_image_data(rgba)
            .map_err(|error| format!("a wiki picture could not be written: {error}"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-frame 4x1 GRP: a transparent run, a solid run, then a speckled run.
    fn sheet() -> Vec<u8> {
        let mut bytes = vec![
            1, 0, // one frame
            4, 0, // 4 wide
            1, 0, // 1 tall
        ];
        bytes.extend_from_slice(&[0, 0, 4, 1]); // x, y, width, height
        bytes.extend_from_slice(&14_u32.to_le_bytes()); // frame offset
        bytes.extend_from_slice(&2_u16.to_le_bytes()); // row 0 starts after the table
        bytes.extend_from_slice(&[
            0x81, // one transparent pixel
            0x41, 9, // one solid pixel of index 9
            0x02, 30, 31, // two speckled pixels
        ]);
        bytes
    }

    fn palette() -> Palette {
        let mut bytes = vec![0_u8; 1024];
        for index in 0..256 {
            bytes[index * 4] = index as u8;
            bytes[index * 4 + 1] = 0;
            bytes[index * 4 + 2] = 255 - index as u8;
        }
        Palette::parse(&bytes).unwrap()
    }

    #[test]
    fn the_three_pixel_line_kinds_each_draw_what_they_declare() {
        let grp = Grp::parse(sheet()).unwrap();
        assert_eq!(grp.frame_count(), 1);
        let rgba = grp.frame_rgba(0, &palette()).unwrap();
        // A transparent run leaves alpha at zero; the rest is opaque.
        assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);
        assert_eq!(&rgba[4..8], &[9, 0, 246, 255]);
        assert_eq!(&rgba[8..12], &[30, 0, 225, 255]);
        assert_eq!(&rgba[12..16], &[31, 0, 224, 255]);
    }

    #[test]
    fn a_frame_beyond_the_sheet_is_refused_by_number() {
        let grp = Grp::parse(sheet()).unwrap();
        let error = grp.frame_rgba(1, &palette()).unwrap_err();
        assert!(error.contains("frame 1"), "{error}");
    }

    #[test]
    fn a_truncated_sheet_never_becomes_a_blank_picture() {
        let mut bytes = sheet();
        bytes.truncate(10);
        let error = Grp::parse(bytes)
            .err()
            .expect("a truncated sheet is an error");
        assert!(error.contains("frames"), "{error}");
    }

    #[test]
    fn the_player_range_is_the_only_part_a_remap_changes() {
        let stock = palette();
        // Player 1's ramp points at indices 100..108.
        let remap: Vec<u8> = (0..128).map(|index| 100 + index as u8).collect();
        let player = stock.with_player(&remap, 1).unwrap();
        for index in 0..256 {
            if PLAYER_RANGE.contains(&index) {
                let shade = 100 + 8 + (index - PLAYER_RANGE.start);
                assert_eq!(player.colors[index], stock.colors[shade], "index {index}");
            } else {
                assert_eq!(player.colors[index], stock.colors[index], "index {index}");
            }
        }
    }

    #[test]
    fn a_player_the_remap_does_not_carry_is_refused() {
        let error = palette().with_player(&[0; 128], 16).unwrap_err();
        assert!(error.contains("player 16"), "{error}");
    }

    #[test]
    fn a_run_length_pcx_decodes_to_its_declared_width() {
        let mut bytes = vec![0_u8; 128];
        bytes[0] = 0x0A;
        bytes[3] = 8;
        bytes[65] = 1;
        bytes[8..10].copy_from_slice(&3_u16.to_le_bytes()); // xMax -> width 4
        bytes[10..12].copy_from_slice(&0_u16.to_le_bytes()); // one row
        bytes[66..68].copy_from_slice(&6_u16.to_le_bytes()); // padded stride
        bytes.extend_from_slice(&[0xC4, 7, 1, 2]); // a run of four 7s, then padding
        bytes.extend_from_slice(&[0x0C]);
        bytes.extend_from_slice(&[0_u8; 768]);
        assert_eq!(pcx_pixels(&bytes).unwrap(), vec![7, 7, 7, 7]);
    }

    #[test]
    fn only_a_768_or_1024_byte_palette_is_accepted() {
        let error = Palette::parse(&[0; 999]).unwrap_err();
        assert!(error.contains("999"), "{error}");
    }
}
