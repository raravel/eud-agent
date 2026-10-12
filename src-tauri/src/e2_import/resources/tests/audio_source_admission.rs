use std::fs as test_fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;
use crate::config::DataDirs;
use crate::e2_import::bgm::{convert_bgm, E2BgmRequest};
use crate::e2_import::parser::parse_e2_bytes;

fn installed_dirs() -> DataDirs {
    DataDirs::from_bases(
        Path::new(&std::env::var("APPDATA").expect("APPDATA")),
        Path::new(&std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")),
    )
}

fn audio_document(path: &str) -> E2Document {
    let mut value = map_only_document(E2SourceKind::E2s, "basemap.scx");
    value.features.push(E2FeatureState {
        feature: E2Feature::SoundBgmReferences,
        activation: E2Activation::Active,
        raw_payload: None,
    });
    value.resources.push(reference(
        "SoundPlayerSET:0",
        E2Feature::SoundBgmReferences,
        ProjectResourceKind::Audio,
        path,
    ));
    value.settings.sound_interval = 1;
    value
}

fn authentic_source(audio: &Path) -> String {
    format!(
        concat!(
            "S_ProjectSET\r\n",
            "Version : 0.17.9.7\r\n",
            "InputMap : basemap.scx\r\n",
            "OutputMap : output.scx\r\n",
            "UsedSetting0 : False\r\n",
            "UsedSetting1 : False\r\n",
            "UsedSetting2 : False\r\n",
            "UsedSetting3 : False\r\n",
            "UsedSetting4 : True\r\n",
            "UsedSetting5 : False\r\n",
            "UsedSetting6 : False\r\n",
            "UsedSetting7 : False\r\n",
            "UsedSetting8 : False\r\n",
            "E_ProjectSET\r\n",
            "S_SoundPlayerSET\r\n",
            "Soundinterval : 1\r\n",
            "{}\r\n",
            "E_SoundPlayerSET\r\n",
        ),
        audio.display()
    )
}

fn write_new(path: &Path, bytes: &[u8]) {
    let mut output = test_fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .expect("create new Task 23 evidence artifact");
    output
        .write_all(bytes)
        .expect("write Task 23 evidence artifact");
}

#[test]
fn producer_audio_extensions_are_admitted_and_unknown_extension_is_rejected() {
    // Given: existing candidates for every source format accepted by Editor 0.17.9.7.
    let fixture = FixtureRoot::new("audio-extensions");
    let source = fixture.write("project.e2s", b"source");
    fixture.write("basemap.scx", b"map");

    // When / Then: each producer-supported extension reaches the owned copy plan.
    for extension in ["flac", "wav", "mp3", "ogg", "avi"] {
        let path = fixture.write(&format!("music.{extension}"), b"candidate");
        let plan = ready(&source, &audio_document(&path.to_string_lossy()), &[], None);
        assert!(plan
            .copies
            .iter()
            .any(|copy| copy.resource.id == "SoundPlayerSET:0" && copy.bytes == b"candidate"));
    }

    // Given / When: a real existing candidate uses an unsupported extension.
    let unsupported = fixture.write("music.aac", b"candidate");
    let result = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &source,
            document: &audio_document(&unsupported.to_string_lossy()),
            relinks: &[],
            expected_fingerprint: None,
        },
        || false,
    );

    // Then: the source-kind boundary remains fail-closed.
    assert!(
        matches!(
            result,
            Err(E2ResourceResolveError::Fatal(E2Issue {
                code: E2IssueCode::InvalidResource,
                ..
            }))
        ),
        "unsupported audio result: {result:?}"
    );
}

#[test]
fn missing_audio_need_exposes_producer_extensions() {
    // Given: an active BGM reference whose source is absent.
    let fixture = FixtureRoot::new("audio-relink-extensions");
    let source = fixture.write("project.e2s", b"source");
    fixture.write("basemap.scx", b"map");

    // When: resource preflight asks the caller to relink the missing audio.
    let result = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &source,
            document: &audio_document("missing.mp3"),
            relinks: &[],
            expected_fingerprint: None,
        },
        || false,
    )
    .expect("missing audio must remain a recoverable relink need");

    // Then: the picker contract exposes every source format accepted by Editor 0.17.9.7.
    let E2ResourceResolution::NeedsRelink { needs, .. } = result else {
        panic!("missing audio must request relink");
    };
    let audio = needs
        .iter()
        .find(|need| need.id == "resource:SoundPlayerSET:0")
        .expect("audio relink need");
    assert_eq!(audio.extensions, ["flac", "wav", "mp3", "ogg", "avi"]);
}

