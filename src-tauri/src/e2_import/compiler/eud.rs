use super::super::definitions::E2Definition;
use super::super::{E2ImportIssue, E2TriggerNode};
use super::eud_memory::{
    a, assign, button_refresh, cunit_read, cunit_write, dat_read, dat_write, set_button,
    upgrade_action, upgrade_condition,
};
use super::metadata::CompileMetadata;
use super::semantics::{raw, unknown};

pub(super) fn action(
    node: &E2TriggerNode,
    definition: &E2Definition,
    metadata: &CompileMetadata,
) -> Result<String, E2ImportIssue> {
    let result = match node.name.as_str() {
        "EUDTurbo" => "SetMemory(0x6509A0, SetTo, 0)".into(),
        "SetMemory" | "SetMemoryEPD" => format!(
            "{}({}, {}, {})",
            node.name,
            a(node, definition, 0),
            a(node, definition, 2),
            a(node, definition, 1)
        ),
        "SetCurrentPlayer" => format!("setcurpl({})", a(node, definition, 0)),
        "SetKills" => format!(
            "SetMemoryEPD(EPD(0x5878A4) + {} + {} * 12, {}, {})",
            a(node, definition, 0),
            a(node, definition, 1),
            a(node, definition, 3),
            a(node, definition, 2)
        ),
        "SetSupply" => format!(
            "SetMemoryEPD(EPD(0x582144) + {} * 36 + {} * 12 + {}, {}, {})",
            a(node, definition, 1),
            a(node, definition, 2),
            a(node, definition, 0),
            a(node, definition, 4),
            a(node, definition, 3)
        ),
        "SetScoreEUD" => format!(
            "SetMemoryEPD(EPD({}) + {}, {}, {})",
            a(node, definition, 1),
            a(node, definition, 0),
            a(node, definition, 3),
            a(node, definition, 2)
        ),
        "SetLight" => format!(
            "SetMemoryEPD(EPD(0x657A9C), {}, {})",
            a(node, definition, 1),
            a(node, definition, 0)
        ),
        "SetUpgrade" => upgrade_action(node, definition, false),
        "SetTech" => upgrade_action(node, definition, true),
        "SetDatfile" => dat_write(node, definition, metadata, false)?,
        "AddDatfile" => dat_write(node, definition, metadata, true)?,
        "CreatePlayerVariable" => format!(
            "const {} = [0, 0, 0, 0, 0, 0, 0, 0]",
            a(node, definition, 0)
        ),
        "CreateVariable" => format!(
            "var {} = {}",
            a(node, definition, 0),
            a(node, definition, 1)
        ),
        "CreateVariableWithNoini" => format!("var {}", a(node, definition, 0)),
        "Rand" => assign(
            node,
            definition,
            &format!("dwrand() % ({} + 1)", a(node, definition, 1)),
        )?,
        "SetVariable" => assign(node, definition, &a(node, definition, 1))?,
        "SetVariableDeaths" => assign(
            node,
            definition,
            &format!(
                "dwread_epd({} + {} * 12)",
                a(node, definition, 2),
                a(node, definition, 3)
            ),
        )?,
        "SetVariableKills" => assign(
            node,
            definition,
            &format!(
                "dwread_epd(EPD(0x5878A4) + {} + {} * 12)",
                a(node, definition, 2),
                a(node, definition, 3)
            ),
        )?,
        "SetVariableScore" => assign(
            node,
            definition,
            &format!(
                "dwread_epd(EPD({}) + {})",
                a(node, definition, 3),
                a(node, definition, 2)
            ),
        )?,
        "SetVariableDatFile" => assign(node, definition, &dat_read(node, metadata, 0, 2, 1)?)?,
        "SetVariableCUnitData" => assign(
            node,
            definition,
            &cunit_read(node, definition, metadata, false)?,
        )?,
        "SetVariableCUnitDataEPD" => assign(
            node,
            definition,
            &cunit_read(node, definition, metadata, true)?,
        )?,
        "SetButton" => set_button(node, metadata)?,
        "ButtonRefresh" => button_refresh().into(),
        "ChangeStarText" => format!(
            "tct.setTbl({}, {}, {})",
            a(node, definition, 0),
            a(node, definition, 1),
            a(node, definition, 2)
        ),
        "ChangeButtonEnableMsg" | "ChangeButtonUnEnableMsg" => {
            let id = metadata
                .button_string(raw(node, 0), node.name == "ChangeButtonEnableMsg")
                .map_or_else(|| a(node, definition, 0), |value| value.to_string());
            format!(
                "tct.setTbl({id}, {}, {})",
                a(node, definition, 1),
                a(node, definition, 2)
            )
        }
        "SetNextUnitPtr" => format!("{} = dwread_epd(EPD(0x628438))", a(node, definition, 0)),
        "SetNextUnitEPD" => format!("{} = epdread_epd(EPD(0x628438))", a(node, definition, 0)),
        "SetCUnitData" => cunit_write(node, definition, metadata, false, false)?,
        "AddCUnitData" => cunit_write(node, definition, metadata, false, true)?,
        "SetCUnitDataEPD" => cunit_write(node, definition, metadata, true, false)?,
        "AddCUnitDataEPD" => cunit_write(node, definition, metadata, true, true)?,
        _ => return Err(unknown(node)),
    };
    Ok(result)
}

pub(super) fn condition(
    node: &E2TriggerNode,
    definition: &E2Definition,
    metadata: &CompileMetadata,
) -> Result<String, E2ImportIssue> {
    let result = match node.name.as_str() {
        "Memory" | "MemoryEPD" => format!(
            "{}({}, {}, {})",
            node.name,
            a(node, definition, 0),
            a(node, definition, 2),
            a(node, definition, 1)
        ),
        "LocalCurrentPlayer" => format!("Memory(0x57F1B0, Exactly, {})", a(node, definition, 0)),
        "ScoreEUD" => format!(
            "MemoryEPD(EPD({}) + {}, {}, {})",
            a(node, definition, 1),
            a(node, definition, 0),
            a(node, definition, 3),
            a(node, definition, 2)
        ),
        "Supply" => format!(
            "MemoryEPD(EPD(0x582144) + {} * 36 + {} * 12 + {}, {}, {})",
            a(node, definition, 1),
            a(node, definition, 2),
            a(node, definition, 0),
            a(node, definition, 4),
            a(node, definition, 3)
        ),
        "Upgrade" => upgrade_condition(node, definition, false),
        "Tech" => upgrade_condition(node, definition, true),
        "Variable" => format!(
            "{} {} {}",
            a(node, definition, 0),
            a(node, definition, 2),
            a(node, definition, 1)
        ),
        "KeyDetect" => format!("bread(0x596A18 + {}) == 1", a(node, definition, 0)),
        "Datfile" => format!(
            "{} {} {}",
            dat_read(node, metadata, 0, 1, 2)?,
            a(node, definition, 3),
            a(node, definition, 4)
        ),
        "CUnitData" => format!(
            "{} {} {}",
            cunit_read(node, definition, metadata, false)?,
            a(node, definition, 2),
            a(node, definition, 3)
        ),
        "CUnitDataEPD" => format!(
            "{} {} {}",
            cunit_read(node, definition, metadata, true)?,
            a(node, definition, 2),
            a(node, definition, 3)
        ),
        _ => return Err(unknown(node)),
    };
    Ok(result)
}
