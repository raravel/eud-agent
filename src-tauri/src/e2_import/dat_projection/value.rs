use super::super::{E2ImportIssue, E2RawSection, E2RawValue};
use super::metadata::malformed;

pub(super) fn required<'a>(
    section: &'a E2RawSection,
    key: &str,
) -> Result<&'a E2RawValue, E2ImportIssue> {
    section
        .fields
        .get(key)
        .ok_or_else(|| malformed(format!("missing {key}")))
}

pub(super) fn raw_text<'a>(value: &'a E2RawValue, key: &str) -> Result<&'a str, E2ImportIssue> {
    match value {
        E2RawValue::String(value) => Ok(value),
        _ => Err(malformed(format!("{key} must be text"))),
    }
}

pub(super) fn raw_i64(value: &E2RawValue, key: &str) -> Result<i64, E2ImportIssue> {
    match value {
        E2RawValue::Integer(value) => Ok(*value),
        E2RawValue::String(value) => parse_i64(value, key),
        _ => Err(malformed(format!("{key} must be an integer"))),
    }
}

pub(super) fn raw_u32(value: &E2RawValue, key: &str) -> Result<u32, E2ImportIssue> {
    u32::try_from(raw_i64(value, key)?).map_err(|_| malformed(format!("invalid {key}")))
}

pub(super) fn raw_bool(value: &E2RawValue) -> Result<bool, E2ImportIssue> {
    match value {
        E2RawValue::Bool(value) => Ok(*value),
        _ => Err(malformed("value must be boolean")),
    }
}

pub(super) fn parse_i64(value: &str, key: &str) -> Result<i64, E2ImportIssue> {
    value
        .parse()
        .map_err(|_| malformed(format!("invalid {key}")))
}

pub(super) fn parse_u32(value: &str, key: &str) -> Result<u32, E2ImportIssue> {
    value
        .parse()
        .map_err(|_| malformed(format!("invalid {key}")))
}

pub(super) fn parse_usize(value: &str, key: &str) -> Result<usize, E2ImportIssue> {
    value
        .parse()
        .map_err(|_| malformed(format!("invalid {key}")))
}
