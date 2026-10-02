mod support {
    pub mod e2_fixture;
}

use std::fs;
use std::path::{Path, PathBuf};

use support::e2_fixture::{
    assert_safe_fixture_root, build_complete_fixture, write_confined_fixture_file,
    FixtureExpectation, MAX_FIXTURE_BYTES, MAX_RESOURCE_BYTES, MAX_TEXT_BYTES,
};

fn test_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "eud-agent-e2-fixture-{}-{}",
        std::process::id(),
        name
    ))
}

fn remove_test_root(root: &Path) {
    if root.starts_with(std::env::temp_dir()) {
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn fixture_contract() {
    let first_root = test_root("first");
    let second_root = test_root("second");
    remove_test_root(&first_root);
    remove_test_root(&second_root);

    let first = build_complete_fixture(&first_root, None).expect("fixture should build");
    let second = build_complete_fixture(&second_root, None).expect("fixture should repeat");
    assert_eq!(
        first.oracle_json().expect("oracle JSON"),
        second.oracle_json().expect("oracle JSON")
    );
    assert_eq!(
        first.source_bytes().expect("source bytes"),
        second.source_bytes().expect("source bytes")
    );
    first.verify_oracle().expect("first oracle should match");
    second.verify_oracle().expect("second oracle should match");
    assert_safe_fixture_root(&first.root).expect("fixture root should be safe");
    assert!(first.e2p_root.starts_with(&first.root));
    assert!(first.audit_paths().expect("physical path audit") > 10);

    assert!(first.oracle.source_bytes <= MAX_TEXT_BYTES);
    assert!(first
        .oracle
        .files
        .iter()
        .all(|file| file.bytes <= MAX_RESOURCE_BYTES * 16));
    assert!(
        first
            .oracle
            .files
            .iter()
            .map(|file| file.bytes)
            .sum::<usize>()
            <= MAX_FIXTURE_BYTES
    );
    assert!(first
        .oracle
        .files
        .iter()
        .all(|file| file.path != "Map/output.scx" || file.consumer == "file_manager_or_map"));
    assert!(!first.oracle.output_map_is_input);
    assert_eq!(first.oracle.sections.len(), 13);
    assert!(first
        .source_bytes()
        .expect("source bytes")
        .windows(10)
        .any(|window| window == b"Version : "));

    let names = first
        .oracle
        .features
        .iter()
        .filter_map(|feature| {
            feature
                .id
                .strip_suffix("_enabled")
                .or_else(|| feature.id.strip_suffix("_disabled"))
        })
        .collect::<std::collections::BTreeSet<_>>();
    for name in names {
        let enabled = first
            .oracle
            .features
            .iter()
            .any(|feature| feature.id == format!("{name}_enabled"));
        let disabled = first
            .oracle
            .features
            .iter()
            .any(|feature| feature.id == format!("{name}_disabled"));
        assert!(enabled, "missing enabled pair for {name}");
        assert!(disabled, "missing disabled pair for {name}");
    }
    let firegraft = first
        .oracle
        .features
        .iter()
        .find(|feature| feature.id == "firegraft-buttons_enabled")
        .expect("firegraft row");
    let bgm = first
        .oracle
        .features
        .iter()
        .find(|feature| feature.id == "buttons-bgm_enabled")
        .expect("BGM row");
    assert_eq!(firegraft.gate, "UsedSetting1");
    assert_eq!(bgm.gate, "UsedSetting4");
    assert!(first
        .oracle
        .features
        .iter()
        .any(|feature| feature.id == "tile-memory_enabled"));
    assert!(first
        .oracle
        .features
        .iter()
        .any(|feature| feature.id == "tile-file_enabled"));
    assert!(first
        .oracle
        .features
        .iter()
        .any(|feature| feature.expectation == FixtureExpectation::EnabledNoOp));
    assert!(first
        .oracle
        .features
        .iter()
        .any(|feature| feature.id == "grp_enabled"
            && feature.expectation == FixtureExpectation::Fatal));
    assert!(first
        .oracle
        .features
        .iter()
        .any(|feature| feature.id == "scdb_enabled"
            && feature.expectation == FixtureExpectation::Fatal));
    assert!(first
        .oracle
        .features
        .iter()
        .any(|feature| feature.id == "chatwrite_enabled"
            && feature.expectation == FixtureExpectation::Fatal));

    let source =
        String::from_utf8(first.source_bytes().expect("UTF-8 fixture")).expect("text fixture");
    for section in &first.oracle.sections {
        assert!(
            source.contains(&format!("S_{section}\r\n")),
            "missing start {section}"
        );
        assert!(
            source.contains(&format!("E_{section}\r\n")),
            "missing end {section}"
        );
    }
    assert!(source.contains("UsedSetting1 : True"));
    assert!(source.contains("UsedSetting4 : True"));
    assert!(source.contains("Soundinterval : 3"));
    assert!(source.contains("0,8,43,-2"));
    assert!(first
        .package_file("resources/e2/audio/M0_0.ogg")
        .is_some_and(|bytes| bytes.starts_with(b"OggS")));
    assert!(first
        .package_file("resources/e2/audio/M0_1.ogg")
        .is_some_and(|bytes| bytes.starts_with(b"OggS")));
    assert!(first
        .package_file("resources/e2/plugins/fixture.eps")
        .is_some());

    let changed = first.source_path.clone();
    let original = fs::read(&changed).expect("read source for mutation probe");
    let mutated = String::from_utf8(original.clone())
        .expect("source UTF-8")
        .replacen("0,7,42,3", "0,7,42,4", 1);
    fs::write(&changed, mutated.as_bytes()).expect("mutate source");
    assert!(
        first.verify_oracle().is_err(),
        "changed delta must fail the literal oracle"
    );
    fs::write(&changed, original).expect("restore source");
    first.verify_oracle().expect("restored source should pass");

    let supplied_root = test_root("supplied-map");
    remove_test_root(&supplied_root);
    let supplied_map = supplied_root.join("caller-basemap.scx");
    fs::create_dir_all(&supplied_root).expect("supplied root");
    fs::write(&supplied_map, b"caller supplied map bytes").expect("supplied map");
    let supplied = build_complete_fixture(&supplied_root.join("fixture"), Some(&supplied_map))
        .expect("supplied map fixture");
    assert_eq!(supplied.oracle.map_source, "caller_supplied_copy");
    assert_eq!(
        supplied.package_file("Map/basemap.scx"),
        Some(b"caller supplied map bytes".as_slice())
    );
    assert!(supplied.oracle.input_map_sha256 != first.oracle.input_map_sha256);
    remove_test_root(&first_root);
    remove_test_root(&second_root);
    remove_test_root(&supplied_root);

    println!("E2_FIXTURE_ORACLE_JSON");
    println!("{}", first.oracle_json().expect("oracle JSON"));
    println!("E2_FIXTURE_STATUS PASS");
    assert!(MAX_FIXTURE_BYTES > MAX_TEXT_BYTES);
}

#[test]
fn fixture_root_escape_is_rejected() {
    let root = test_root("escape");
    remove_test_root(&root);
    fs::create_dir_all(&root).expect("test root");
    let outside = root
        .parent()
        .expect("temp parent")
        .join("eud-agent-e2-fixture-escape-outside");
    remove_test_root(&outside);

    let parent_escape = write_confined_fixture_file(
        &root,
        "../eud-agent-e2-fixture-escape-outside/escape.bin",
        b"must not write",
    );
    assert!(parent_escape.is_err());
    assert!(!outside.exists());
    let absolute_escape = write_confined_fixture_file(
        &root,
        &outside.join("absolute.bin").display().to_string(),
        b"must not write",
    );
    assert!(absolute_escape.is_err());
    assert!(!outside.exists());

    let protected = root.join(".research");
    assert!(build_complete_fixture(&protected, None).is_err());
    remove_test_root(&root);
    remove_test_root(&outside);
    println!("E2_FIXTURE_ESCAPE PASS");
}
