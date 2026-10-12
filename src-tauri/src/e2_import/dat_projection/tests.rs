use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use crate::{
    native_build::DatCatalog,
    native_project::{DatScalar, DatTarget},
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
    // Given: exact Editor2 indices and field-schema digests, independent of the native order.
    let expected = [
        ("units", 59, 0x8ae9_9b50_2812_f52f),
        ("weapons", 24, 0x2b04_cf32_b8a9_42c9),
        ("flingy", 7, 0xda94_0dec_5bb4_1662),
        ("sprites", 6, 0xcbd6_2bf5_eebe_f475),
        ("images", 14, 0x23a4_317f_4962_e361),
        ("upgrades", 12, 0xfb87_9127_d47f_968c),
        ("techdata", 10, 0x0ffa_3cc3_de02_d9a1),
        ("orders", 19, 0x7b86_01c7_954f_48fc),
        ("sfxdata", 5, 0x74d7_58df_ead5_356f),
        ("portdata", 3, 0xfb7d_e9ad_1368_25bf),
    ];

    // When: the two historically divergent indices are decoded.
    let native = catalog();
    let decoded = decode_dat(
        &document(vec![section("DatEditSET", &["8,0,0,1", "9,0,0,1"], &[])]),
        &native,
    )
    .unwrap();
    let receipt = project_dat(&decoded, &native).unwrap();

    // Then: indices 8/9 and every ordered field's name, VarStart, and width match release facts.
    assert_eq!(
        decoded
            .deltas
            .iter()
            .map(|row| (&*row.table, &*row.field, row.before, row.after, row.width))
            .collect::<Vec<_>>(),
        [
            ("sfxdata", "Sound File", 0, 1, E2IntegerWidth::Dword),
            ("portdata", "Portrait File", 1, 2, E2IntegerWidth::Dword),
        ]
    );
    for (table, count, expected_hash) in expected {
        let mut fields = native
            .table_fields(table)
            .unwrap()
            .values()
            .collect::<Vec<_>>();
        fields.sort_by_key(|field| field.index);
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for field in &fields {
            for byte in format!(
                "{}|{}|{}|{}\n",
                field.index, field.name, field.var_start, field.size
            )
            .bytes()
            {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
            }
        }
        assert_eq!((fields.len(), hash), (count, expected_hash), "{table}");
    }
    assert_eq!(receipt.rows.len(), 2);
}

include!("tests/sample.rs");
include!("tests/failures.rs");
