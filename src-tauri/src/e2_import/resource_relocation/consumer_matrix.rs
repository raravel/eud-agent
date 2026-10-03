use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::e2_import::{E2Feature, ProjectResourceConsumer, ProjectResourceKind};

use super::relocated_build::RelocatedBuild;

pub(super) fn assert_consumer_matrix(build: &RelocatedBuild, file_tile: bool) {
    for feature in [
        E2Feature::Bin,
        E2Feature::CustomTiles,
        E2Feature::ButtonsBgm,
        E2Feature::Plugin,
        E2Feature::FileManager,
        E2Feature::SoundBgmReferences,
    ] {
        assert!(
            build.features.contains(&feature),
            "missing feature {feature:?}"
        );
    }
    let import = build.project.manifest().e2_import.as_ref().unwrap();
    assert_eq!(import.resources.len(), build.original_resource_count);
    assert!(build.project.read_source("src/e2/bin_compat.eps").is_ok());
    let tile = build.project.read_source("src/e2/tile_compat.eps").unwrap();
    if file_tile {
        assert!(tile.contains(
            "SetDeaths(cv5offset + 5, SetTo, cv5_20b2 * 65536 + cv5_20b3 * 16777216 + 0, 0);"
        ));
        assert_eq!(
            tile.matches("SetDeaths(CurrentPlayer, SetTo, 16777472, 0);")
                .count(),
            256
        );
    } else {
        assert!(tile.contains("73") && tile.contains("74"));
    }
    let bgm = build.project.read_source("src/e2/bgm.eps").unwrap();
    assert!(bgm.contains("M1_0.ogg"));
    let tct = build.project.read_source("src/e2/tct.py").unwrap();
    assert!(tct.contains("def f_"));

    let whole = import
        .resources
        .iter()
        .find(|resource| resource.id == "SoundPlayerSET:1")
        .expect("whole authentic audio is retained");
    let segments = import
        .resources
        .iter()
        .filter(|resource| resource.id.starts_with("SoundPlayerSET:1:segment:"))
        .collect::<Vec<_>>();
    assert!(
        !segments.is_empty(),
        "actual converter segments are persisted"
    );
    assert!(segments
        .iter()
        .all(|resource| resource.consumer == ProjectResourceConsumer::Mpq));
    assert!(segments
        .iter()
        .all(|resource| resource.sha256 != whole.sha256));
    for segment in segments {
        let alias = segment.build_destination.replace('/', "\\");
        let actual = isom::map_asset(&build.output, &alias, segment.size as usize).unwrap();
        assert_eq!(actual.len() as u64, segment.size);
        assert_eq!(format!("{:x}", Sha256::digest(&actual)), segment.sha256);
        assert_eq!(
            actual,
            fs::read(build.project.root().join(&segment.path)).unwrap()
        );
    }

    let resources = &import.resources;
    for (kind, consumer) in [
        (ProjectResourceKind::Bin, ProjectResourceConsumer::BinCompat),
        (ProjectResourceKind::Plugin, ProjectResourceConsumer::Plugin),
        (
            ProjectResourceKind::File,
            ProjectResourceConsumer::FileManager,
        ),
    ] {
        assert!(resources.iter().any(|resource| {
            resource.kind == kind
                && resource.consumer == consumer
                && sha256_path(&build.project.root().join(&resource.path)) == resource.sha256
        }));
    }
    if file_tile {
        assert!(resources.iter().any(|resource| {
            resource.kind == ProjectResourceKind::Tile
                && resource.consumer == ProjectResourceConsumer::TileCompat
                && sha256_path(&build.project.root().join(&resource.path)) == resource.sha256
        }));
    } else {
        assert!(!resources
            .iter()
            .any(|resource| resource.kind == ProjectResourceKind::Tile));
    }
    for id in [
        "FileManagerSET:stat_txt",
        "FileManagerSET:AIscript",
        "FileManagerSET:iscript",
    ] {
        assert!(resources.iter().any(|resource| {
            resource.id == id
                && resource.kind == ProjectResourceKind::File
                && resource.consumer == ProjectResourceConsumer::FileManager
                && resource.path != resource.build_destination
        }));
    }

    let sources = build.project.list_source_files().unwrap();
    let eps = sources
        .iter()
        .find(|path| path.contains("plugins") && path.ends_with(".eps"))
        .unwrap();
    let python = sources
        .iter()
        .find(|path| path.contains("plugins") && path.ends_with(".py"))
        .unwrap();
    assert!(build.project.read_source(eps).unwrap().contains("plugin_b"));
    assert!(build
        .project
        .read_source(python)
        .unwrap()
        .contains("onPluginStart"));
    let eds = fs::read_to_string(&build.result.artifacts.eds_path).unwrap();
    assert!(
        eds.contains("PythonPath"),
        "helper-backed Python needs PythonPath"
    );
    let normalized_eds = eds.replace('\\', "/");
    assert!(!normalized_eds
        .lines()
        .any(|line| line.trim_end().ends_with("src/e2/tct.py")));
    let eps_at = normalized_eds.find(eps.as_str()).unwrap();
    let python_at = normalized_eds.find(python.as_str()).unwrap();
    assert!(
        eps_at < python_at,
        "case-sensitive producer order survives build"
    );
    for destination in ["0x6D5A30", "0x68C104", "0x6D1200"] {
        assert!(eds.contains(destination));
    }
    assert!(build.log.contains("Injecting plugins"));
}

fn sha256_path(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}
