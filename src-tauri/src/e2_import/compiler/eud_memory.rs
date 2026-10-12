use super::super::definitions::E2Definition;
use super::super::{E2ImportIssue, E2TriggerNode};
use super::metadata::{CompileMetadata, MemoryField};
use super::semantics::{arg, integer, malformed, raw};

pub(super) fn a(node: &E2TriggerNode, definition: &E2Definition, index: usize) -> String {
    arg(node, definition, index)
}

fn index(node: &E2TriggerNode, at: usize, label: &str) -> Result<usize, E2ImportIssue> {
    integer(node.values.get(at))
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| malformed(format!("{} {} has invalid {label}", node.name, node.id)))
}

fn dat_field(
    node: &E2TriggerNode,
    metadata: &CompileMetadata,
    table_at: usize,
    field_at: usize,
) -> Result<MemoryField, E2ImportIssue> {
    metadata
        .dat_field(
            index(node, table_at, "DAT table")?,
            index(node, field_at, "DAT field")?,
        )
        .ok_or_else(|| {
            malformed(format!(
                "{} {} selects an unknown DAT field",
                node.name, node.id
            ))
        })
}

pub(super) fn dat_read(
    node: &E2TriggerNode,
    metadata: &CompileMetadata,
    table_at: usize,
    field_at: usize,
    object_at: usize,
) -> Result<String, E2ImportIssue> {
    let field = dat_field(node, metadata, table_at, field_at)?;
    Ok(format!(
        "{}read(0x{:X} + {} * {})",
        prefix(field.width),
        field.address,
        raw(node, object_at),
        field.width
    ))
}

pub(super) fn dat_write(
    node: &E2TriggerNode,
    definition: &E2Definition,
    metadata: &CompileMetadata,
    add: bool,
) -> Result<String, E2ImportIssue> {
    let field = dat_field(node, metadata, 0, 2)?;
    let address = format!(
        "0x{:X} + {} * {}",
        field.address,
        a(node, definition, 1),
        field.width
    );
    let value = a(node, definition, 3);
    let value = if add {
        format!("{}read({address}) + {value}", prefix(field.width))
    } else {
        value
    };
    Ok(format!("{}write({address}, {value})", prefix(field.width)))
}

fn cunit_field(
    node: &E2TriggerNode,
    metadata: &CompileMetadata,
) -> Result<MemoryField, E2ImportIssue> {
    metadata
        .cunit_field(index(node, 1, "CUnit field")?)
        .ok_or_else(|| {
            malformed(format!(
                "{} {} selects an unknown CUnit field",
                node.name, node.id
            ))
        })
}

pub(super) fn cunit_read(
    node: &E2TriggerNode,
    definition: &E2Definition,
    metadata: &CompileMetadata,
    epd: bool,
) -> Result<String, E2ImportIssue> {
    let field = cunit_field(node, metadata)?;
    let base = a(node, definition, 0);
    Ok(if epd && field.width == 4 {
        format!("dwread_epd({base} + {}/4)", field.address)
    } else if epd {
        format!(
            "{}read_epd({base} + {}/4, {}%4)",
            prefix(field.width),
            field.address,
            field.address
        )
    } else {
        format!("{}read({base} + {})", prefix(field.width), field.address)
    })
}

pub(super) fn cunit_write(
    node: &E2TriggerNode,
    definition: &E2Definition,
    metadata: &CompileMetadata,
    epd: bool,
    add: bool,
) -> Result<String, E2ImportIssue> {
    let field = cunit_field(node, metadata)?;
    let base = a(node, definition, 0);
    let value = a(node, definition, 2);
    if epd && field.width == 4 {
        return Ok(format!(
            "SetMemoryEPD({base} + {}/4, {}, {value})",
            field.address,
            if add { "Add" } else { "SetTo" }
        ));
    }
    if epd && add {
        return Ok(format!(
            "SetMemoryEPD({base} + {}/4, Add, {value} * {})",
            field.address,
            256_u64.pow((field.address as u32) % 4)
        ));
    }
    let address = if epd {
        format!("{base} + {}/4, {}%4", field.address, field.address)
    } else {
        format!("{base} + {}", field.address)
    };
    let value = if add {
        format!("{}read({address}) + {value}", prefix(field.width))
    } else {
        value
    };
    Ok(format!(
        "{}write{}({address}, {value})",
        prefix(field.width),
        if epd { "_epd" } else { "" }
    ))
}

fn prefix(width: u8) -> &'static str {
    match width {
        1 => "b",
        2 => "w",
        _ => "dw",
    }
}

