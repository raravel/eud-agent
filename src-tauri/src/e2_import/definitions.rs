
#[cfg(test)]
mod tests {
    use super::{
        action_definition, action_definitions, catalog_inventory_digest, catalog_inventory_digest_for,
        condition_definition, condition_definitions, E2SemanticTemplate, ACTIONS, CONDITIONS,
    };

    #[test]
    fn exact_v0_17_9_7_inventory_is_complete_and_lookup_is_fail_closed() {
        assert_eq!(action_definitions().len(), 110);
        assert_eq!(condition_definitions().len(), 44);
        assert!(action_definition("ChatWrite").is_some());
        assert!(condition_definition("CurrentBGM").is_some());
        assert!(action_definition("chatwrite").is_none());
        assert!(condition_definition("UnknownCondition").is_none());
        assert_eq!(
            action_definition("ChatWrite").map(|definition| definition.template),
            Some(E2SemanticTemplate::UnsupportedChatWrite)
        );
        for definition in action_definitions().iter().chain(condition_definitions()) {
            assert_eq!(definition.arity(), definition.value_kinds.len());
        }
        assert_eq!(super::structural_node_kind(10), Some(super::E2StructuralNodeKind::For));
        assert_eq!(super::structural_node_kind(29), Some(super::E2StructuralNodeKind::SwitchCase));
        assert_eq!(super::structural_node_kind(30), None);
    }

    #[test]
    fn inventory_digest_is_stable() {
        assert_eq!(catalog_inventory_digest(), 16_628_810_622_852_550_280);
    }

