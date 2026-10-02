use std::collections::BTreeMap;

use crate::e2_import::definitions::E2Definition;
use crate::e2_import::{
    E2Activation, E2CompiledSources, E2Document, E2Feature, E2FeatureState, E2ProjectSettings,
    E2RawSection, E2RawValue, E2SourceKind, E2TriggerNode, E2TriggerNodeKind, E2Version,
};

pub(super) const EUD_FORMS: &[(&str, &str)] = &[
    ("EUDTurbo", "SetMemory(0x6509A0"),
    ("SetCurrentPlayer", "setcurpl("),
    ("SetKills", "EPD(0x5878A4)"),
    ("SetSupply", "EPD(0x582144)"),
    ("SetLight", "EPD(0x657A9C)"),
    ("SetDatfile", "wwrite("),
    (
        "CreatePlayerVariable",
        "const fixtureCreatePlayerVariable = [0, 0, 0",
    ),
    ("Rand", "dwrand() %"),
    ("SetVariableKills", "dwread_epd(EPD(0x5878A4)"),
    ("SetButton", "EPD(0x5187EC)"),
    ("ButtonRefresh", "cunitepdread_epd"),
    ("ChangeStarText", "tct.setTbl("),
    ("SetNextUnitPtr", "EPD(0x628438)"),
    ("SetCUnitData", "dwwrite("),
    ("Memory", "Memory("),
    ("LocalCurrentPlayer", "Memory(0x57F1B0"),
    ("KeyDetect", "bread(0x596A18"),
    ("CUnitDataEPD", "dwread_epd("),
];

pub(super) const TCT_HELPERS: &[&str] = &[
    "addText = f_addText",
    "chatAnnouncement = f_chatAnnouncement",
    "displayText = f_displayText",
    "makeText = f_makeText",
    "print = f_print",
    "printError = f_printError",
    "setTbl = f_setTbl",
];

const BGM_SIGNATURE_STUB: &str = r#"# Test-only Task 9 signature context; no runtime behavior claim.
from eudplib import *

def Stop(player): return []
def Resume(player): return []
def Play(player, bgm, flag): return []
def CurrentPlayMusic(player, bgm): return Always()
def isplaying(player, bgm): return Always()
f_Stop, f_Resume, f_Play = Stop, Resume, Play
f_CurrentPlayMusic, f_isplaying = CurrentPlayMusic, isplaying
"#;

pub(super) fn document(triggers: Vec<E2TriggerNode>) -> E2Document {
    E2Document {
        version: E2Version::V0_17_9_7,
        source_kind: E2SourceKind::E2s,
        settings: E2ProjectSettings {
            name: "compiler-oracle".to_string(),
            input_map: "maps/source.scx".to_string(),
            output_map: "build/output.scx".to_string(),
            sound_interval: 1,
            raw_fields: BTreeMap::new(),
        },
        features: vec![E2FeatureState {
            feature: E2Feature::StructGui,
            activation: E2Activation::Active,
            raw_payload: None,
        }],
        triggers,
        resources: Vec::new(),
        raw_sections: Vec::new(),
    }
}

pub(super) fn malformed_button_document(activation: E2Activation) -> E2Document {
    let mut malformed = document(Vec::new());
    malformed.raw_sections.push(E2RawSection {
        name: "BtnSET".to_string(),
        activation,
        fields: BTreeMap::from([
            ("BtnUse0".to_string(), E2RawValue::Bool(true)),
            (
                "BtnData0".to_string(),
                E2RawValue::String("0,1,,3,4,5,6,7,".to_string()),
            ),
        ]),
        payload: E2RawValue::Null,
    });
    malformed
}

pub(super) fn malformed_tbl_document(activation: E2Activation) -> E2Document {
    let mut malformed = document(Vec::new());
    malformed.raw_sections.push(E2RawSection {
        name: "FileManagerSET".to_string(),
        activation,
        fields: BTreeMap::from([(
            "stattextdicCount".to_string(),
            E2RawValue::String("1".to_string()),
        )]),
        payload: E2RawValue::Null,
    });
    malformed
}

pub(super) fn with_tbl_override(mut document: E2Document) -> E2Document {
    document.raw_sections.push(E2RawSection {
        name: "FileManagerSET".to_string(),
        activation: E2Activation::Active,
        fields: BTreeMap::from([
            (
                "stattextdicCount".to_string(),
                E2RawValue::String("1".to_string()),
            ),
            (
                "stattextdickey0".to_string(),
                E2RawValue::String("7".to_string()),
            ),
            (
                "stattextdicvalue0".to_string(),
                E2RawValue::String("A#B".to_string()),
            ),
        ]),
        payload: E2RawValue::Null,
    });
    document
}