fn upgrade_address(node: &E2TriggerNode, definition: &E2Definition, tech: bool) -> String {
    let maximum = integer(node.values.get(2)) != Some(0);
    let (base, stride, count, adjust) = match (tech, maximum) {
        (false, false) => ("0x58D2B0", "0x207C", 46, 31),
        (false, true) => ("0x58D088", "0x21F0", 46, 31),
        (true, false) => ("0x58CF44", "0x21FC", 24, 4),
        (true, true) => ("0x58CE24", "0x222C", 24, 4),
    };
    format!("{base} + {stride} * ({1}/{count}) + {0} * ({count} - {adjust} * ({1}/{count})) + ({1}%{count})", a(node, definition, 0), a(node, definition, 1))
}

pub(super) fn upgrade_action(
    node: &E2TriggerNode,
    definition: &E2Definition,
    tech: bool,
) -> String {
    let address = upgrade_address(node, definition, tech);
    if integer(node.values.get(4)) == Some(0) {
        format!("bwrite({address}, {})", a(node, definition, 3))
    } else {
        format!(
            "SetMemoryEPD(EPD({address}), {}, bitlshift({}, 8 * (({address}) % 4)))",
            a(node, definition, 4),
            a(node, definition, 3)
        )
    }
}

pub(super) fn upgrade_condition(
    node: &E2TriggerNode,
    definition: &E2Definition,
    tech: bool,
) -> String {
    format!(
        "bread({}) {} {}",
        upgrade_address(node, definition, tech),
        a(node, definition, 4),
        a(node, definition, 3)
    )
}

pub(super) fn assign(
    node: &E2TriggerNode,
    definition: &E2Definition,
    value: &str,
) -> Result<String, E2ImportIssue> {
    let target = definition
        .value_kinds
        .iter()
        .position(|kind| *kind == "Variable")
        .ok_or_else(|| malformed(format!("{} {} has no Variable value", node.name, node.id)))?;
    let modifier = definition
        .value_kinds
        .iter()
        .position(|kind| *kind == "VariableModifier")
        .ok_or_else(|| malformed(format!("{} {} has no modifier value", node.name, node.id)))?;
    Ok(format!(
        "{} {} {value}",
        a(node, definition, target),
        a(node, definition, modifier)
    ))
}

pub(super) fn set_button(
    node: &E2TriggerNode,
    metadata: &CompileMetadata,
) -> Result<String, E2ImportIssue> {
    let (unit, position) = metadata
        .button_position(raw(node, 0))
        .or_else(|| {
            let (unit, _) = raw(node, 0).split_once(':')?;
            Some((unit.parse().ok()?, 0))
        })
        .ok_or_else(|| malformed(format!("SetButton {} has invalid UnitBtn", node.id)))?;
    let words = packed_button(raw(node, 1))
        .ok_or_else(|| malformed(format!("SetButton {} has invalid BtnData", node.id)))?;
    let suffix = node.id.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    let (ptr, pos, old) = (
        format!("btnPtr{suffix:x}"),
        format!("btnPos{suffix:x}"),
        format!("oldCP{suffix:x}"),
    );
    Ok(format!("var {ptr} = EPD(0x5187EC) + 3 * {unit}; var {pos} = 5 * {position}; dwadd_epd({ptr}, 0); {pos} += epdread_epd({ptr}); var {old} = getcurpl(); DoActions(list(SetMemory(0x6509B0, SetTo, {pos}), SetDeaths(CurrentPlayer, SetTo, {}, 0), SetMemory(0x6509B0, Add, 1), SetDeaths(CurrentPlayer, SetTo, {}, 0), SetMemory(0x6509B0, Add, 1), SetDeaths(CurrentPlayer, SetTo, {}, 0), SetMemory(0x6509B0, Add, 1), SetDeaths(CurrentPlayer, SetTo, {}, 0), SetMemory(0x6509B0, Add, 1), SetDeaths(CurrentPlayer, SetTo, {}, 0), SetMemory(0x6509B0, SetTo, {old})))", words[0], words[1], words[2], words[3], words[4]))
}

fn packed_button(stream: &str) -> Option<[u32; 5]> {
    if stream.len() != 60 || !stream.is_ascii() {
        return None;
    }
    let mut words = [0; 5];
    for (word, chunks) in words.iter_mut().zip(stream.as_bytes().chunks_exact(12)) {
        for (shift, digits) in chunks.chunks_exact(3).enumerate() {
            let byte = std::str::from_utf8(digits).ok()?.parse::<u32>().ok()?;
            if byte > 255 {
                return None;
            }
            *word |= byte << (shift * 8);
        }
    }
    Some(words)
}

pub(super) fn button_refresh() -> &'static str {
    "const btntemp1 = wread_epd(EPD(0x6615AA), 2); SetMemoryX(0x6615AA, SetTo, 0x20000, 0xFFFF0000); const btntemp2, btntemp3 = cunitepdread_epd(EPD(0x628438)); CreateUnit(1, 73, 64, 7); if (!Memory(0x628438, Exactly, btntemp2)) { wwrite_epd(btntemp3 + 0x110/4, 0, 1); wwrite_epd(EPD(0x6615AA), 2, btntemp1); }"
}
