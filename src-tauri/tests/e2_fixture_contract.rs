mod support {
    pub mod e2_fixture;
}

use std::fs;
use std::path::{Path, PathBuf};

use support::e2_fixture::{
    assert_safe_fixture_root, build_complete_fixture, write_confined_fixture_file, FixtureBundle,
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

struct ExpectedFeature {
    id: &'static str,
    case_path: &'static str,
    source_lines: &'static [&'static str],
    resources: &'static [&'static str],
    expectation: FixtureExpectation,
    ir: &'static str,
    eap: &'static str,
    build: &'static str,
    artifact: &'static str,
}

fn assert_literal_feature_records(bundle: &FixtureBundle) -> Result<(), String> {
    let enabled = [
        ExpectedFeature {
            id: "dat_enabled",
            case_path: "complete.e2s",
            source_lines: &["UsedSetting0 : True", "0,7,42,3", "0,8,43,-2"],
            resources: &[],
            expectation: FixtureExpectation::Enabled,
            ir: "DAT[0,7,42]=+3 width=1; DAT[0,8,43]=-2 width=2",
            eap: "dat/standard.json:delta records",
            build: "DataEditor.py:AddSubtract(width-aware)",
            artifact: "src/e2/dat_delta.eps",
        },
        ExpectedFeature {
            id: "firegraft-buttons_enabled",
            case_path: "complete.e2s",
            source_lines: &["UsedSetting1 : True", "FireGraft7 : 1,2,3"],
            resources: &[],
            expectation: FixtureExpectation::Enabled,
            ir: "FireGraft[7]=(1,2,3); BtnSet injection",
            eap: "dat/xdat.json + dat/buttons.json",
            build: "ExtraDataEditor.py + button consumer",
            artifact: "src/e2/firegraft_buttons.eps",
        },
        ExpectedFeature {
            id: "bin_enabled",
            case_path: "complete.e2s",
            source_lines: &[
                "UsedSetting2 : True",
                "Bin0 : object=7,pointer=0x006D5EC8,width=4,value=0x00000002,PlayerRace=0",
            ],
            resources: &["resources/e2/bin/fixture.bin"],
            expectation: FixtureExpectation::Enabled,
            ir: "object=7 pointer=0x006D5EC8 race=0 width=4",
            eap: "resources/e2/bin/fixture.bin",
            build: "bounded BIN compatibility consumer",
            artifact: "src/e2/bin_compat.eps",
        },
        ExpectedFeature {
            id: "tile-memory_enabled",
            case_path: "complete.e2s",
            source_lines: &[
                "UsedSetting3 : True",
                "ProjectTileUseFile : False",
                "ProjectTIleMSet0 : 73",
            ],
            resources: &[
                "resources/e2/tiles/fixture.cv5",
                "resources/e2/tiles/fixture.vx4",
                "resources/e2/tiles/fixture.vr4",
            ],
            expectation: FixtureExpectation::Enabled,
            ir: "CV5(4,9),VX4=0x0021,VR4=0x0034",
            eap: "resources/e2/tiles/*.cv5|vx4|vr4",
            build: "in-memory tile halfword patch",
            artifact: "src/e2/tile_compat.eps",
        },
        ExpectedFeature {
            id: "tile-file_enabled",
            case_path: "cases/tile-file.e2s",
            source_lines: &[
                "UsedSetting3 : True",
                "ProjectTileUseFile : True",
                "ProjectTileSetFileName : resources/e2/tiles/file-mode/fixture.bmp",
            ],
            resources: &["resources/e2/tiles/file-mode/fixture.bmp"],
            expectation: FixtureExpectation::Enabled,
            ir: "ProjectTileUseFile=true file refs",
            eap: "resources/e2/tiles/file-mode/*",
            build: "file-backed tile patch",
            artifact: "src/e2/tile_file_compat.eps",
        },
        ExpectedFeature {
            id: "buttons-bgm_enabled",
            case_path: "complete.e2s",
            source_lines: &[
                "UsedSetting4 : True",
                "Soundinterval : 3",
                "Sound0 : M0_0.ogg",
                "Sound1 : M0_1.ogg",
            ],
            resources: &["resources/e2/audio/M0_0.ogg", "resources/e2/audio/M0_1.ogg"],
            expectation: FixtureExpectation::Enabled,
            ir: "interval=3; M0_0.ogg,M0_1.ogg contiguous",
            eap: "dat/buttons.json + resources/e2/audio",
            build: "ordered MPQAddFile aliases",
            artifact: "src/e2/bgm.eps",
        },
        ExpectedFeature {
            id: "grp_enabled",
            case_path: "cases/effective-grp.e2s",
            source_lines: &["UsedSetting5 : True", "GRPUse0 : True"],
            resources: &[],
            expectation: FixtureExpectation::Fatal,
            ir: "effective replacement/use record",
            eap: "fatal inventory only",
            build: "active_grp_unsupported",
            artifact: "none",
        },
        ExpectedFeature {
            id: "struct-gui_enabled",
            case_path: "complete.e2s",
            source_lines: &["UsedSetting6 : True", "act:DisplayText"],
            resources: &[],
            expectation: FixtureExpectation::Enabled,
            ir: "recursive node order/flags/raw values",
            eap: "src/main.eps provenance",
            build: "EUDLoop lifecycle compiler",
            artifact: "src/main.eps",
        },
        ExpectedFeature {
            id: "plugin_enabled",
            case_path: "complete.e2s",
            source_lines: &[
                "UsedSetting7 : True",
                "fixturePlugin : resources/e2/plugins/fixture.eps",
                "fixturePluginEnabled : True",
            ],
            resources: &["resources/e2/plugins/fixture.eps"],
            expectation: FixtureExpectation::Enabled,
            ir: "ordered fixturePlugin entrypoint",
            eap: "plugins/fixture.eps",
            build: "ordered plugin consumer",
            artifact: "src/e2/plugin_compat.eps",
        },
        ExpectedFeature {
            id: "file-manager_enabled",
            case_path: "complete.e2s",
            source_lines: &[
                "UsedSetting8 : True",
                "stat_txt : resources/e2/files/stat_txt.tbl",
                "iscript : resources/e2/files/iscript.bin",
                "aiscript : resources/e2/files/aiscript.bin",
            ],
            resources: &[
                "resources/e2/files/stat_txt.tbl",
                "resources/e2/files/iscript.bin",
                "resources/e2/files/aiscript.bin",
            ],
            expectation: FixtureExpectation::Enabled,
            ir: "TBL/iscript/aiscript owned refs",
            eap: "resources/e2/files",
            build: "TBL/wireframe/binary consumer",
            artifact: "src/e2/file_manager.eps",
        },
        ExpectedFeature {
            id: "scdb_enabled",
            case_path: "cases/active-scdb.e2s",
            source_lines: &["SCDBUse : True", "SCDBDeath : 1,2"],
            resources: &[],
            expectation: FixtureExpectation::Fatal,
            ir: "SCDBUse=true inventory",
            eap: "fatal inventory only",
            build: "active_scdb_unsupported",
            artifact: "none",
        },
        ExpectedFeature {
            id: "sound_enabled",
            case_path: "complete.e2s",
            source_lines: &[
                "Soundinterval : 3",
                "Sound0 : M0_0.ogg",
                "Sound1 : M0_1.ogg",
            ],
            resources: &["resources/e2/audio/M0_0.ogg", "resources/e2/audio/M0_1.ogg"],
            expectation: FixtureExpectation::Enabled,
            ir: "M0_0 then M0_1; interval*1050-70",
            eap: "resources/e2/audio/*.ogg",
            build: "BGM player + MPQ aliases",
            artifact: "src/e2/bgm.eps",
        },
        ExpectedFeature {
            id: "chatwrite_enabled",
            case_path: "cases/active-chatwrite.e2s",
            source_lines: &["UsedSetting6 : True", "act:ChatWrite"],
            resources: &[],
            expectation: FixtureExpectation::Fatal,
            ir: "ChatWrite preserved as unresolved action",
            eap: "no authority",
            build: "active_chatwrite_unverified",
            artifact: "none",
        },
    ];
    for expected in enabled {
        assert_literal_feature(bundle, &expected)?;
    }
    for (id, line, ir) in [
        (
            "dat_disabled",
            "UsedSetting0 : False",
            "inert provenance: DAT[0,7,42]=+3 width=1; DAT[0,8,43]=-2 width=2",
        ),
        (
            "firegraft-buttons_disabled",
            "UsedSetting1 : False",
            "inert provenance: FireGraft[7]=(1,2,3); BtnSet injection",
        ),
        (
            "bin_disabled",
            "UsedSetting2 : False",
            "inert provenance: object=7 pointer=0x006D5EC8 race=0 width=4",
        ),
        (
            "tile-memory_disabled",
            "UsedSetting3 : False",
            "inert provenance: CV5(4,9),VX4=0x0021,VR4=0x0034",
        ),
        (
            "tile-file_disabled",
            "UsedSetting3 : False",
            "inert provenance: ProjectTileUseFile=true file refs",
        ),
        (
            "buttons-bgm_disabled",
            "UsedSetting4 : False",
            "inert provenance: interval=3; M0_0.ogg,M0_1.ogg contiguous",
        ),
        (
            "grp_disabled",
            "UsedSetting5 : False",
            "inert provenance: effective replacement/use record",
        ),
        (
            "struct-gui_disabled",
            "UsedSetting6 : False",
            "inert provenance: recursive node order/flags/raw values",
        ),
        (
            "plugin_disabled",
            "UsedSetting7 : False",
            "inert provenance: ordered fixturePlugin entrypoint",
        ),
        (
            "file-manager_disabled",
            "UsedSetting8 : False",
            "inert provenance: TBL/iscript/aiscript owned refs",
        ),
        (
            "scdb_disabled",
            "SCDBUse : False",
            "inert provenance: SCDBUse=true inventory",
        ),
        (
            "sound_disabled",
            "UsedSetting4 : False",
            "inert provenance: M0_0 then M0_1; interval*1050-70",
        ),
        (
            "chatwrite_disabled",
            "UsedSetting6 : False",
            "inert provenance: ChatWrite preserved as unresolved action",
        ),
    ] {
        let feature = bundle
            .oracle
            .features
            .iter()
            .find(|feature| feature.id == id)
            .ok_or_else(|| format!("missing literal feature {id}"))?;
        if feature.expectation != FixtureExpectation::Disabled
            || feature.source.case_path != "cases/disabled-payload.e2s"
            || feature.oracle.ir != ir
            || feature.oracle.eap != "optional import provenance only"
            || feature.oracle.build != "no active section"
            || feature.oracle.artifact != "none"
        {
            return Err(format!(
                "literal disabled three-layer record changed for {id}"
            ));
        }
        let source = fs::read_to_string(bundle.root.join("cases/disabled-payload.e2s"))
            .map_err(|error| error.to_string())?;
        if !source.contains(line)
            || !feature
                .source
                .required_lines
                .iter()
                .any(|actual| actual == line)
        {
            return Err(format!(
                "disabled source observation changed for {id}: {line}"
            ));
        }
    }
    Ok(())
}

