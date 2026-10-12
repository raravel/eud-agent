use super::{bin_baseline, test_support::*, E2ArtifactConsumer};
use crate::e2_import::{E2RawValue, ProjectResourceKind};

#[test]
fn complete_consumer_matrix() {
    let pixels = (0..1_024)
        .map(|value| (value % 256).to_string())
        .collect::<Vec<_>>()
        .join(",");
    let bin = section(
        "BinEditSET",
        &[(
            "Bin0",
            E2RawValue::String("pointer=0x006D5EC8,width=4,value=0x00000002,PlayerRace=0".into()),
        )],
    );
    let tiles = section(
        "TileSET",
        &[
            ("ProjectTileUseFile", E2RawValue::Bool(false)),
            ("ProjectTileSetDataCount", E2RawValue::Integer(2)),
            ("ProjectTileSetDataTileSetNum0", E2RawValue::Integer(73)),
            ("ProjectTileSetDataisMaker0", E2RawValue::Bool(true)),
            (
                "ProjectTileSetDataTileSetData0",
                E2RawValue::String(pixels.clone()),
            ),
            ("ProjectTileSetDataTileSetNum1", E2RawValue::Integer(74)),
            ("ProjectTileSetDataisMaker1", E2RawValue::Bool(false)),
            ("ProjectTileSetDataTileSetData1", E2RawValue::String(pixels)),
        ],
    );
    let plugins = section("PluginSET", &[("safeEnabled", E2RawValue::Bool(true))]);
    let resources = vec![
        copy(
            "BinEditSET:DataFile",
            ProjectResourceKind::Bin,
            "r/a.bin",
            &[2, 0, 0, 0],
        ),
        copy(
            "PluginSET:safe",
            ProjectResourceKind::Plugin,
            "r/safe.eps",
            b"function safe() {}\n",
        ),
        copy(
            "PluginSET:python",
            ProjectResourceKind::Plugin,
            "r/p.py",
            b"def plugin():\n    pass\n",
        ),
        copy(
            "FileManagerSET:iscript",
            ProjectResourceKind::File,
            "r/iscript.bin",
            b"ISCR",
        ),
        copy(
            "FileManagerSET:stat_txt",
            ProjectResourceKind::File,
            "r/stat.tbl",
            b"STAT",
        ),
        copy(
            "FileManagerSET:AIscript",
            ProjectResourceKind::File,
            "r/ai.bin",
            b"AISC",
        ),
    ];
    let plan = compile(
        &document(vec![bin, tiles, plugins, section("FileManagerSET", &[])]),
        &resources,
        None,
    )
    .unwrap();
    record(&plan, "memory");
    let tile = text(&plan, "src/e2/tile_compat.eps");
    assert!(
        tile.contains("SetDeaths(cv5offset + 61, SetTo, cv5_246b0 + cv5_246b1 * 256 + 4784128, 0)")
    );
    assert!(tile.contains(
        "SetDeaths(vx4offset + 584, SetTo, vx4_2336b2 * 65536 + vx4_2336b3 * 16777216 + 2336, 0)"
    ));
    assert!(
        tile.contains("var cv5tile1 = cv5_1b0 + cv5_1b1 * 256") && tile.contains("vx4value1_0 & 1")
    );
    assert!(tile.contains("50462976") && tile.contains("67438087"));
    assert_eq!(
        &destinations(&plan)[2..5],
        [
            "dataDumper:0x6D5A30",
            "dataDumper:0x68C104",
            "dataDumper:0x6D1200"
        ]
    );
    let plugin = plan
        .artifacts
        .iter()
        .find(|a| a.consumer == E2ArtifactConsumer::EdsPlugin && a.source_section == "PluginSET")
        .unwrap();
    assert_eq!(plugin.build_destination, "eud-agent.eds:before-main");
    assert!(plan.artifacts.iter().any(|artifact| {
        artifact.consumer == E2ArtifactConsumer::PythonEntrypoint
            && artifact.bytes == b"def plugin():\n    pass\n"
    }));

    let mut image = bmp();
    let file_section = section("TileSET", &[("ProjectTileUseFile", E2RawValue::Bool(true))]);
    let before = compile(
        &document(vec![file_section.clone()]),
        &[copy(
            "TileSET:ProjectTileSetFileName",
            ProjectResourceKind::Tile,
            "r/t.bmp",
            &image,
        )],
        None,
    )
    .unwrap();
    record(&before, "bmp");
    let file = text(&before, "src/e2/tile_compat.eps");
    assert!(file.find("572662306").unwrap() < file.find("286331153").unwrap());
    image[0x436] = 0x12;
    let after = compile(
        &document(vec![file_section]),
        &[copy(
            "TileSET:ProjectTileSetFileName",
            ProjectResourceKind::Tile,
            "r/t.bmp",
            &image,
        )],
        None,
    )
    .unwrap();
    assert_ne!(before.artifacts[0].sha256, after.artifacts[0].sha256);
    let scene = section(
        "BinEditSET",
        &[
            (".(0)(0)isDefault", E2RawValue::Bool(true)),
            (".(0)(0)posx", E2RawValue::Integer(221)),
            (".(0)(0)posy", E2RawValue::Integer(1)),
            (".(0)(0)sizew", E2RawValue::Integer(205)),
            (".(0)(0)sizeh", E2RawValue::Integer(24)),
            (".(0)(0)(0)isDefault", E2RawValue::Bool(true)),
            (".(0)(0)(0)posx", E2RawValue::Integer(353)),
            (".(0)(0)(0)posy", E2RawValue::Integer(2)),
            (".(0)(0)(0)sizew", E2RawValue::Integer(70)),
            (".(0)(0)(0)sizeh", E2RawValue::Integer(20)),
        ],
    );
    let scene_doc = document(vec![scene]);
    let scene_plan = compile(&scene_doc, &[], Some(0)).unwrap();
    record(&scene_plan, "bin");
    let scene_text = text(&scene_plan, "src/e2/bin_compat.eps");
    assert!(scene_text.contains("f_epdread_epd(264116) + 1, 65757"));
    assert!(scene_text.contains("f_epdread_epd(264116) + 2, 1573289"));
    assert!(scene_text.contains("f_epdread_epd(264116) + 22, Add, 65536"));
    assert!(scene_text.contains("f_epdread_epd(264116) + 23, SetTo, 27656194"));
    let baseline = bin_baseline::v0_17_9_7();
    assert_eq!((baseline[0].rect.x2, baseline[0].objects.len()), (419, 8));
    assert_eq!(
        baseline.iter().map(|row| row.objects.len()).sum::<usize>(),
        135
    );
    assert!(compile(&scene_doc, &[], Some(255))
        .unwrap()
        .artifacts
        .is_empty());
}
#[test]
fn invalid_and_excluded_matrix() {
    let invalid_bin = section(
        "BinEditSET",
        &[(
            "Bin0",
            E2RawValue::String("pointer=0x00000004,width=4,value=1,PlayerRace=0".into()),
        )],
    );
    assert_eq!(
        error_code(&document(vec![invalid_bin]), &[]),
        crate::e2_import::E2IssueCode::InvalidResource
    );
    let unsafe_plugin = copy(
        "PluginSET:unsafe",
        ProjectResourceKind::Plugin,
        "r/unsafe.dll",
        b"MZ",
    );
    assert_eq!(
        error_code(&document(vec![]), &[unsafe_plugin]),
        crate::e2_import::E2IssueCode::UnsafePlugin
    );
    let disabled_plugin = section("PluginSET", &[("unsafeEnabled", E2RawValue::Bool(false))]);
    let dll = copy(
        "PluginSET:unsafe",
        ProjectResourceKind::Plugin,
        "r/unsafe.dll",
        b"MZ",
    );
    assert!(compile(&document(vec![disabled_plugin]), &[dll], None)
        .unwrap()
        .artifacts
        .is_empty());

    let grp = copy(
        "PluginSET:dataDumper_grpwire",
        ProjectResourceKind::File,
        "r/grpwire.grp",
        b"GRP",
    );
    let inactive = section(
        "PluginSET",
        &[("dataDumper_grpwire_f", E2RawValue::Integer(0))],
    );
    assert!(
        compile(&document(vec![inactive]), std::slice::from_ref(&grp), None)
            .unwrap()
            .artifacts
            .is_empty()
    );
    let active = section(
        "PluginSET",
        &[("dataDumper_grpwire_f", E2RawValue::Integer(1))],
    );
    assert_eq!(
        error_code(&document(vec![active]), &[grp]),
        crate::e2_import::E2IssueCode::ActiveGrpUnsupported
    );
    let empty_grp = section("GRPEditorSET", &[("GRPUse0", E2RawValue::Bool(false))]);
    assert!(compile(&document(vec![empty_grp]), &[], None)
        .unwrap()
        .artifacts
        .is_empty());
    let effective_grp = section("GRPEditorSET", &[("GRPUse0", E2RawValue::Bool(true))]);
    assert_eq!(
        error_code(&document(vec![effective_grp]), &[]),
        crate::e2_import::E2IssueCode::ActiveGrpUnsupported
    );
    let unknown = copy(
        "FileManagerSET:unknown",
        ProjectResourceKind::File,
        "r/u.bin",
        b"U",
    );
    assert_eq!(
        error_code(&document(vec![section("FileManagerSET", &[])]), &[unknown]),
        crate::e2_import::E2IssueCode::InvalidResource
    );
}
