use crate::native_build::DatCatalog;

use super::super::{
    E2Activation, E2DatDelta, E2DatProjection, E2ImportIr, E2ImportIssue, E2IntegerWidth,
    E2RawSection, E2RawValue,
};
use super::metadata::malformed;

const E2_DAT_TABLES: [&str; 10] = [
    "units", "weapons", "flingy", "sprites", "images", "upgrades", "techdata", "orders", "sfxdata",
    "portdata",
];

/// Decodes the ordered E2 DAT rows and active metadata sections.
///
/// # Errors
///
/// Returns a fatal import issue when a row cannot be resolved against the
/// versioned native catalog.
pub fn decode_dat(
    document: &E2ImportIr,
    catalog: &DatCatalog,
) -> Result<E2DatProjection, E2ImportIssue> {
    let mut deltas = Vec::new();
    if let Some(section) = active_section(document, "DatEditSET") {
        let E2RawValue::List(lines) = &section.payload else {
            return Err(malformed("DatEditSET payload must be a list"));
        };
        for (row_index, value) in lines.iter().enumerate() {
            let E2RawValue::String(line) = value else {
                return Err(malformed("DAT row must be text"));
            };
            if !line.trim().is_empty() {
                deltas.push(decode_delta(line, row_index, catalog)?);
            }
        }
    }
    Ok(E2DatProjection {
        deltas,
        xdat: active_sections(document, &["FireGraftSET", "FileManagerSET"]),
        tbl: active_sections(document, &["FileManagerSET"]),
        requirements: active_sections(document, &["ReqSET"]),
        buttons: active_sections(document, &["BtnSET"]),
    })
}

fn decode_delta(
    line: &str,
    row_index: usize,
    catalog: &DatCatalog,
) -> Result<E2DatDelta, E2ImportIssue> {
    let values = line.split(',').map(str::trim).collect::<Vec<_>>();
    if values.len() != 4 {
        return Err(malformed(format!("malformed DAT row {row_index}")));
    }
    let number = |at: usize| {
        values[at]
            .parse::<i64>()
            .map_err(|_| malformed(format!("malformed DAT row {row_index}")))
    };
    let table_index = usize::try_from(number(0)?)
        .map_err(|_| malformed(format!("invalid DAT table in row {row_index}")))?;
    let field_index = u32::try_from(number(1)?)
        .map_err(|_| malformed(format!("invalid DAT field in row {row_index}")))?;
    let local_index = u32::try_from(number(2)?)
        .map_err(|_| malformed(format!("invalid DAT index in row {row_index}")))?;
    let delta = number(3)?;
    if delta == 0 {
        return Err(malformed(format!("DAT row {row_index} is a no-op")));
    }
    let table = E2_DAT_TABLES
        .get(table_index)
        .ok_or_else(|| malformed(format!("invalid DAT table in row {row_index}")))?;
    let meta = catalog
        .table_fields(table)
        .and_then(|fields| fields.values().find(|field| field.index == field_index))
        .ok_or_else(|| malformed(format!("invalid DAT field in row {row_index}")))?;
    let object_id = meta
        .var_start
        .checked_add(local_index)
        .ok_or_else(|| malformed(format!("DAT index overflow in row {row_index}")))?;
    let before = catalog
        .numeric_value(table, object_id, &meta.name)
        .map_err(malformed)?;
    let after = before
        .checked_add(delta)
        .ok_or_else(|| malformed(format!("DAT value overflow in row {row_index}")))?;
    let (min, max) = meta.value_range();
    if after < min || after > max {
        return Err(malformed(format!(
            "DAT row {row_index} after value {after} is outside {min}..{max}"
        )));
    }
    Ok(E2DatDelta {
        table: (*table).to_string(),
        object_id,
        field: meta.name.clone(),
        before,
        after,
        width: width(meta.size)?,
        signed: true,
        var_start: Some(meta.var_start),
    })
}

pub(super) fn width(size: u8) -> Result<E2IntegerWidth, E2ImportIssue> {
    match size {
        1 => Ok(E2IntegerWidth::Byte),
        2 => Ok(E2IntegerWidth::Word),
        4 => Ok(E2IntegerWidth::Dword),
        _ => Err(malformed(format!("unsupported DAT width {size}"))),
    }
}

fn active_section<'a>(document: &'a E2ImportIr, name: &str) -> Option<&'a E2RawSection> {
    document
        .raw_sections
        .iter()
        .find(|section| section.name == name && section.activation == E2Activation::Active)
}

fn active_sections(document: &E2ImportIr, names: &[&str]) -> Vec<E2RawSection> {
    document
        .raw_sections
        .iter()
        .filter(|section| {
            section.activation == E2Activation::Active && names.contains(&section.name.as_str())
        })
        .cloned()
        .collect()
}
