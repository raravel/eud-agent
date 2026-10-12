use std::collections::BTreeSet;

use crate::{
    native_build::DatCatalog,
    native_project::{DatTarget, NativeDatChange},
};

use super::super::super::{E2ImportIssue, E2RawSection};
use super::super::{insert_change, value, DatProjectionReceipt};
use super::malformed;

const TABLES: [&str; 5] = ["units", "upgrades", "techdata", "Stechdata", "orders"];

pub(super) fn project_requirements(
    section: &E2RawSection,
    catalog: &DatCatalog,
    receipt: &mut DatProjectionReceipt,
    seen: &mut BTreeSet<DatTarget>,
) -> Result<(), E2ImportIssue> {
    for (key, raw) in &section.fields {
        let Some(indices) = key.strip_prefix("ReqUse") else {
            continue;
        };
        let (table, object_id) = target(indices, key)?;
        let mode = value::raw_u32(raw, key)?;
        if mode == 0 {
            continue;
        }
        let after = match mode {
            1 | 2 => mode.to_string(),
            3 => custom(section, indices)?,
            _ => return Err(malformed(format!("{key} mode must be 0..3"))),
        };
        let before = catalog
            .requirement_payload(table, object_id)
            .map_err(malformed)?;
        insert_change(
            receipt,
            seen,
            key.clone(),
            NativeDatChange::Requirement {
                dat: table.to_string(),
                object_id,
                before,
                after,
            },
        )?;
    }
    Ok(())
}

fn custom(section: &E2RawSection, indices: &str) -> Result<String, E2ImportIssue> {
    let count_key = format!("ReqCount{indices}");
    let count = value::raw_u32(value::required(section, &count_key)?, &count_key)?;
    let mut words = Vec::new();
    for index in 0..count {
        let key = format!("ReqData{indices},{index}");
        let word = value::raw_u32(value::required(section, &key)?, &key)?;
        words.push(u16::try_from(word).map_err(|_| malformed(format!("{key} exceeds u16")))?);
    }
    let mut segments = vec!["4".to_string()];
    let mut index = 0;
    while index < words.len() {
        let raw = words[index];
        index += 1;
        if raw <= 0xff {
            segments.push(format!("0,{raw}"));
            continue;
        }
        let opcode = raw
            .checked_sub(0xff00)
            .ok_or_else(|| malformed(format!("invalid requirement word {raw}")))?;
        if matches!(opcode, 2 | 3 | 4 | 37) {
            let operand = words
                .get(index)
                .ok_or_else(|| malformed("requirement opcode is missing a value"))?;
            index += 1;
            segments.push(format!("{opcode},{operand}"));
        } else {
            segments.push(opcode.to_string());
        }
    }
    Ok(segments.join("."))
}

fn target(indices: &str, key: &str) -> Result<(&'static str, u32), E2ImportIssue> {
    let (table, object) = indices
        .split_once(',')
        .ok_or_else(|| malformed(format!("invalid {key}")))?;
    let table = TABLES
        .get(value::parse_usize(table, key)?)
        .ok_or_else(|| malformed(format!("invalid {key}")))?;
    Ok((table, value::parse_u32(object, key)?))
}
