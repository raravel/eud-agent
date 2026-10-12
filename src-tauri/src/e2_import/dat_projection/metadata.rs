use std::collections::BTreeSet;

use crate::{
    native_build::DatCatalog,
    native_project::{DatTarget, NativeDatChange},
};

use super::super::{
    E2DatProjection, E2Feature, E2ImportIssue, E2Issue, E2IssueCode, E2IssueSeverity, E2RawSection,
};
use super::value::{
    parse_i64, parse_u32, parse_usize, raw_bool, raw_i64, raw_text, raw_u32, required,
};
use super::{insert_change, DatProjectionReceipt};

pub(super) fn malformed(message: impl Into<String>) -> E2ImportIssue {
    E2Issue {
        code: E2IssueCode::MalformedInput,
        severity: E2IssueSeverity::Fatal,
        message: message.into(),
        path: None,
        feature: Some(E2Feature::Dat),
        need_id: None,
    }
}

pub(super) fn project_metadata(
    projection: &E2DatProjection,
    catalog: &DatCatalog,
    receipt: &mut DatProjectionReceipt,
    seen: &mut BTreeSet<DatTarget>,
) -> Result<(), E2ImportIssue> {
    for section in &projection.xdat {
        match section.name.as_str() {
            "FireGraftSET" => project_firegraft(section, catalog, receipt, seen)?,
            "FileManagerSET" => project_wireframe(section, catalog, receipt, seen)?,
            name => return Err(malformed(format!("unexpected XDAT section {name}"))),
        }
    }
    for section in &projection.tbl {
        if section.name != "FileManagerSET" {
            return Err(malformed(format!(
                "unexpected TBL section {}",
                section.name
            )));
        }
        project_tbl(section, catalog, receipt, seen)?;
    }
    for section in &projection.buttons {
        if section.name != "BtnSET" {
            return Err(malformed(format!(
                "unexpected button section {}",
                section.name
            )));
        }
        project_buttons(section, catalog, receipt, seen)?;
    }
    for section in &projection.requirements {
        if section.name != "ReqSET" {
            return Err(malformed(format!(
                "unexpected requirement section {}",
                section.name
            )));
        }
        requirements::project_requirements(section, catalog, receipt, seen)?;
    }
    Ok(())
}

fn project_firegraft(
    section: &E2RawSection,
    catalog: &DatCatalog,
    receipt: &mut DatProjectionReceipt,
    seen: &mut BTreeSet<DatTarget>,
) -> Result<(), E2ImportIssue> {
    for (key, value) in &section.fields {
        let Some(id) = key.strip_prefix("FireGraft") else {
            continue;
        };
        let object_id = parse_u32(id, key)?;
        let values = raw_text(value, key)?.split(',').collect::<Vec<_>>();
        if values.len() != 3 {
            return Err(malformed(format!("{key} needs status,display,debug")));
        }
        for (field, raw) in ["Status", "Display", "Debug"].into_iter().zip(values) {
            let delta = parse_i64(raw, key)?;
            if delta == 0 {
                continue;
            }
            let before = if field == "Debug" {
                let status_count = u32::try_from(catalog.status_count())
                    .map_err(|_| malformed("status catalog is too large"))?;
                if object_id >= status_count {
                    return Err(malformed(format!("{key} is outside the status catalog")));
                }
                0
            } else {
                catalog
                    .xdat_value("statusinfor", object_id, field)
                    .map_err(malformed)?
            };
            let after = before
                .checked_add(delta)
                .ok_or_else(|| malformed(format!("{key} overflows")))?;
            if (field == "Status" || field == "Display") && !(0..=8).contains(&after) {
                return Err(malformed(format!("{key} {field} is outside 0..8")));
            }
            insert_change(
                receipt,
                seen,
                key.clone(),
                NativeDatChange::Xdat {
                    dat: "statusinfor".to_string(),
                    object_id,
                    field: field.to_string(),
                    before,
                    after,
                },
            )?;
        }
    }
    Ok(())
}

