use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use crate::{
    native_build::{DatCatalog, DAT_TABLES},
    native_project::{DatScalar, DatTarget, NativeDatState},
};

use super::*;
use crate::e2_import::{
    E2Activation, E2Document, E2IntegerWidth, E2ProjectSettings, E2RawSection, E2RawValue,
    E2SourceKind, E2Version,
};

fn catalog() -> DatCatalog {
    DatCatalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/eud-editor-compat"))
        .unwrap()
}

fn section(name: &str, lines: &[&str], fields: &[(&str, E2RawValue)]) -> E2RawSection {
    E2RawSection {
        name: name.to_string(),
        activation: E2Activation::Active,
        fields: fields
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect(),
        payload: E2RawValue::List(
            lines
                .iter()
                .map(|line| E2RawValue::String((*line).to_string()))
                .collect(),
        ),
    }
}

fn document(raw_sections: Vec<E2RawSection>) -> E2Document {
    E2Document {
        version: E2Version::V0_17_9_7,
        source_kind: E2SourceKind::E2s,
        settings: E2ProjectSettings {
            name: "oracle".to_string(),
            input_map: "input.scx".to_string(),
            output_map: "output.scx".to_string(),
            sound_interval: 0,
            raw_fields: BTreeMap::new(),
        },
        features: Vec::new(),
        triggers: Vec::new(),
        resources: Vec::new(),
        raw_sections,
    }
}

#[test]
fn all_widths_and_families() {
    // Given: 1/2/4-byte DAT rows plus one row in every metadata family.
    let input = document(vec![
        section(
            "DatEditSET",
            &["0,9,0,1", "0,36,0,-1", "0,22,0,1", "0,3,0,1"],
            &[],
        ),
        section(
            "FireGraftSET",
            &[],
            &[("FireGraft7", E2RawValue::String("1,2,3".to_string()))],
        ),
        section(
            "BtnSET",
            &[],
            &[
                ("BtnUse0", E2RawValue::Bool(true)),
                (
                    "BtnData0",
                    E2RawValue::String("1,2,3,4,5,6,7,8,".to_string()),
                ),
            ],
        ),
        section(
            "ReqSET",
            &[],
            &[
                ("ReqUse0,0", E2RawValue::Integer(2)),
                ("ReqUse0,1", E2RawValue::Integer(1)),
                ("ReqUse0,2", E2RawValue::Integer(3)),
                ("ReqCount0,2", E2RawValue::Integer(2)),
                ("ReqData0,2,0", E2RawValue::Integer(65_282)),
                ("ReqData0,2,1", E2RawValue::Integer(5)),
            ],
        ),
        section(
            "FileManagerSET",
            &[],
            &[
                ("stattextdicCount", E2RawValue::Integer(1)),
                ("stattextdickey0", E2RawValue::Integer(7)),
                ("stattextdicvalue0", E2RawValue::String("A#B".to_string())),
                ("wireuse", E2RawValue::Bool(true)),
                ("wireframData1", E2RawValue::Integer(3)),
            ],
        ),
    ]);

    // When: the document is decoded and projected.
    let decoded = decode_dat(&input, &catalog()).unwrap();
    let receipt = project_dat(&decoded, &catalog()).unwrap();

    // Then: widths, VarStart, and all five native families stay distinct.
    assert_eq!(
        decoded
            .deltas
            .iter()
            .map(|row| row.width)
            .collect::<Vec<_>>(),
        [
            E2IntegerWidth::Byte,
            E2IntegerWidth::Word,
            E2IntegerWidth::Dword,
            E2IntegerWidth::Word,
        ]
    );
    assert_eq!(decoded.deltas[0].after - decoded.deltas[0].before, 1);
    assert_eq!(decoded.deltas[1].after - decoded.deltas[1].before, -1);
    assert_eq!(decoded.deltas[3].object_id, 106);
    assert_eq!(decoded.deltas[3].var_start, Some(106));
    assert!(decoded.deltas.iter().all(|row| row.signed));
    assert!(!receipt.state.standard.tables.is_empty());
    assert_eq!(receipt.state.tbl.values[&7].after, "A#B");
    assert_eq!(receipt.state.buttons.values[&0].after, "1,2,3,4,5,6,7,8");
    assert_eq!(receipt.state.requirements.tables["units"][&0].after, "2");
    assert_eq!(receipt.state.requirements.tables["units"][&1].after, "1");
    assert_eq!(
        receipt.state.requirements.tables["units"][&2].after,
        "4.2,5"
    );
    let targets = receipt
        .rows
        .iter()
        .map(|row| row.target.clone())
        .collect::<BTreeSet<_>>();
    for target in [
        DatTarget::Xdat {
            dat: "statusinfor".to_string(),
            object_id: 7,
            field: "Status".to_string(),
        },
        DatTarget::Xdat {
            dat: "statusinfor".to_string(),
            object_id: 7,
            field: "Display".to_string(),
        },
        DatTarget::Xdat {
            dat: "statusinfor".to_string(),
            object_id: 7,
            field: "Debug".to_string(),
        },
        DatTarget::Xdat {
            dat: "wireframe".to_string(),
            object_id: 1,
            field: "wire".to_string(),
        },
        DatTarget::Tbl(7),
        DatTarget::Requirement {
            dat: "units".to_string(),
            object_id: 0,
        },
        DatTarget::Button(0),
    ] {
        assert!(targets.contains(&target), "missing target {target:?}");
    }
}

#[test]
fn all_common_tables_follow_release_order() {
    // Given: field zero from every common E2 table index in release order.
    let rows = (0..DAT_TABLES.len())
        .map(|table| format!("{table},0,0,1"))
        .collect::<Vec<_>>();
    let row_refs = rows.iter().map(String::as_str).collect::<Vec<_>>();
    let expected = [
        ("units", "Graphics", 78, 79, 59),
        ("weapons", "Label", 229, 230, 24),
        ("flingy", "Sprite", 130, 131, 7),
        ("sprites", "Image File", 589, 590, 6),
        ("images", "GRP File", 1, 2, 14),
        ("upgrades", "Mineral Cost Base", 100, 101, 12),
        ("techdata", "Mineral Cost", 100, 101, 10),
        ("orders", "Label", 903, 904, 19),
        ("portdata", "Portrait File", 1, 2, 3),
        ("sfxdata", "Sound File", 0, 1, 5),
    ];

    // When: the indexed rows are decoded and projected.
    let native = catalog();
    let decoded = decode_dat(
        &document(vec![section("DatEditSET", &row_refs, &[])]),
        &native,
    )
    .unwrap();
    let receipt = project_dat(&decoded, &native).unwrap();

    // Then: table names, ordered field names/counts, baselines, and targets match.
    for (index, (table, field, before, after, field_count)) in expected.iter().enumerate() {
        let delta = &decoded.deltas[index];
        assert_eq!(
            (&delta.table, &delta.field, delta.before, delta.after),
            (
                &(*table).to_string(),
                &(*field).to_string(),
                *before,
                *after
            )
        );
        assert_eq!(native.table_fields(table).unwrap().len(), *field_count);
    }
    assert_eq!(receipt.rows.len(), expected.len());
}

include!("tests/sample.rs");
include!("tests/failures.rs");
