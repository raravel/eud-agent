use super::{active_section, artifact, issue, E2ArtifactConsumer, E2CompatRequest};
use crate::e2_import::{E2Feature, E2IssueCode, E2RawValue, ProjectResourceConsumer};

pub(super) const EPD_BASE: u32 = 0x58A364;

pub(super) fn compile(
    request: &E2CompatRequest<'_>,
) -> Result<Option<super::GeneratedE2Artifact>, crate::e2_import::E2ImportIssue> {
    let Some(section) = active_section(request.document, "BinEditSET") else {
        return Ok(None);
    };
    let resources = request
        .resources
        .iter()
        .filter(|copy| copy.resource.consumer == ProjectResourceConsumer::BinCompat)
        .collect::<Vec<_>>();
    let direct = section
        .fields
        .iter()
        .filter(|(key, _)| {
            key.strip_prefix("Bin")
                .is_some_and(|suffix| suffix.parse::<u16>().is_ok())
        })
        .collect::<Vec<_>>();
    let mut output = String::from("function onPluginStart() {\n");
    let scene_mode = direct.is_empty();
    if !scene_mode {
        for (index, (_, value)) in direct.iter().enumerate() {
            let E2RawValue::String(row) = value else {
                return Err(invalid("BIN direct row must be text"));
            };
            render_direct(row, resources.get(index).copied(), &mut output)?;
        }
    } else {
        super::bin_baseline::render_scenes(request, section, &mut output)?;
    }
    if output.lines().count() == 1 {
        return Ok(None);
    }
    if scene_mode {
        output.insert_str(
            0,
            &format!(
                "// E2 0.17.9.7 BIN facts: commit={}, digest={}\n",
                super::bin_baseline::SOURCE_COMMIT,
                super::bin_baseline::CANONICAL_DIGEST
            ),
        );
    }
    output.push_str("}\n");
    Ok(Some(artifact(
        "BinEditSET",
        resources
            .iter()
            .map(|copy| copy.resource.id.clone())
            .collect(),
        "src/e2/bin_compat.eps".to_string(),
        "src/e2/bin_compat.eps".to_string(),
        E2ArtifactConsumer::Eps,
        output.into_bytes(),
    )))
}

fn render_direct(
    row: &str,
    resource: Option<&crate::e2_import::resources::E2PlannedCopy>,
    output: &mut String,
) -> Result<(), crate::e2_import::E2ImportIssue> {
    let values = row
        .split(',')
        .filter_map(|item| item.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let race = parse_u32(required(&values, "PlayerRace")?)?;
    if race == 255 {
        return Ok(());
    }
    if race > 2 {
        return Err(invalid("BIN PlayerRace must be 0, 1, 2, or 255"));
    }
    let pointer = parse_u32(required(&values, "pointer")?)?;
    validate_pointer(pointer)?;
    let width = parse_u32(required(&values, "width")?)?;
    if !matches!(width, 1 | 2 | 4) {
        return Err(invalid("BIN write width must be 1, 2, or 4"));
    }
    let declared = parse_u32(required(&values, "value")?)?;
    let value = match resource {
        Some(copy) => owned_value(&copy.bytes, width, declared)?,
        None => declared,
    };
    output.push_str(&format!(
        "SetMemory(0x{pointer:08X}, SetTo, 0x{value:08X}); // width={width}\n"
    ));
    Ok(())
}

fn owned_value(
    bytes: &[u8],
    width: u32,
    declared: u32,
) -> Result<u32, crate::e2_import::E2ImportIssue> {
    let count = usize::try_from(width).map_err(|_| invalid("BIN width cannot be represented"))?;
    let slice = bytes
        .get(..count)
        .ok_or_else(|| invalid("owned BIN payload is shorter than its write width"))?;
    let mut padded = [0u8; 4];
    padded[..count].copy_from_slice(slice);
    let value = u32::from_le_bytes(padded);
    if value != declared {
        return Err(invalid(
            "owned BIN payload does not match its declared write value",
        ));
    }
    Ok(value)
}

fn required<'a>(
    values: &'a std::collections::BTreeMap<&str, &str>,
    key: &str,
) -> Result<&'a str, crate::e2_import::E2ImportIssue> {
    values
        .get(key)
        .copied()
        .ok_or_else(|| invalid(format!("BIN row is missing {key}")))
}

fn parse_u32(value: &str) -> Result<u32, crate::e2_import::E2ImportIssue> {
    value
        .strip_prefix("0x")
        .map_or_else(|| value.parse(), |hex| u32::from_str_radix(hex, 16))
        .map_err(|_| invalid("BIN numeric value is malformed"))
}

pub(super) fn validate_pointer(pointer: u32) -> Result<(), crate::e2_import::E2ImportIssue> {
    if !(0x400000..=0x800000).contains(&pointer) || pointer % 4 != 0 {
        Err(invalid("BIN pointer is outside the aligned EPD range"))
    } else {
        Ok(())
    }
}

pub(super) fn selected(race: u8, row: u8) -> bool {
    row <= 4
        || match race {
            0 => (5..=10).contains(&row),
            1 => (11..=17).contains(&row),
            2 => (18..=25).contains(&row),
            _ => false,
        }
}

pub(super) fn pack(low: i16, high: i16) -> u32 {
    u32::from(u16::from_le_bytes(low.to_le_bytes()))
        | (u32::from(u16::from_le_bytes(high.to_le_bytes())) << 16)
}

pub(super) fn operation(value: i16, old: i16) -> &'static str {
    if value >= old {
        "Add"
    } else {
        "Subtract"
    }
}
pub(super) fn delta(value: i16, old: i16) -> u32 {
    u32::from(value.abs_diff(old))
}
pub(super) fn invalid(message: impl Into<String>) -> crate::e2_import::E2ImportIssue {
    issue(E2IssueCode::InvalidResource, E2Feature::Bin, message)
}