pub(super) fn lifecycle_document() -> E2Document {
    let mut loop_13 = node(
        E2TriggerNodeKind::Group,
        "loop-13",
        "type-10",
        vec![E2RawValue::Integer(13)],
    );
    loop_13.children.push(action("loop-action", "Victory"));
    let mut player_loop = node(
        E2TriggerNodeKind::Group,
        "loop-player",
        "type-10",
        vec![E2RawValue::Integer(4)],
    );
    player_loop.children.push(action("player-action", "Defeat"));
    let mut start = node(
        E2TriggerNodeKind::Lifecycle,
        "start",
        "onPluginStart",
        Vec::new(),
    );
    start.children.push(action("start-action", "PauseGame"));
    let mut after = node(
        E2TriggerNodeKind::Lifecycle,
        "after",
        "afterTriggerExec",
        Vec::new(),
    );
    after.children.push(action("after-action", "UnpauseGame"));
    document(vec![start, loop_13, player_loop, after])
}

fn action(id: &str, name: &str) -> E2TriggerNode {
    node(E2TriggerNodeKind::Action, id, name, vec![E2RawValue::Null])
}

pub(super) fn node(
    kind: E2TriggerNodeKind,
    id: &str,
    name: &str,
    values: Vec<E2RawValue>,
) -> E2TriggerNode {
    E2TriggerNode {
        id: id.to_string(),
        enabled: true,
        folded: false,
        not_condition: false,
        kind,
        name: name.to_string(),
        values,
        children: Vec::new(),
    }
}

pub(super) fn values(arity: usize) -> Vec<E2RawValue> {
    (0..arity)
        .map(|index| E2RawValue::Integer(i64::try_from(index + 1).expect("small test arity")))
        .collect()
}

pub(super) fn catalog_values(definition: &E2Definition) -> Vec<E2RawValue> {
    if definition.identifier == "SetButton" {
        return vec![
            E2RawValue::String("1:0".to_string()),
            E2RawValue::String("000".repeat(20)),
        ];
    }
    definition
        .value_kinds
        .iter()
        .enumerate()
        .map(|(index, kind)| match *kind {
            "None" => E2RawValue::Null,
            "WAVName" => E2RawValue::String("sound\\Terran\\fixture.wav".to_string()),
            "Text" | "Label" | "ScenarioName" | "CText" | "DBText" => {
                E2RawValue::String("fixture-text".to_string())
            }
            "Properties" => E2RawValue::String("UnitProperty()".to_string()),
            "RawString" => E2RawValue::String("SetMemory(0x6509A0, SetTo, 0)".to_string()),
            "Name" => E2RawValue::String(format!(
                "fixture{}",
                definition
                    .identifier
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .collect::<String>()
            )),
            "Variable" => E2RawValue::String("fixtureCreateVariable".to_string()),
            "Boolean" => E2RawValue::Bool(true),
            "VariableModifier" | "VariableComparison" | "Modifier" | "TimeModifier"
            | "Comparison" | "SState" | "State" | "CState" | "AlwaysDisplay" => {
                E2RawValue::Integer(0)
            }
            _ => E2RawValue::Integer(i64::try_from(index + 1).expect("small test arity")),
        })
        .collect()
}

pub(super) fn catalog_statement<'a>(main: &'a str, id: &str) -> &'a str {
    let marker = format!("// E2 id={id}");
    main.split_once(&marker)
        .expect("catalog row marker")
        .1
        .lines()
        .find(|line| !line.trim().is_empty())
        .expect("catalog row successor")
        .trim()
}

pub(super) fn persist_evidence(compiled: &E2CompiledSources, sample: &str) {
    let Ok(root) = std::env::var("E2_COMPILER_EVIDENCE_DIR") else {
        return;
    };
    for (relative, contents) in &compiled.files {
        let path = std::path::Path::new(&root).join(sample).join(relative);
        std::fs::create_dir_all(path.parent().expect("generated source parent"))
            .expect("create evidence source directory");
        std::fs::write(path, contents).expect("write generated compiler evidence");
    }
    if sample == "full-catalog" {
        let path = std::path::Path::new(&root)
            .join(sample)
            .join("src/e2/bgm.py");
        std::fs::write(path, BGM_SIGNATURE_STUB).expect("write test-only BGM signature context");
    }
}
