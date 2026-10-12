use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::compile_trigger_tree;
use crate::e2_import::parser::parse_e2_bytes;
use crate::e2_import::E2SourceKind;
use crate::native_build::{run_native_build, EuddraftLaunch};
use crate::native_project::{
    NativeProject, ProjectManifest, ProjectSettings, PROJECT_SCHEMA_VERSION,
};

const INPUT_MAP: &str = "E:/proj/eud/test.scx";
const EUDDRAFT: &str = "E:/proj/eud/euddraft0.10.2.5/euddraft.exe";

fn source() -> String {
    concat!(
        "S_ProjectSET\nVersion : 0.17.9.7\nInputMap : Map/input.scx\n",
        "OutputMap : Map/output.scx\nUsedSetting6 : True\nE_ProjectSET\n",
        "S_TriggerEditorSET\n",
        "&functions&\nType:15,False,False,False\nElementsCount:1\n",
        "Type:16,False,False,False\npulse\u{0b90}True\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:0\nEND\n",
        "Type:18,False,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:RawCode\nvar pulseTick = 1\n",
        "ElementsCount:0\nEND\nEND\nEND\nEND\n",
        "&RawTriggers&\nType:25,False,False,False\nElementsCount:1\n",
        "Type:24,False,False,False\n1\nElementsCount:2\n",
        "Type:26,False,False,False\nAnd\nElementsCount:1\n",
        "Type:7,False,False,False\ncon:Always\n0\nElementsCount:0\nEND\nEND\n",
        "Type:27,False,False,False\nElementsCount:4\n",
        "Type:8,False,False,False\nact:RawCode\nvar rawBefore = 1\nElementsCount:0\nEND\n",
        "Type:20,False,False,False\n2\nElementsCount:0\nEND\n",
        "Type:8,False,False,False\nact:RawCode\nvar rawAfter = 1\nElementsCount:0\nEND\n",
        "Type:8,False,False,False\nact:PreserveTrigger\n0\nElementsCount:0\nEND\n",
        "END\nEND\nEND\nE_TriggerEditorSET\n",
    )
    .to_string()
}

fn assert_ordered(text: &str, fragments: &[&str]) {
    let positions = fragments
        .iter()
        .map(|fragment| {
            text.find(fragment)
                .unwrap_or_else(|| panic!("missing {fragment:?}"))
        })
        .collect::<Vec<_>>();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
}

fn assert_semantics(main: &str) {
    let executor_start = main.find("function ClassicTriggerExec0()").unwrap();
    let starter_start = main.find("function ClassicTriggerStarter0()").unwrap();
    let executor = &main[executor_start..starter_start];
    let state_one = executor.find(" == 1").unwrap();
    let state_three = executor.find(" == 3").unwrap();
    let reset = executor
        .find("ClassicTriggerExecTimer0[getcurpl()] = 0;")
        .unwrap();
    assert!(executor[state_one..state_three].contains("var rawBefore = 1;"));
    assert!(!executor[state_one..state_three].contains("var rawAfter = 1;"));
    assert!(executor[state_three..reset].contains("var rawAfter = 1;"));
    assert!(!executor[state_three..reset].contains("var rawBefore = 1;"));
    assert!(executor[state_three..reset].contains("PreserveTrigger();"));
    assert!(!executor.contains("ClassicTriggerExecTimer0[getcurpl()] = -1;"));
    assert_ordered(
        &executor[reset..],
        &[
            "ClassicTriggerExecTimer0[getcurpl()] = 0;",
            "ClassicTriggerExecTimer0[getcurpl()] > 0",
            "ClassicTriggerExecTimer0[getcurpl()] += 1;",
        ],
    );
    let hook = &main[main.find("function beforeTriggerExec()").unwrap()..];
    assert_ordered(
        hook,
        &[
            "EUDPlayerLoop()();",
            "WaitableTriggerExec();",
            "ClassicTriggerExec();",
            "EUDEndPlayerLoop();",
            "ClassicTriggerStarter();",
        ],
    );
    let p1 = &main[main.find("playerexist(0)").unwrap()..main.find("playerexist(1)").unwrap()];
    assert_ordered(
        p1,
        &[
            "playerexist(0)",
            "setcurpl(0);",
            "ClassicTriggerStarter0();",
        ],
    );
    assert_eq!(p1.matches("ClassicTriggerStarter0();").count(), 1);
}