fn assert_literal_failure_records(bundle: &FixtureBundle) -> Result<(), String> {
    for (id, path, diagnostic) in [
        (
            "unknown_version",
            "cases/unknown-version.e2s",
            "unsupported_e2_version",
        ),
        ("malformed", "cases/malformed.e2s", "malformed_e2_section"),
        ("over_bound", "cases/over-bound.e2s", "e2_input_too_large"),
        (
            "active_scdb",
            "cases/active-scdb.e2s",
            "active_scdb_unsupported",
        ),
        (
            "effective_grp",
            "cases/effective-grp.e2s",
            "active_grp_unsupported",
        ),
        (
            "active_chatwrite",
            "cases/active-chatwrite.e2s",
            "active_chatwrite_unverified",
        ),
        (
            "missing_map",
            "cases/missing-map.e2p/project.e2s",
            "needs_relink_missing_input_map",
        ),
        (
            "ambiguous_map",
            "cases/ambiguous-map.e2p/project.e2s",
            "needs_relink_ambiguous_input_map",
        ),
        (
            "stale_map",
            "cases/stale-map.e2p/project.e2s",
            "needs_relink_stale_input_map",
        ),
    ] {
        let actual = bundle
            .oracle
            .failures
            .iter()
            .find(|failure| failure.id == id)
            .ok_or_else(|| format!("missing literal failure {id}"))?;
        if actual.path != path
            || actual.diagnostic != diagnostic
            || !bundle.root.join(path).is_file()
        {
            return Err(format!("failure record changed for {id}"));
        }
    }
    if fs::metadata(bundle.root.join("cases/over-bound.e2s"))
        .map_err(|error| error.to_string())?
        .len()
        != u64::try_from(MAX_TEXT_BYTES + 1).map_err(|error| error.to_string())?
    {
        return Err("over-bound input did not retain its literal boundary+1 size".to_string());
    }
    let unknown = fs::read_to_string(bundle.root.join("cases/unknown-version.e2s"))
        .map_err(|error| error.to_string())?;
    let malformed = fs::read_to_string(bundle.root.join("cases/malformed.e2s"))
        .map_err(|error| error.to_string())?;
    let stale = fs::read_to_string(bundle.root.join("cases/stale-map.e2p/project.e2s"))
        .map_err(|error| error.to_string())?;
    if !unknown.contains("Version : 0.17.9.8")
        || !malformed.ends_with("not,a,dat,delta\r\n")
        || !stale.contains("InputMap : C:/old/basemap.scx")
    {
        return Err(
            "literal malformed, version, or stale-map source observation changed".to_string(),
        );
    }
    if bundle
        .root
        .join("cases/missing-map.e2p/Map/missing.scx")
        .exists()
        || !bundle
            .root
            .join("cases/ambiguous-map.e2p/Map/basemap.scx")
            .is_file()
        || !bundle
            .root
            .join("cases/ambiguous-map.e2p/Map/alternate/basemap.scx")
            .is_file()
    {
        return Err("literal missing or ambiguous map layout changed".to_string());
    }
    Ok(())
}

