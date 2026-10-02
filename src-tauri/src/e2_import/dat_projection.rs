mod decode;
mod metadata;
mod value;

use std::collections::BTreeSet;

use crate::{
    native_build::DatCatalog,
    native_project::{
        DatScalar, DatTarget, NativeDatChange, NativeDatState, NumericOverride, TextOverride,
    },
};

use super::{E2DatProjection, E2ImportIssue};

pub use decode::decode_dat;

/// One validated E2 source row and the native sparse target it resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatProjectionRow {
    pub source: String,
    pub target: DatTarget,
    pub before: DatScalar,
    pub after: DatScalar,
}

/// Pure projection output. Import orchestration persists `state` only after this
/// function has validated every row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatProjectionReceipt {
    pub state: NativeDatState,
    pub rows: Vec<DatProjectionRow>,
}

/// Validates a decoded E2 projection and builds all five sparse native DAT
/// documents without performing persistence.
///
/// # Errors
///
/// Returns a fatal import issue before yielding state when any source row is
/// stale, duplicated, malformed, or outside the catalog.
pub fn project_dat(
    projection: &E2DatProjection,
    catalog: &DatCatalog,
) -> Result<DatProjectionReceipt, E2ImportIssue> {
    let mut receipt = DatProjectionReceipt {
        state: NativeDatState::default(),
        rows: Vec::new(),
    };
    let mut seen = BTreeSet::new();
    for (index, delta) in projection.deltas.iter().enumerate() {
        let meta = catalog
            .field(&delta.table, &delta.field)
            .map_err(metadata::malformed)?;
        let expected_width = decode::width(meta.size)?;
        if delta.width != expected_width || !delta.signed || delta.var_start != Some(meta.var_start)
        {
            return Err(metadata::malformed(format!(
                "DAT row {index} metadata does not match {}.{}",
                delta.table, delta.field
            )));
        }
        let baseline = catalog
            .numeric_value(&delta.table, delta.object_id, &delta.field)
            .map_err(metadata::malformed)?;
        if delta.before != baseline {
            return Err(metadata::malformed(format!(
                "DAT row {index} has stale baseline {} (expected {baseline})",
                delta.before
            )));
        }
        let (min, max) = meta.value_range();
        if delta.after < min || delta.after > max {
            return Err(metadata::malformed(format!(
                "DAT row {index} after value {} is outside {min}..{max}",
                delta.after
            )));
        }
        insert_change(
            &mut receipt,
            &mut seen,
            format!("DatEditSET[{index}]"),
            NativeDatChange::Dat {
                dat: delta.table.clone(),
                object_id: delta.object_id,
                field: delta.field.clone(),
                before: delta.before,
                after: delta.after,
            },
        )?;
    }
    metadata::project_metadata(projection, catalog, &mut receipt, &mut seen)?;
    Ok(receipt)
}

pub(super) fn insert_change(
    receipt: &mut DatProjectionReceipt,
    seen: &mut BTreeSet<DatTarget>,
    source: String,
    change: NativeDatChange,
) -> Result<(), E2ImportIssue> {
    let target = change.target();
    if !seen.insert(target.clone()) {
        return Err(metadata::malformed(format!("duplicate target {target:?}")));
    }
    let (before, after) = match &change {
        NativeDatChange::Dat { before, after, .. }
        | NativeDatChange::Xdat { before, after, .. } => {
            (DatScalar::Number(*before), DatScalar::Number(*after))
        }
        NativeDatChange::Tbl { before, after, .. }
        | NativeDatChange::Requirement { before, after, .. }
        | NativeDatChange::Button { before, after, .. } => (
            DatScalar::Text(before.clone()),
            DatScalar::Text(after.clone()),
        ),
    };
    if before == after {
        return Err(metadata::malformed(format!("no-op target {target:?}")));
    }
    apply_change(&mut receipt.state, change);
    receipt.rows.push(DatProjectionRow {
        source,
        target,
        before,
        after,
    });
    Ok(())
}

fn apply_change(state: &mut NativeDatState, change: NativeDatChange) {
    match change {
        NativeDatChange::Dat {
            dat,
            object_id,
            field,
            before,
            after,
        } => {
            state
                .standard
                .tables
                .entry(dat)
                .or_default()
                .entry(object_id)
                .or_default()
                .insert(field, NumericOverride { before, after });
        }
        NativeDatChange::Xdat {
            dat,
            object_id,
            field,
            before,
            after,
        } => {
            state
                .xdat
                .tables
                .entry(dat)
                .or_default()
                .entry(object_id)
                .or_default()
                .insert(field, NumericOverride { before, after });
        }
        NativeDatChange::Tbl {
            index,
            before,
            after,
        } => {
            state
                .tbl
                .values
                .insert(index, TextOverride { before, after });
        }
        NativeDatChange::Requirement {
            dat,
            object_id,
            before,
            after,
        } => {
            state
                .requirements
                .tables
                .entry(dat)
                .or_default()
                .insert(object_id, TextOverride { before, after });
        }
        NativeDatChange::Button {
            set_id,
            before,
            after,
        } => {
            state
                .buttons
                .values
                .insert(set_id, TextOverride { before, after });
        }
    }
}

#[cfg(test)]
mod tests;