#[test]
#[ignore = "requires frozen euddraft and E:/proj/eud/test.scx"]
fn parser_generated_raw_wait_builds_with_frozen_euddraft() {
    let parsed = parse_e2_bytes(source().as_bytes(), E2SourceKind::E2s)
        .expect("authentic joint RAW and WAIT wire parses");
    let compiled = compile_trigger_tree(&parsed.document).expect("joint RAW and WAIT compiles");
    let main = &compiled.files["src/main.eps"];
    assert_semantics(main);
    let evidence = PathBuf::from(std::env::var("EUD_AGENT_RAW_WAIT_BUILD_EVIDENCE").unwrap());
    assert!(evidence.is_absolute() && evidence.parent().is_some());
    fs::create_dir(&evidence).unwrap();

    let original_path = Path::new(INPUT_MAP);
    let original_before = fs::read(original_path).unwrap();
    let root =
        std::env::temp_dir().join(format!("eud-agent-raw-wait-build-{}", uuid::Uuid::new_v4()));
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("maps")).unwrap();
    fs::write(root.join("maps/source.scx"), &original_before).unwrap();
    let copied_before = fs::read(root.join("maps/source.scx")).unwrap();
    assert_eq!(copied_before, original_before);

    let project = NativeProject::create(
        &root,
        ProjectManifest {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: "RAW WAIT Frozen Build".to_string(),
            source_map: "maps/source.scx".to_string(),
            output_map: "build/output.scx".to_string(),
            main_file: "src/main.eps".to_string(),
            settings: ProjectSettings::default(),
            plugins: Vec::new(),
            python_entrypoints: Vec::new(),
            python_dependencies: Vec::new(),
            python_lock: None,
            editor_compatibility: None,
            e2_import: None,
        },
    )
    .unwrap();
    project.create_source_dir("src/e2").unwrap();
    for (path, text) in &compiled.files {
        project.write_source(path, text).unwrap();
    }
    assert_eq!(project.read_source("src/main.eps").unwrap(), main.as_str());
    let revision_before = project.revision().unwrap();
    let output_path = root.join("build/output.scx");
    assert!(!output_path.exists());

    let launcher = EuddraftLaunch::resolve(Path::new(EUDDRAFT)).unwrap();
    let result = run_native_build(
        &project,
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/eud-editor-compat"),
        &launcher,
    )
    .unwrap();
    let project_main = project.read_source("src/main.eps").unwrap();
    let copied_after = fs::read(root.join("maps/source.scx")).unwrap();
    let original_after = fs::read(original_path).unwrap();
    let revision_after = project.revision().unwrap();
    let eds = fs::read(&result.artifacts.eds_path).unwrap();
    let log = fs::read(&result.log_path).unwrap();
    let output = fs::read(&result.artifacts.output_map).ok();

    write_new(&evidence.join("generated-main.eps"), main.as_bytes());
    write_new(&evidence.join("project-main.eps"), project_main.as_bytes());
    write_new(&evidence.join("generated.eds"), &eds);
    write_new(&evidence.join("native-build.log"), &log);
    if let Some(bytes) = &output {
        write_new(&evidence.join("fresh-output.scx"), bytes);
        write_new(
            &evidence.join("fresh-output.sha256"),
            format!("{}\n", sha(bytes)).as_bytes(),
        );
    }
    write_new(
        &evidence.join("input-map.sha256"),
        format!(
            "original_before={}\ncopied_before={}\ncopied_after={}\noriginal_after={}\n",
            sha(&original_before),
            sha(&copied_before),
            sha(&copied_after),
            sha(&original_after),
        )
        .as_bytes(),
    );
    write_new(
        &evidence.join("receipt.txt"),
        format!(
            "selector=parser_generated_raw_wait_builds_with_frozen_euddraft\nlauncher={}\nrevision_before={}\nrevision_after={}\neps_bytes={}\neds_bytes={}\nlog_bytes={}\noutput_bytes={}\ngame_equivalence=not_claimed\n",
            launcher.frozen_executable().unwrap().display(),
            revision_before,
            revision_after,
            main.len(),
            eds.len(),
            log.len(),
            output.as_ref().map_or(0, Vec::len),
        )
        .as_bytes(),
    );

    assert!(result.ok, "native errors: {:?}", result.errors);
    assert!(result.errors.is_empty());
    assert!(
        result.warnings.is_empty(),
        "native warnings: {:?}",
        result.warnings
    );
    let log = String::from_utf8(log).unwrap();
    assert!(log.starts_with("# euddraft build log\n# exit status: 0x00000000\n"));
    for marker in [
        "[epScript]",
        "[[ freeze activated ]]",
        "Injecting plugins",
        "[Stage 4/3]",
    ] {
        assert!(log.contains(marker), "missing native marker {marker}");
    }
    assert!(!log.to_ascii_lowercase().contains("error"));
    assert!(!log.contains("ResourceWarning: unclosed file"));
    let eds = String::from_utf8(eds).unwrap();
    assert_eq!(eds.matches("[../../src/main.eps]").count(), 1);
    assert!(!eds.contains("[../../src/e2/tct.py]"));
    let output = output.expect("fresh output SCX");
    assert!(!output.is_empty());
    let retained_output = fs::read(evidence.join("fresh-output.scx")).unwrap();
    let retained_sha = fs::read_to_string(evidence.join("fresh-output.sha256")).unwrap();
    assert_eq!(retained_output, output);
    assert_eq!(retained_sha.trim(), sha(&output));
    assert_eq!(copied_after, original_before);
    assert_eq!(original_after, original_before);
    assert_eq!(project_main, main.as_str());
    assert_eq!(revision_after, revision_before);
}

fn write_new(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}