fn assert_literal_feature(
    bundle: &FixtureBundle,
    expected: &ExpectedFeature,
) -> Result<(), String> {
    let feature = bundle
        .oracle
        .features
        .iter()
        .find(|feature| feature.id == expected.id)
        .ok_or_else(|| format!("missing literal feature {}", expected.id))?;
    if feature.expectation != expected.expectation
        || feature.source.case_path != expected.case_path
        || feature.oracle.ir != expected.ir
        || feature.oracle.eap != expected.eap
        || feature.oracle.build != expected.build
        || feature.oracle.artifact != expected.artifact
    {
        return Err(format!(
            "literal three-layer record changed for {}",
            expected.id
        ));
    }
    let source = fs::read_to_string(bundle.root.join(expected.case_path))
        .map_err(|error| error.to_string())?;
    for line in expected.source_lines {
        if !source.contains(line)
            || !feature
                .source
                .required_lines
                .iter()
                .any(|actual| actual == line)
        {
            return Err(format!(
                "source observation changed for {}: {line}",
                expected.id
            ));
        }
    }
    for resource in expected.resources {
        let expected_bytes = bundle
            .package_file(resource)
            .ok_or_else(|| format!("missing package bytes for {resource}"))?;
        let physical_bytes = fs::read(bundle.e2p_root.join(resource))
            .map_err(|error| format!("missing physical package resource {resource}: {error}"))?;
        if physical_bytes != expected_bytes
            || !feature
                .source
                .referenced_resources
                .iter()
                .any(|actual| actual == resource)
        {
            return Err(format!(
                "resource observation changed for {}: {resource}",
                expected.id
            ));
        }
    }
    Ok(())
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
    assert_literal_feature_records(&first).expect("independent literal feature records");
    assert_literal_failure_records(&first).expect("independent literal failure records");
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
    let file_mode = first
        .oracle
        .files
        .iter()
        .find(|file| file.path == "resources/e2/tiles/file-mode/fixture.bmp")
        .expect("file-mode BMP inventory");
    assert_eq!(
        file_mode.source_refs,
        vec!["cases/tile-file.e2s:ProjectTileSetFileName"]
    );
    let file_mode_bytes = first
        .package_file("resources/e2/tiles/file-mode/fixture.bmp")
        .expect("file-mode BMP bytes");
    assert_eq!(&file_mode_bytes[0..2], b"BM");
    assert_eq!(&file_mode_bytes[18..22], &32u32.to_le_bytes());
    assert_eq!(&file_mode_bytes[22..26], &32u32.to_le_bytes());

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

    let tile_file_path = first.root.join("cases/tile-file.e2s");
    let tile_file_original = fs::read_to_string(&tile_file_path).expect("read tile-file source");
    let tile_file_mutated = tile_file_original.replacen(
        "resources/e2/tiles/file-mode/fixture.bmp",
        "resources/e2/tiles/file-mode/missing.bmp",
        1,
    );
    fs::write(&tile_file_path, tile_file_mutated).expect("mispoint tile-file source");
    assert!(
        assert_literal_feature_records(&first).is_err(),
        "independent source oracle must reject a mispointed file-mode input before SHA checks"
    );
    fs::write(&tile_file_path, tile_file_original).expect("restore tile-file source");
    assert_literal_feature_records(&first).expect("restored independent source oracle");
    println!("E2_FIXTURE_LITERAL_MUTATION PASS");

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
