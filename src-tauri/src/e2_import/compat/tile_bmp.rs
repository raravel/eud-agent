use super::tile::invalid;

const PIXELS: usize = 0x436;

pub(super) struct BmpTile {
    pub number: u16,
    pub pixels: Vec<u8>,
}

pub(super) fn tiles(bytes: &[u8]) -> Result<Vec<BmpTile>, crate::e2_import::E2ImportIssue> {
    if bytes.get(..2) != Some(b"BM")
        || read_u32(bytes, 10)? != 0x436
        || read_u16(bytes, 26)? != 1
        || read_u16(bytes, 28)? != 8
    {
        return Err(invalid(
            "custom tile BMP must be indexed 8bpp with pixels at 0x436",
        ));
    }
    let width = usize::try_from(read_u32(bytes, 18)?)
        .map_err(|_| invalid("BMP width cannot be represented"))?;
    let height = usize::try_from(read_u32(bytes, 22)?)
        .map_err(|_| invalid("BMP height cannot be represented"))?;
    if width == 0 || height == 0 || width % 32 != 0 || height % 32 != 0 {
        return Err(invalid(
            "custom tile BMP dimensions must be positive multiples of 32",
        ));
    }
    let end = width
        .checked_mul(height)
        .and_then(|size| PIXELS.checked_add(size))
        .ok_or_else(|| invalid("BMP dimensions overflow"))?;
    if bytes.len() < end {
        return Err(invalid("custom tile BMP pixel payload is truncated"));
    }
    let tiles_x = width / 32;
    let tiles_y = height / 32;
    let mut result = Vec::with_capacity(tiles_x * tiles_y);
    for by in 0..tiles_y {
        for bx in 0..tiles_x {
            let number = u16::try_from(bx + by * tiles_x)
                .map_err(|_| invalid("BMP contains too many tiles"))?;
            let mut pixels = Vec::with_capacity(1_024);
            for mini in 0..16usize {
                for row in 0..8usize {
                    let y = height - 1 - by * 32 - row - (mini / 4) * 8;
                    let start = PIXELS + (mini % 4) * 8 + bx * 32 + y * width;
                    pixels.extend_from_slice(&bytes[start..start + 8]);
                }
            }
            result.push(BmpTile { number, pixels });
        }
    }
    Ok(result)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, crate::e2_import::E2ImportIssue> {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| <[u8; 4]>::try_from(slice).ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| invalid("custom tile BMP header is truncated"))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, crate::e2_import::E2ImportIssue> {
    bytes
        .get(offset..offset + 2)
        .and_then(|slice| <[u8; 2]>::try_from(slice).ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| invalid("custom tile BMP header is truncated"))
}