#[test]
fn authentic_unsupported_audio_source_is_rejected() {
    // Given: an authentic active SoundPlayer section points at a real drive-absolute unknown file.
    let fixture = FixtureRoot::new("authentic-unsupported-audio");
    fixture.write("basemap.scx", b"map");
    let unsupported = fixture.write("music.txt", b"not-audio");
    let source_text = authentic_source(&unsupported);
    let source = fixture.write("project.e2s", source_text.as_bytes());
    let parsed = parse_e2_bytes(source_text.as_bytes(), E2SourceKind::E2s)
        .expect("unknown SoundPlayer source remains parseable for typed rejection");

    // When: resource preflight evaluates the parsed source candidate.
    let result = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &source,
            document: &parsed.document,
            relinks: &[],
            expected_fingerprint: None,
        },
        || false,
    );

    // Then: the unsupported extension is explicit, never silently omitted from BGM resources.
    assert!(matches!(
        result,
        Err(E2ResourceResolveError::Fatal(E2Issue {
            code: E2IssueCode::InvalidResource,
            ..
        }))
    ));
}

#[test]
#[ignore = "requires installed managed FFmpeg"]
fn authentic_flac_source_reaches_managed_ogg_output() {
    // Given: an authentic 0.17.9.7 E2S document and a real FLAC derived from the frozen OGG fixture.
    let fixture = FixtureRoot::new("authentic-flac");
    fixture.write("basemap.scx", b"map");
    let flac = fixture.0.join("music.flac");
    let dirs = installed_dirs();
    let tools = crate::bootstrap::resolve_managed_ffmpeg(&dirs)
        .expect("installed managed ffmpeg must match its frozen manifest");
    let tone = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/isom/tests/fixtures/tone.ogg");
    let status = Command::new(&tools.ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(&tone)
        .args(["-map_metadata", "-1", "-vn", "-c:a", "flac", "-y"])
        .arg(&flac)
        .status()
        .expect("managed ffmpeg fixture conversion must start");
    assert!(
        status.success(),
        "managed ffmpeg must create the FLAC fixture"
    );
    let source_text = authentic_source(&flac);
    let source = fixture.write("project.e2s", source_text.as_bytes());
    let parsed = parse_e2_bytes(source_text.as_bytes(), E2SourceKind::E2s)
        .expect("authentic FLAC SoundPlayer source must parse");
    let expected_source = flac.to_string_lossy().into_owned();
    assert_eq!(
        parsed
            .document
            .resources
            .iter()
            .filter(|resource| resource.feature == E2Feature::SoundBgmReferences)
            .map(|resource| resource.source_path.as_str())
            .collect::<Vec<_>>(),
        [expected_source.as_str()]
    );
    let plan = ready(&source, &parsed.document, &[], None);
    let owned = plan
        .copies
        .iter()
        .find(|copy| copy.resource.kind == ProjectResourceKind::Audio)
        .expect("resolved plan must own the FLAC source bytes");
    assert_eq!(
        owned.bytes,
        test_fs::read(&flac).expect("reread FLAC fixture")
    );

    // When: the existing managed BGM converter consumes the byte-owned plan.
    let output = convert_bgm(
        E2BgmRequest {
            document: &parsed.document,
            resources: &plan,
            data_dirs: &dirs,
            temp_root: &fixture.0,
        },
        || false,
    )
    .expect("managed FLAC conversion must succeed")
    .expect("active BGM must emit output");

    // Then: the final owned resources are probed OGG segments, never the original container.
    assert!(!output.generated_copies.is_empty());
    assert!(output
        .generated_copies
        .iter()
        .all(|copy| copy.resource.path.ends_with(".ogg") && copy.bytes.starts_with(b"OggS")));
    if let Some(root) = std::env::var_os("EUD_AGENT_TASK23_EVIDENCE") {
        let root = PathBuf::from(root);
        test_fs::create_dir_all(&root).expect("create Task 23 evidence directory");
        write_new(&root.join("source.flac"), &owned.bytes);
        write_new(
            &root.join("generated.ogg"),
            &output.generated_copies[0].bytes,
        );
    }
}
