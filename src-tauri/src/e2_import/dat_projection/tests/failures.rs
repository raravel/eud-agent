#[test]
fn invalid_projection_matrix() {
    // Given: one valid decoded row and untouched native state.
    let decoded = decode_dat(
        &document(vec![section("DatEditSET", &["0,9,0,-4"], &[])]),
        &catalog(),
    )
    .unwrap();
    let original = NativeDatState::default();

    // When/Then: each stale, duplicate, range, width, and value mutation fails.
    let mut cases = Vec::new();
    let mut stale = decoded.clone();
    stale.deltas[0].before += 1;
    cases.push(stale);
    let mut duplicate = decoded.clone();
    duplicate.deltas.push(duplicate.deltas[0].clone());
    cases.push(duplicate);
    let mut range = decoded.clone();
    range.deltas[0].object_id = u32::MAX;
    cases.push(range);
    let mut width = decoded.clone();
    width.deltas[0].width = E2IntegerWidth::Dword;
    cases.push(width);
    let mut value = decoded.clone();
    value.deltas[0].after = 256;
    cases.push(value);
    let mut unsigned = decoded.clone();
    unsigned.deltas[0].signed = false;
    cases.push(unsigned);
    let mut wrong_start = decoded.clone();
    wrong_start.deltas[0].var_start = Some(1);
    cases.push(wrong_start);
    let mut unsupported = decoded.clone();
    unsupported.deltas[0].table = "mapdata".to_string();
    cases.push(unsupported);
    for case in cases {
        assert!(project_dat(&case, &catalog()).is_err());
        assert_eq!(original, NativeDatState::default());
    }
    let malformed_button = E2DatProjection {
        deltas: Vec::new(),
        xdat: Vec::new(),
        tbl: Vec::new(),
        requirements: Vec::new(),
        buttons: vec![section(
            "BtnSET",
            &[],
            &[
                ("BtnUse0", E2RawValue::Bool(true)),
                (
                    "BtnData0",
                    E2RawValue::String("1,,3,4,5,6,7,8,".to_string()),
                ),
            ],
        )],
    };
    assert!(project_dat(&malformed_button, &catalog()).is_err());
    assert_eq!(
        DatScalar::Number(4),
        DatScalar::Number(decoded.deltas[0].before)
    );
}

#[test]
fn inactive_payload_is_inert_but_active_payload_is_validated() {
    // Given: malformed data retained behind inactive feature gates.
    let mut inert_dat = section("DatEditSET", &["malformed"], &[]);
    inert_dat.activation = E2Activation::Inert;
    let mut inert_buttons = section(
        "BtnSET",
        &[],
        &[
            ("BtnUse0", E2RawValue::Bool(true)),
            ("BtnData0", E2RawValue::String("malformed".to_string())),
        ],
    );
    inert_buttons.activation = E2Activation::Inert;

    // When: inactive provenance is decoded and projected.
    let decoded = decode_dat(
        &document(vec![inert_dat.clone(), inert_buttons.clone()]),
        &catalog(),
    )
    .unwrap();
    let receipt = project_dat(&decoded, &catalog()).unwrap();

    // Then: it remains inert, while the same active payload fails its boundary.
    assert!(receipt.rows.is_empty());
    inert_dat.activation = E2Activation::Active;
    assert!(decode_dat(&document(vec![inert_dat]), &catalog()).is_err());
    inert_buttons.activation = E2Activation::Active;
    let active = decode_dat(&document(vec![inert_buttons]), &catalog()).unwrap();
    assert!(project_dat(&active, &catalog()).is_err());
}