    #[test]
    fn inventory_digest_rejects_name_arity_value_kind_and_row_mutations() {
        let expected = catalog_inventory_digest();
        let mut wrong_name = ACTIONS.to_vec();
        wrong_name[0].identifier = "V1ctory";
        assert_ne!(catalog_inventory_digest_for(&wrong_name, CONDITIONS), expected);

        let mut wrong_arity_and_value_kind = ACTIONS.to_vec();
        wrong_arity_and_value_kind[6].value_kinds = &["Unit", "Where", "WAVName"];
        assert_ne!(
            catalog_inventory_digest_for(&wrong_arity_and_value_kind, CONDITIONS),
            expected
        );

        let mut missing_row = ACTIONS.to_vec();
        missing_row.pop();
        assert_ne!(catalog_inventory_digest_for(&missing_row, CONDITIONS), expected);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum E2SemanticTemplate {
    Builtin,
    EudExtension,
    StructuralMarker,
    RawCode,
    Bgm,
    CustomText,
    UnsupportedChatWrite,
    Scdb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum E2StructuralNodeKind {
    Main,
    If,
    IfElse,
    ConditionClause,
    ActionClause,
    Then,
    Else,
    Condition,
    Action,
    While,
    For,
    SwitchStatement,
    Switch,
    WhileCondition,
    WhileBody,
    ForBody,
    Functions,
    FunctionDefinition,
    Argument,
    Code,
    Function,
    Wait,
    Folder,
    FolderAction,
    RawString,
    RawTrigger,
    RawTriggers,
    TriggerCondition,
    TriggerAction,
    SwitchCase,
}

pub(crate) const fn structural_node_kind(code: u8) -> Option<E2StructuralNodeKind> {
    match code {
        0 => Some(E2StructuralNodeKind::Main),
        1 => Some(E2StructuralNodeKind::If),
        2 => Some(E2StructuralNodeKind::IfElse),
        3 => Some(E2StructuralNodeKind::ConditionClause),
        4 => Some(E2StructuralNodeKind::ActionClause),
        5 => Some(E2StructuralNodeKind::Then),
        6 => Some(E2StructuralNodeKind::Else),
        7 => Some(E2StructuralNodeKind::Condition),
        8 => Some(E2StructuralNodeKind::Action),
        9 => Some(E2StructuralNodeKind::While),
        10 => Some(E2StructuralNodeKind::For),
        11 => Some(E2StructuralNodeKind::SwitchStatement),
        12 => Some(E2StructuralNodeKind::WhileCondition),
        13 => Some(E2StructuralNodeKind::WhileBody),
        14 => Some(E2StructuralNodeKind::ForBody),
        15 => Some(E2StructuralNodeKind::Functions),
        16 => Some(E2StructuralNodeKind::FunctionDefinition),
        17 => Some(E2StructuralNodeKind::Argument),
        18 => Some(E2StructuralNodeKind::Code),
        19 => Some(E2StructuralNodeKind::Function),
        20 => Some(E2StructuralNodeKind::Wait),
        21 => Some(E2StructuralNodeKind::Folder),
        22 => Some(E2StructuralNodeKind::FolderAction),
        23 => Some(E2StructuralNodeKind::RawString),
        24 => Some(E2StructuralNodeKind::RawTrigger),
        25 => Some(E2StructuralNodeKind::RawTriggers),
        26 => Some(E2StructuralNodeKind::TriggerCondition),
        27 => Some(E2StructuralNodeKind::TriggerAction),
        28 => Some(E2StructuralNodeKind::Switch),
        29 => Some(E2StructuralNodeKind::SwitchCase),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct E2Definition {
    pub(crate) identifier: &'static str,
    pub(crate) value_kinds: &'static [&'static str],
    pub(crate) template: E2SemanticTemplate,
}

impl E2Definition {
    pub(crate) const fn arity(self) -> usize {
        self.value_kinds.len()
    }
}

macro_rules! definition {
    ($identifier:literal; [$($value_kind:literal),* $(,)?]) => {
        definition!($identifier; [$($value_kind),*]; Builtin)
    };
    ($identifier:literal; [$($value_kind:literal),* $(,)?]; $template:ident) => {
        E2Definition {
            identifier: $identifier,
            value_kinds: &[$($value_kind),*],
            template: E2SemanticTemplate::$template,
        }
    };
}

const ACTIONS: &[E2Definition] = &[
    definition!("Victory"; ["None"]), definition!("Defeat"; ["None"]),
    definition!("PreserveTrigger"; ["None"]), definition!("Wait(Don't Use)"; ["Time"]),
    definition!("PauseGame"; ["None"]), definition!("UnpauseGame"; ["None"]),
    definition!("Transmission"; ["Unit", "Where", "WAVName", "TimeModifier", "Time", "Text", "AlwaysDisplay"]),
    definition!("PlayWAV"; ["WAVName"]), definition!("DisplayText"; ["Text", "AlwaysDisplay"]),
    definition!("CenterView"; ["Where"]), definition!("CreateUnitWithProperties"; ["Count", "Unit", "Where", "Player", "Properties"]),
    definition!("SetMissionObjectives"; ["Text"]), definition!("SetSwitch"; ["Switch", "SState"]),
    definition!("SetCountdownTimer"; ["TimeModifier", "Time"]), definition!("RunAIScript"; ["Script"]),
    definition!("RunAIScriptAt"; ["Script", "Where"]), definition!("LeaderBoardControl"; ["Unit", "Label"]),
    definition!("LeaderBoardControlAt"; ["Unit", "Location", "Label"]), definition!("LeaderBoardResources"; ["ResourceType", "Label"]),
    definition!("LeaderBoardKills"; ["Unit", "Label"]), definition!("LeaderBoardScore"; ["ScoreType", "Label"]),
    definition!("KillUnit"; ["Unit", "Player"]), definition!("KillUnitAt"; ["Count", "Unit", "Where", "ForPlayer"]),
    definition!("RemoveUnit"; ["Unit", "Player"]), definition!("RemoveUnitAt"; ["Count", "Unit", "Where", "ForPlayer"]),
    definition!("SetResources"; ["Player", "Modifier", "Amount", "ResourceType"]), definition!("SetScore"; ["Player", "Modifier", "Amount", "ScoreType"]),
    definition!("MinimapPing"; ["Where"]), definition!("TalkingPortrait"; ["Unit", "Time"]),
    definition!("MuteUnitSpeech"; ["None"]), definition!("UnMuteUnitSpeech"; ["None"]),
    definition!("LeaderBoardComputerPlayers"; ["State"]), definition!("LeaderBoardGoalControl"; ["Goal", "Unit", "Label"]),
    definition!("LeaderBoardGoalControlAt"; ["Goal", "Unit", "Location", "Label"]), definition!("LeaderBoardGoalResources"; ["Goal", "ResourceType", "Label"]),
    definition!("LeaderBoardGoalKills"; ["Goal", "Unit", "Label"]), definition!("LeaderBoardGoalScore"; ["Goal", "ScoreType", "Label"]),
    definition!("MoveLocation"; ["Location", "OnUnit", "Owner", "DestLocation"]), definition!("MoveUnit"; ["Count", "UnitType", "Owner", "StartLocation", "DestLocation"]),
    definition!("LeaderBoardGreed"; ["Goal"]), definition!("SetNextScenario"; ["ScenarioName"]),
    definition!("SetDoodadState"; ["State", "Unit", "Owner", "Where"]), definition!("SetInvincibility"; ["State", "Unit", "Owner", "Where"]),
    definition!("CreateUnit"; ["Number", "Unit", "Where", "ForPlayer"]), definition!("SetDeaths"; ["Player", "Modifier", "Number", "Unit"]),
    definition!("Order"; ["Unit", "Owner", "StartLocation", "OrderType", "DestLocation"]), definition!("Comment"; ["Text"]),
    definition!("GiveUnits"; ["Count", "Unit", "Owner", "Where", "NewOwner"]), definition!("ModifyUnitHitPoints"; ["Count", "Unit", "Owner", "Where", "Percent"]),
    definition!("ModifyUnitEnergy"; ["Count", "Unit", "Owner", "Where", "Percent"]), definition!("ModifyUnitShields"; ["Count", "Unit", "Owner", "Where", "Percent"]),
    definition!("ModifyUnitResourceAmount"; ["Count", "Owner", "Where", "NewValue"]), definition!("ModifyUnitHangarCount"; ["Add", "Count", "Unit", "Owner", "Where"]),
    definition!("PauseTimer"; ["None"]), definition!("UnpauseTimer"; ["None"]), definition!("Draw"; ["None"]), definition!("SetAllianceStatus"; ["Player", "Status"]),
    definition!("EUDPart"; ["EUDPart"]; StructuralMarker), definition!("EUDTurbo"; ["None"]; EudExtension),
    definition!("SetMemory"; ["Offset", "Number", "Modifier"]; EudExtension), definition!("SetMemoryEPD"; ["Offset", "Number", "Modifier"]; EudExtension),
    definition!("SetCurrentPlayer"; ["Player"]; EudExtension), definition!("SetKills"; ["PlayerX", "Unit", "Number", "Modifier"]; EudExtension),
    definition!("SetSupply"; ["PlayerX", "Races", "SupplyType", "Amount", "Modifier"]; EudExtension), definition!("SetScoreEUD"; ["PlayerX", "ScoreOffset", "Amount", "Modifier"]; EudExtension),
    definition!("SetLight"; ["Number", "Modifier"]; EudExtension), definition!("SetUpgrade"; ["PlayerX", "Upgrade", "UTType", "Count", "Modifier"]; EudExtension),
    definition!("SetTech"; ["PlayerX", "Techdata", "UTType", "Count", "Modifier"]; EudExtension), definition!("SetDatfile"; ["DatFile", "Object", "OffsetName", "DValue"]; EudExtension),
    definition!("AddDatfile"; ["DatFile", "Object", "OffsetName", "DValue"]; EudExtension), definition!("CreatePlayerVariable"; ["Name"]; EudExtension),
    definition!("CreateVariable"; ["Name", "Count"]; EudExtension), definition!("CreateVariableWithNoini"; ["Name"]; EudExtension),
    definition!("Rand"; ["Variable", "Count", "VariableModifier"]; EudExtension), definition!("SetVariable"; ["Variable", "Count", "VariableModifier"]; EudExtension),
    definition!("SetVariableDeaths"; ["Variable", "VariableModifier", "PlayerX", "Unit", "Number"]; EudExtension), definition!("SetVariableKills"; ["Variable", "VariableModifier", "PlayerX", "Unit", "Number"]; EudExtension),
    definition!("SetVariableScore"; ["Variable", "VariableModifier", "PlayerX", "ScoreOffset"]; EudExtension), definition!("SetVariableDatFile"; ["DatFile", "Object", "OffsetName", "Variable", "VariableModifier"]; EudExtension),
    definition!("SetVariableCUnitData"; ["UnitPTR", "StructOffset", "Variable", "VariableModifier"]; EudExtension), definition!("SetVariableCUnitDataEPD"; ["UnitEPD", "StructOffset", "Variable", "VariableModifier"]; EudExtension),
    definition!("SetButton"; ["UnitBtn", "BtnData"]; EudExtension), definition!("ButtonRefresh"; ["None"]; EudExtension),
    definition!("ChangeStarText"; ["stat_txt", "Number", "Text"]; EudExtension), definition!("ChangeButtonEnableMsg"; ["BtnEnableTxt", "Number", "Text"]; EudExtension),
    definition!("ChangeButtonUnEnableMsg"; ["BtnUnEnableTxt", "Number", "Text"]; EudExtension), definition!("STRUCTPart"; ["STRUCTPart"]; StructuralMarker),
    definition!("SetNextUnitPtr"; ["Variable"]; EudExtension), definition!("SetNextUnitEPD"; ["Variable"]; EudExtension),
    definition!("SetCUnitData"; ["UnitPTR", "StructOffset", "SValue"]; EudExtension), definition!("AddCUnitData"; ["UnitPTR", "StructOffset", "SValue"]; EudExtension),
    definition!("SetCUnitDataEPD"; ["UnitEPD", "StructOffset", "SValue"]; EudExtension), definition!("AddCUnitDataEPD"; ["UnitEPD", "StructOffset", "SValue"]; EudExtension),
    definition!("CUSTOMPart"; ["CUSTOMPart"]; StructuralMarker), definition!("RawCode"; ["RawString"]; RawCode),
    definition!("BGMStop"; ["PlayerX"]; Bgm), definition!("BGMResume"; ["PlayerX"]; Bgm), definition!("BGMPlay"; ["PlayerX", "BGM", "BGMFlag"]; Bgm),
    definition!("PrintError"; ["PlayerX"]; CustomText), definition!("ChatWrite"; ["Line", "Text"]; UnsupportedChatWrite),
    definition!("ChatAnnouncement"; ["Player", "CText"]; CustomText), definition!("DisplayCText"; ["CText", "DisplayOption"]; CustomText),
    definition!("MakeCText"; ["CText"]; CustomText), definition!("AddCText"; ["CText"]; CustomText), definition!("AddCode"; ["RawString"]; CustomText), definition!("DisplaySavedCText"; ["DisplayOption"]; CustomText),
    definition!("SCDB:SaveData"; ["None"]; Scdb), definition!("SCDB:LoadData"; ["None"]; Scdb),
    definition!("SCDB:UseCustomMsg"; ["Boolean"]; Scdb), definition!("SCDB:LastMsgReset"; ["None"]; Scdb),
];

const CONDITIONS: &[E2Definition] = &[
    definition!("Accumulate"; ["Player", "Comparison", "Number", "ResourceType"]), definition!("Always"; ["None"]),
    definition!("Bring"; ["Player", "Comparison", "Number", "Unit", "Location"]), definition!("Command"; ["Player", "Comparison", "Number", "Unit"]),
    definition!("CommandLeast"; ["Unit"]), definition!("CommandLeastAt"; ["Unit", "Location"]), definition!("CommandMost"; ["Unit"]), definition!("CommandMostAt"; ["Unit", "Location"]),
    definition!("CountdownTimer"; ["Comparison", "Time"]), definition!("Deaths"; ["Player", "Comparison", "Number", "Unit"]), definition!("ElapsedTime"; ["Comparison", "Time"]),
    definition!("HighestScore"; ["ScoreType"]), definition!("Kills"; ["Player", "Comparison", "Number", "Unit"]), definition!("LeastKills"; ["Unit"]),
    definition!("LeastResources"; ["ResourceType"]), definition!("LowestScore"; ["ScoreType"]), definition!("MostKills"; ["Unit"]), definition!("MostResources"; ["ResourceType"]),
    definition!("Never"; ["None"]), definition!("Opponents"; ["Player", "Comparison", "Number"]), definition!("Score"; ["Player", "ScoreType", "Comparison", "Number"]),
    definition!("Switch"; ["Switch", "CState"]), definition!("Briefing"; ["None"]), definition!("EUDPart"; ["EUDPart"]; StructuralMarker),
    definition!("Memory"; ["Offset", "Number", "Comparison"]; EudExtension), definition!("MemoryEPD"; ["Offset", "Number", "Comparison"]; EudExtension),
    definition!("LocalCurrentPlayer"; ["PlayerX"]; EudExtension), definition!("ScoreEUD"; ["PlayerX", "ScoreOffset", "Number", "Comparison"]; EudExtension),
    definition!("Supply"; ["PlayerX", "Races", "SupplyType", "Amount", "Comparison"]; EudExtension), definition!("Upgrade"; ["PlayerX", "Upgrade", "UTType", "Count", "VariableComparison"]; EudExtension),
    definition!("Tech"; ["PlayerX", "Techdata", "UTType", "Count", "VariableComparison"]; EudExtension), definition!("Variable"; ["Variable", "Count", "VariableComparison"]; EudExtension),
    definition!("KeyDetect"; ["KeyCode"]; EudExtension), definition!("Datfile"; ["DatFile", "OffsetName", "Object", "VariableComparison", "Count"]; EudExtension),
    definition!("STRUCTPart"; ["STRUCTPart"]; StructuralMarker), definition!("CUnitData"; ["UnitPTR", "StructOffset", "VariableComparison", "SValue"]; EudExtension),
    definition!("CUnitDataEPD"; ["UnitEPD", "StructOffset", "VariableComparison", "SValue"]; EudExtension), definition!("CUSTOMPart"; ["CUSTOMPart"]; StructuralMarker),
    definition!("RawCode"; ["RawString"]; RawCode), definition!("CurrentBGM"; ["Player", "BGM"]; Bgm), definition!("BGMPlaying"; ["Player", "BGMFlag"]; Bgm),
    definition!("SCDB:Msg"; ["SCDBMsgType"]; Scdb), definition!("SCDB:Connect"; ["SCDBConnectStatus"]; Scdb), definition!("SCDB:Loadable"; ["None"]; Scdb),
];

pub(crate) fn action_definitions() -> &'static [E2Definition] { ACTIONS }

pub(crate) fn condition_definitions() -> &'static [E2Definition] { CONDITIONS }

pub(crate) fn action_definition(identifier: &str) -> Option<&'static E2Definition> {
    ACTIONS.iter().find(|definition| definition.identifier == identifier)
}

pub(crate) fn condition_definition(identifier: &str) -> Option<&'static E2Definition> {
    CONDITIONS.iter().find(|definition| definition.identifier == identifier)
}

pub(crate) fn catalog_inventory_digest() -> u64 {
    catalog_inventory_digest_for(ACTIONS, CONDITIONS)
}

fn catalog_inventory_digest_for(actions: &[E2Definition], conditions: &[E2Definition]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for (kind, definitions) in [(b'A', actions), (b'C', conditions)] {
        hash = hash_byte(hash, kind);
        for definition in definitions {
            hash = hash_bytes(hash, definition.identifier.as_bytes());
            hash = hash_byte(hash, 0);
            hash = hash_byte(hash, arity_token(definition.arity()));
            for value_kind in definition.value_kinds {
                hash = hash_bytes(hash, value_kind.as_bytes());
                hash = hash_byte(hash, 0);
            }
        }
    }
    hash
}

fn arity_token(arity: usize) -> u8 {
    match arity {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 5,
        6 => 6,
        7 => 7,
        _ => u8::MAX,
    }
}

fn hash_byte(hash: u64, byte: u8) -> u64 {
    hash.wrapping_mul(0x0000_0100_0000_01b3) ^ u64::from(byte)
}

fn hash_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash = hash_byte(hash, *byte);
    }
    hash
}
