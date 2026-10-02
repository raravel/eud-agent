#[test]
fn real_sample_dat_oracle() {
    // Given: the supplied sample's literal 12 release rows.
    let rows = [
        "0,9,0,-4",
        "0,17,0,130",
        "0,19,0,130",
        "0,22,0,2097152",
        "0,22,63,-2097152",
        "0,36,0,-15",
        "0,37,0,-18",
        "0,40,0,-7",
        "0,41,0,-8",
        "0,42,0,-7",
        "0,43,0,-9",
        "3,0,235,-203",
    ];
    let expected = [
        ("units", 0, "Elevation Level", 4, 0),
        ("units", 0, "Ground Weapon", 0, 130),
        ("units", 0, "Air Weapon", 0, 130),
        (
            "units",
            0,
            "Special Ability Flags",
            402_718_720,
            404_815_872,
        ),
        (
            "units",
            63,
            "Special Ability Flags",
            471_859_456,
            469_762_304,
        ),
        ("units", 0, "StarEdit Placement Box Width", 17, 2),
        ("units", 0, "StarEdit Placement Box Height", 20, 2),
        ("units", 0, "Unit Size Left", 8, 1),
        ("units", 0, "Unit Size Up", 9, 1),
        ("units", 0, "Unit Size Right", 8, 1),
        ("units", 0, "Unit Size Down", 10, 1),
        ("sprites", 235, "Image File", 239, 36),
    ];

    // When: the rows are projected against the bundled byte-identical catalog.
    let decoded = decode_dat(
        &document(vec![section("DatEditSET", &rows, &[])]),
        &catalog(),
    )
    .unwrap();
    let receipt = project_dat(&decoded, &catalog()).unwrap();

    // Then: every independently derived baseline and after value is literal.
    let actual = decoded
        .deltas
        .iter()
        .map(|row| {
            (
                row.table.as_str(),
                row.object_id,
                row.field.as_str(),
                row.before,
                row.after,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert_eq!(receipt.rows.len(), 12);
    assert_eq!(receipt.rows[0].before, DatScalar::Number(4));
    assert_eq!(receipt.rows[0].after, DatScalar::Number(0));
    assert_eq!(receipt.rows[11].before, DatScalar::Number(239));
    assert_eq!(receipt.rows[11].after, DatScalar::Number(36));
    if let Ok(path) = std::env::var("EUD_AGENT_E2_SAMPLE") {
        let sample = std::fs::read_to_string(path).unwrap();
        assert!(rows
            .iter()
            .all(|row| sample.lines().any(|line| line == *row)));
    }
}
