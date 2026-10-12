use super::{active_section, artifact, issue, E2ArtifactConsumer, E2CompatRequest};
use crate::e2_import::{E2Feature, E2IssueCode, E2RawSection, E2RawValue, ProjectResourceConsumer};

pub(super) fn compile(
    request: &E2CompatRequest<'_>,
) -> Result<Option<super::GeneratedE2Artifact>, crate::e2_import::E2ImportIssue> {
    let Some(section) = active_section(request.document, "TileSET") else {
        return Ok(None);
    };
    let resources = request
        .resources
        .iter()
        .filter(|copy| copy.resource.consumer == ProjectResourceConsumer::TileCompat)
        .collect::<Vec<_>>();
    let mut output = String::from(
        "function onPluginStart() {\n    var cv5offset = f_epdread_epd(EPD(0x6D5EC8));\n    var vr4offset = f_epdread_epd(EPD(0x628444));\n    var vx4offset = f_epdread_epd(EPD(0x628458));\n",
    );
    if boolean(section, "ProjectTileUseFile", false)? {
        let file = resources
            .iter()
            .find(|copy| {
                super::resource_extension(copy).is_some_and(|ext| ext.eq_ignore_ascii_case("bmp"))
            })
            .ok_or_else(|| invalid("file-backed custom tiles require an owned BMP"))?;
        for tile in super::tile_bmp::tiles(&file.bytes)? {
            render_owned_tile(tile.number, &tile.pixels, &mut output);
        }
    } else {
        render_memory(section, &mut output)?;
    }
    if output.lines().count() == 4 {
        return Ok(None);
    }
    output.push_str("}\n");
    Ok(Some(artifact(
        "TileSET",
        resources
            .iter()
            .map(|copy| copy.resource.id.clone())
            .collect(),
        "src/e2/tile_compat.eps".to_string(),
        "src/e2/tile_compat.eps".to_string(),
        E2ArtifactConsumer::Eps,
        output.into_bytes(),
    )))
}

fn render_memory(
    section: &E2RawSection,
    output: &mut String,
) -> Result<(), crate::e2_import::E2ImportIssue> {
    let count = usize::try_from(integer(section, "ProjectTileSetDataCount", 0)?)
        .map_err(|_| invalid("custom tile count cannot be represented"))?;
    if count > 4_096 {
        return Err(invalid("custom tile count exceeds 4,096"));
    }
    for index in 0..count {
        let tile = u16::try_from(integer_required(
            section,
            &format!("ProjectTileSetDataTileSetNum{index}"),
        )?)
        .map_err(|_| invalid("custom tile number is outside u16"))?;
        let maker = boolean(section, &format!("ProjectTileSetDataisMaker{index}"), false)?;
        let bytes = byte_list(section, &format!("ProjectTileSetDataTileSetData{index}"))?;
        if bytes.len() != 1_024 {
            return Err(invalid(
                "custom tile payload must contain 1,024 indexed pixels",
            ));
        }
        if maker {
            render_owned_tile(tile, &bytes, output);
        } else {
            super::tile_emit::render_existing_tile(tile, &bytes, index, output);
        }
    }
    Ok(())
}

fn render_owned_tile(tile: u16, bytes: &[u8], output: &mut String) {
    let group = u32::from(tile / 16);
    let index = u32::from(tile % 16);
    super::tile_emit::render_halfword(
        "cv5offset",
        0x14 + 52 * group + 2 * index,
        u32::from(tile),
        output,
    );
    for mini in 0..16u16 {
        let vr4 = tile.wrapping_mul(16).wrapping_add(mini);
        super::tile_emit::render_halfword(
            "vx4offset",
            u32::from(tile) * 32 + u32::from(mini) * 2,
            u32::from(vr4) << 1,
            output,
        );
        render_pixels(
            &format!("vr4offset + {}", u32::from(vr4) * 16),
            bytes,
            usize::from(mini) * 64,
            false,
            output,
        );
    }
}

pub(super) fn render_pixels(
    address: &str,
    bytes: &[u8],
    start: usize,
    flip: bool,
    output: &mut String,
) {
    output.push_str(&format!("SetMemory(0x6509B0, SetTo, {address});\n"));
    for row in 0..8usize {
        for word in 0..2usize {
            let offset = start + row * 8 + word * 4;
            let value = if flip {
                let edge = start + row * 8 + 7 - word * 4;
                u32::from_le_bytes([
                    bytes[edge],
                    bytes[edge - 1],
                    bytes[edge - 2],
                    bytes[edge - 3],
                ])
            } else {
                u32::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ])
            };
            output.push_str(&format!("SetDeaths(CurrentPlayer, SetTo, {value}, 0);\n"));
            output.push_str("SetMemory(0x6509B0, Add, 1);\n");
        }
    }
}

fn byte_list(
    section: &E2RawSection,
    key: &str,
) -> Result<Vec<u8>, crate::e2_import::E2ImportIssue> {
    let Some(E2RawValue::String(value)) = section.fields.get(key) else {
        return Err(invalid("custom tile pixel payload is missing"));
    };
    value
        .split(',')
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            part.trim()
                .parse::<u8>()
                .map_err(|_| invalid("custom tile pixel is outside u8"))
        })
        .collect()
}

fn integer(
    section: &E2RawSection,
    key: &str,
    fallback: i64,
) -> Result<i64, crate::e2_import::E2ImportIssue> {
    match section.fields.get(key) {
        None => Ok(fallback),
        Some(E2RawValue::Integer(value)) => Ok(*value),
        Some(_) => Err(invalid("custom tile integer field is malformed")),
    }
}
fn integer_required(
    section: &E2RawSection,
    key: &str,
) -> Result<i64, crate::e2_import::E2ImportIssue> {
    section
        .fields
        .get(key)
        .and_then(|value| match value {
            E2RawValue::Integer(number) => Some(*number),
            _ => None,
        })
        .ok_or_else(|| invalid("custom tile integer field is missing"))
}
fn boolean(
    section: &E2RawSection,
    key: &str,
    fallback: bool,
) -> Result<bool, crate::e2_import::E2ImportIssue> {
    match section.fields.get(key) {
        None => Ok(fallback),
        Some(E2RawValue::Bool(value)) => Ok(*value),
        Some(_) => Err(invalid("custom tile boolean field is malformed")),
    }
}
pub(super) fn invalid(message: impl Into<String>) -> crate::e2_import::E2ImportIssue {
    issue(
        E2IssueCode::InvalidResource,
        E2Feature::CustomTiles,
        message,
    )
}