fn project_wireframe(
    section: &E2RawSection,
    catalog: &DatCatalog,
    receipt: &mut DatProjectionReceipt,
    seen: &mut BTreeSet<DatTarget>,
) -> Result<(), E2ImportIssue> {
    if !section
        .fields
        .get("wireuse")
        .map(raw_bool)
        .transpose()?
        .unwrap_or(false)
    {
        return Ok(());
    }
    for (key, value) in &section.fields {
        let mapping = [
            ("wireframData", "wire"),
            ("grpwireData", "grp"),
            ("tranwireData", "tran"),
        ]
        .into_iter()
        .find_map(|(prefix, field)| key.strip_prefix(prefix).map(|id| (id, field)));
        let Some((id, field)) = mapping else { continue };
        let stored = raw_i64(value, key)?;
        if stored == 0 {
            continue;
        }
        let object_id = parse_u32(id, key)?;
        let before = catalog
            .xdat_value("wireframe", object_id, field)
            .map_err(malformed)?;
        let after = stored
            .checked_sub(1)
            .ok_or_else(|| malformed(format!("{key} underflows")))?;
        insert_change(
            receipt,
            seen,
            key.clone(),
            NativeDatChange::Xdat {
                dat: "wireframe".to_string(),
                object_id,
                field: field.to_string(),
                before,
                after,
            },
        )?;
    }
    Ok(())
}

fn project_tbl(
    section: &E2RawSection,
    catalog: &DatCatalog,
    receipt: &mut DatProjectionReceipt,
    seen: &mut BTreeSet<DatTarget>,
) -> Result<(), E2ImportIssue> {
    let Some(count) = section.fields.get("stattextdicCount") else {
        return Ok(());
    };
    let count = parse_usize(
        &raw_i64(count, "stattextdicCount")?.to_string(),
        "stattextdicCount",
    )?;
    for index in 0..count {
        let key_name = format!("stattextdickey{index}");
        let value_name = format!("stattextdicvalue{index}");
        let key = raw_u32(required(section, &key_name)?, &key_name)?;
        let after = raw_text(required(section, &value_name)?, &value_name)?.to_string();
        let before = catalog.tbl_value(key).map_err(malformed)?.to_string();
        insert_change(
            receipt,
            seen,
            value_name,
            NativeDatChange::Tbl {
                index: key,
                before,
                after,
            },
        )?;
    }
    Ok(())
}

fn project_buttons(
    section: &E2RawSection,
    catalog: &DatCatalog,
    receipt: &mut DatProjectionReceipt,
    seen: &mut BTreeSet<DatTarget>,
) -> Result<(), E2ImportIssue> {
    let mut units = section
        .fields
        .iter()
        .filter_map(|(key, value)| key.strip_prefix("BtnUse").map(|unit| (unit, key, value)))
        .collect::<Vec<_>>();
    units.sort_by_key(|(unit, _, _)| unit.parse::<u32>().unwrap_or(u32::MAX));
    for (unit, key, value) in units {
        if !raw_bool(value)? {
            continue;
        }
        let set_id = parse_u32(unit, key)?;
        let data_key = format!("BtnData{set_id}");
        let csv = raw_text(required(section, &data_key)?, &data_key)?;
        let body = csv.strip_suffix(',').unwrap_or(csv);
        let fields = body.split(',').collect::<Vec<_>>();
        if fields.is_empty() || fields.len() % 8 != 0 || fields.iter().any(|field| field.is_empty())
        {
            return Err(malformed(format!("invalid {data_key} field count")));
        }
        for field in &fields {
            parse_u32(field, &data_key)?;
        }
        let after = fields
            .chunks_exact(8)
            .map(|row| row.join(","))
            .collect::<Vec<_>>()
            .join(".");
        let before = catalog.button_csv(set_id).map_err(malformed)?;
        insert_change(
            receipt,
            seen,
            data_key,
            NativeDatChange::Button {
                set_id,
                before,
                after,
            },
        )?;
    }
    Ok(())
}

mod requirements;
