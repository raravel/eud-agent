use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use encoding_rs::EUC_KR;

use super::{
    E2Activation, E2Document, E2Feature, E2FeatureState, E2Issue, E2IssueCode, E2IssueSeverity,
    E2ProjectSettings, E2RawSection, E2RawValue, E2ResourceReference, E2SourceKind, E2TriggerNode,
    E2TriggerNodeKind, E2Version, ProjectResourceKind,
};

pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SECTIONS: usize = 64;
pub const MAX_LINES: usize = 200_000;
pub const MAX_LINE_BYTES: usize = 64 * 1024;
pub const MAX_VALUE_BYTES: usize = 1024 * 1024;
pub const MAX_TRIGGER_DEPTH: usize = 128;
pub const MAX_TRIGGER_NODES: usize = 100_000;
pub const MAX_DAT_RECORDS: usize = 100_000;
pub const MAX_RESOURCES: usize = 4_096;
pub const MAX_RESOURCE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_AGGREGATE_RESOURCE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_PATH_BYTES: usize = 4_096;
pub const MAX_SOURCE_CANDIDATES: usize = 128;

const VERSION: &str = "0.17.9.7";
const TRIGGER_VALUE_SEPARATOR: &str = "ஐ";
const KNOWN_SECTIONS: [&str; 13] = [
    "ProjectSET",
    "DatEditSET",
    "FireGraftSET",
    "BtnSET",
    "ReqSET",
    "BinEditSET",
    "SoundPlayerSET",
    "GRPEditorSET",
    "TileSET",
    "SCDBSet",
    "FileManagerSET",
    "TriggerEditorSET",
    "PluginSET",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2ParseOutput {
    pub document: E2Document,
    pub issues: Vec<E2Issue>,
}

#[derive(Debug)]
struct Section {
    name: String,
    lines: Vec<String>,
    fields: BTreeMap<String, E2RawValue>,
}

pub fn parse_e2_bytes(
    bytes: &[u8],
    source_kind: E2SourceKind,
) -> Result<E2ParseOutput, Vec<E2Issue>> {
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(fatal("input exceeds the 4 MiB text limit"));
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => EUC_KR
            .decode_without_bom_handling_and_without_replacement(bytes)
            .map(|text| text.into_owned())
            .ok_or_else(|| fatal("source is neither UTF-8 nor CP949"))?,
    };
    parse_text(&text.replace("\r\n", "\n").replace('\r', "\n"), source_kind)
}

pub fn parse_e2_path(path: &Path) -> Result<E2ParseOutput, Vec<E2Issue>> {
    check_path(path)?;
    let source = if path.is_dir() {
        select_source(path)?
    } else {
        path.to_path_buf()
    };
    let kind = match source.extension().and_then(|value| value.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("e2p") => E2SourceKind::E2p,
        Some(ext) if ext.eq_ignore_ascii_case("e2s") => {
            if path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("e2p"))
            {
                E2SourceKind::E2p
            } else {
                E2SourceKind::E2s
            }
        }
        _ => return Err(fatal("source must have an .e2s or .e2p extension")),
    };
    let metadata = fs::metadata(&source)
        .map_err(|error| fatal(format!("cannot inspect E2 source: {error}")))?;
    if metadata.len() > u64::try_from(MAX_TEXT_BYTES).unwrap_or(u64::MAX) {
        return Err(fatal("input exceeds the 4 MiB text limit"));
    }
    let bytes =
        fs::read(&source).map_err(|error| fatal(format!("cannot read E2 source: {error}")))?;
    let mut output = parse_e2_bytes(&bytes, kind)?;
    if output.document.settings.name == "Imported E2 Project" {
        if let Some(name) = source.file_stem().and_then(|value| value.to_str()) {
            output.document.settings.name = name.to_string();
        }
    }
    Ok(output)
}

pub fn validate_resource_budget(sizes: &[u64]) -> Result<(), E2Issue> {
    if sizes.len() > MAX_RESOURCES {
        return Err(make_issue(
            "resource count exceeds 4,096",
            E2IssueSeverity::Fatal,
        ));
    }
    let mut total = 0u64;
    for size in sizes {
        if *size > MAX_RESOURCE_BYTES {
            return Err(make_issue(
                "resource exceeds 64 MiB",
                E2IssueSeverity::Fatal,
            ));
        }
        total = total
            .checked_add(*size)
            .ok_or_else(|| make_issue("resource size overflow", E2IssueSeverity::Fatal))?;
        if total > MAX_AGGREGATE_RESOURCE_BYTES {
            return Err(make_issue(
                "resources exceed 512 MiB",
                E2IssueSeverity::Fatal,
            ));
        }
    }
    Ok(())
}

fn parse_text(text: &str, source_kind: E2SourceKind) -> Result<E2ParseOutput, Vec<E2Issue>> {
    let lines = text.split('\n').collect::<Vec<_>>();
    if lines.len() > MAX_LINES {
        return Err(fatal("input exceeds 200,000 lines"));
    }
    if lines.iter().any(|line| line.len() > MAX_LINE_BYTES) {
        return Err(fatal("input contains a line longer than 64 KiB"));
    }
    let sections = split_sections(&lines)?;
    let project = required_section(&sections, "ProjectSET")?;
    let version = required_string(&project.fields, "Version")?;
    if version != VERSION {
        return Err(vec![E2Issue {
            code: E2IssueCode::UnsupportedVersion,
            severity: E2IssueSeverity::Fatal,
            message: format!("unsupported E2 version {version:?}; expected {VERSION}"),
            path: None,
            feature: None,
            need_id: None,
        }]);
    }
    let gates = (0..=8)
        .map(|index| bool_field(&project.fields, &format!("UsedSetting{index}"), false))
        .collect::<Result<Vec<_>, _>>()?;
    let configured_scdb = find_section(&sections, "SCDBSet")
        .map(|section| bool_field(&section.fields, "SCDBUse", false))
        .transpose()?
        .unwrap_or(false);
    let grp_effective = find_section(&sections, "GRPEditorSET").is_some_and(grp_effective);

    let mut issues = sections
        .iter()
        .filter(|section| !KNOWN_SECTIONS.contains(&section.name.as_str()))
        .map(|section| {
            make_issue(
                format!("unknown section {} preserved", section.name),
                E2IssueSeverity::Warning,
            )
        })
        .collect::<Vec<_>>();
    if gates[5] && grp_effective {
        issues.push(feature_issue(
            E2IssueCode::ActiveGrpUnsupported,
            E2Feature::Grp,
            "effective GRP requires preflight refusal",
        ));
    }

    if let Some(dat) = find_section(&sections, "DatEditSET") {
        let rows = dat
            .lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        if rows.len() > MAX_DAT_RECORDS {
            return Err(fatal("DAT record count exceeds 100,000"));
        }
        for row in rows {
            let values = row.split(',').map(str::trim).collect::<Vec<_>>();
            if values.len() != 4 || values.iter().any(|value| value.parse::<i64>().is_err()) {
                return Err(fatal(format!("malformed DAT record {row:?}")));
            }
        }
    }

    let mut node_count = 0;
    let triggers = find_section(&sections, "TriggerEditorSET")
        .map(|section| parse_triggers(&section.lines, &mut node_count))
        .transpose()?
        .unwrap_or_default();
    let scdb_active = configured_scdb || (gates[6] && trigger_has_prefix(&triggers, "SCDB:"));
    if scdb_active {
        issues.push(feature_issue(
            E2IssueCode::ActiveScdbUnsupported,
            E2Feature::Scdb,
            "active SCDB requires preflight refusal",
        ));
    }
    if gates[6] && trigger_has(&triggers, "ChatWrite") {
        issues.push(feature_issue(
            E2IssueCode::ActiveChatwriteUnverified,
            E2Feature::StructGui,
            "active ChatWrite requires preflight refusal",
        ));
    }
    let resources = collect_resources(&sections, &gates)?;
    let settings = E2ProjectSettings {
        name: string_field(&project.fields, "ProjectName")
            .or_else(|| string_field(&project.fields, "Name"))
            .unwrap_or_else(|| "Imported E2 Project".to_string()),
        input_map: required_string(&project.fields, "InputMap")?,
        output_map: required_string(&project.fields, "OutputMap")?,
        sound_interval: find_section(&sections, "SoundPlayerSET")
            .and_then(|section| string_field(&section.fields, "Soundinterval"))
            .map(|value| value.parse::<u32>())
            .transpose()
            .map_err(|_| fatal("Soundinterval must be unsigned"))?
            .unwrap_or(0),
        raw_fields: project.fields.clone(),
    };
    let features = feature_inventory(&sections, &gates, scdb_active, grp_effective);
    let raw_sections = sections
        .into_iter()
        .map(|section| {
            let activation = section_activation(&section, &gates, scdb_active, grp_effective);
            E2RawSection {
                name: section.name,
                activation,
                fields: section.fields,
                payload: E2RawValue::List(
                    section.lines.into_iter().map(E2RawValue::String).collect(),
                ),
            }
        })
        .collect();
    Ok(E2ParseOutput {
        document: E2Document {
            version: E2Version::V0_17_9_7,
            source_kind,
            settings,
            features,
            triggers,
            resources,
            raw_sections,
        },
        issues,
    })
}

fn split_sections(lines: &[&str]) -> Result<Vec<Section>, Vec<E2Issue>> {
    let mut sections = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    let mut names = BTreeSet::new();
    for line in lines {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("S_") {
            if current.is_some() || name.is_empty() {
                return Err(fatal("nested or empty section start"));
            }
            if !names.insert(name.to_string()) {
                return Err(fatal(format!("duplicate authoritative section {name}")));
            }
            current = Some((name.to_string(), Vec::new()));
        } else if let Some(name) = trimmed.strip_prefix("E_") {
            let Some((open, body)) = current.take() else {
                return Err(fatal("section end without start"));
            };
            if open != name {
                return Err(fatal(format!("section {open} closed by {name}")));
            }
            let fields = parse_fields(&open, &body)?;
            sections.push(Section {
                name: open,
                lines: body,
                fields,
            });
            if sections.len() > MAX_SECTIONS {
                return Err(fatal("section count exceeds 64"));
            }
        } else if let Some((_, body)) = &mut current {
            body.push((*line).to_string());
        } else if !trimmed.is_empty() {
            return Err(fatal("content outside an E2 section"));
        }
    }
    if let Some((name, _)) = current {
        return Err(fatal(format!("section {name} is truncated")));
    }
    Ok(sections)
}

fn parse_fields(
    name: &str,
    lines: &[String],
) -> Result<BTreeMap<String, E2RawValue>, Vec<E2Issue>> {
    let mut fields = BTreeMap::new();
    if matches!(name, "DatEditSET" | "TriggerEditorSET") {
        return Ok(fields);
    }
    for line in lines.iter().filter(|line| !line.trim().is_empty()) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.strip_prefix(' ').unwrap_or(value).trim_end();
        if key.is_empty() || value.len() > MAX_VALUE_BYTES {
            return Err(fatal(format!("invalid field in {name}")));
        }
        let raw = if value.eq_ignore_ascii_case("true") {
            E2RawValue::Bool(true)
        } else if value.eq_ignore_ascii_case("false") {
            E2RawValue::Bool(false)
        } else if let Ok(number) = value.parse::<i64>() {
            E2RawValue::Integer(number)
        } else {
            E2RawValue::String(value.to_string())
        };
        if fields.insert(key.to_string(), raw).is_some() {
            return Err(fatal(format!(
                "duplicate authoritative field {key} in {name}"
            )));
        }
    }
    Ok(fields)
}

fn parse_triggers(lines: &[String], count: &mut usize) -> Result<Vec<E2TriggerNode>, Vec<E2Issue>> {
    let mut cursor = 0;
    let mut nodes = Vec::new();
    while cursor < lines.len() {
        let line = lines[cursor].trim();
        if line.is_empty() {
            cursor += 1;
        } else if let Some((name, kind)) = trigger_section(line) {
            cursor += 1;
            while lines.get(cursor).is_some_and(|line| line.trim().is_empty()) {
                cursor += 1;
            }
            let id = next_trigger_id(count)?;
            let child = parse_node(lines, &mut cursor, 2, count)?;
            nodes.push(E2TriggerNode {
                id,
                enabled: true,
                folded: false,
                not_condition: false,
                kind,
                name: name.to_string(),
                values: Vec::new(),
                children: vec![child],
            });
        } else if line.starts_with('&') {
            cursor += 1;
        } else if line.starts_with("Type:") {
            nodes.push(parse_node(lines, &mut cursor, 1, count)?);
        } else {
            return Err(fatal(format!("unexpected trigger line {line:?}")));
        }
    }
    Ok(nodes)
}

fn trigger_section(line: &str) -> Option<(&'static str, E2TriggerNodeKind)> {
    match line {
        "&AddText&" => Some(("section:AddText", E2TriggerNodeKind::Group)),
        "&functions&" => Some(("section:functions", E2TriggerNodeKind::Group)),
        "&GlobalVar&" => Some(("section:GlobalVar", E2TriggerNodeKind::Group)),
        "&RawTriggers&" => Some(("section:RawTriggers", E2TriggerNodeKind::Group)),
        "&onPluginStart&" => Some(("onPluginStart", E2TriggerNodeKind::Lifecycle)),
        "&beforeTriggerExec&" => Some(("beforeTriggerExec", E2TriggerNodeKind::Lifecycle)),
        "&afterTriggerExec&" => Some(("afterTriggerExec", E2TriggerNodeKind::Lifecycle)),
        _ => None,
    }
}

fn next_trigger_id(count: &mut usize) -> Result<String, Vec<E2Issue>> {
    *count = count
        .checked_add(1)
        .ok_or_else(|| fatal("trigger count overflow"))?;
    if *count > MAX_TRIGGER_NODES {
        return Err(fatal("trigger node count exceeds 100,000"));
    }
    Ok(format!("node-{count}"))
}

fn parse_node(
    lines: &[String],
    cursor: &mut usize,
    depth: usize,
    count: &mut usize,
) -> Result<E2TriggerNode, Vec<E2Issue>> {
    if depth > MAX_TRIGGER_DEPTH {
        return Err(fatal("trigger depth exceeds 128"));
    }
    let id = next_trigger_id(count)?;
    let header = lines
        .get(*cursor)
        .ok_or_else(|| fatal("truncated trigger"))?
        .trim()
        .strip_prefix("Type:")
        .ok_or_else(|| fatal("missing trigger Type"))?;
    *cursor += 1;
    let flags = header.split(',').map(str::trim).collect::<Vec<_>>();
    if flags.len() > 4 {
        return Err(fatal("too many trigger flags"));
    }
    let type_id = flags
        .first()
        .ok_or_else(|| fatal("empty trigger Type"))?
        .parse::<u8>()
        .map_err(|_| fatal("nonnumeric trigger Type"))?;
    let disabled = trigger_bool(flags.get(1).copied().unwrap_or("False"))?;
    let folded = trigger_bool(flags.get(2).copied().unwrap_or("False"))?;
    let not_condition = trigger_bool(flags.get(3).copied().unwrap_or("False"))?;
    let mut kind = match type_id {
        7 => E2TriggerNodeKind::Condition,
        8 => E2TriggerNodeKind::Action,
        16 | 19 => E2TriggerNodeKind::Group,
        _ => E2TriggerNodeKind::Group,
    };
    let mut name = format!("type-{type_id}");
    if let Some(line) = lines.get(*cursor).map(|line| line.trim()) {
        if let Some(value) = line.strip_prefix("act:") {
            kind = E2TriggerNodeKind::Action;
            name = value.to_string();
            *cursor += 1;
        } else if let Some(value) = line.strip_prefix("con:") {
            kind = E2TriggerNodeKind::Condition;
            name = value.to_string();
            *cursor += 1;
        }
    }
    let mut raw_values = Vec::new();
    let children_count = loop {
        let line = lines
            .get(*cursor)
            .ok_or_else(|| fatal("trigger truncated before ElementsCount"))?
            .trim();
        *cursor += 1;
        if let Some(value) = line.strip_prefix("ElementsCount:") {
            break value
                .trim()
                .parse::<usize>()
                .map_err(|_| fatal("invalid ElementsCount"))?;
        }
        raw_values.push(line.to_string());
        let size = raw_values
            .iter()
            .try_fold(0usize, |total, value| total.checked_add(value.len() + 1))
            .ok_or_else(|| fatal("trigger value overflow"))?;
        if size > MAX_VALUE_BYTES {
            return Err(fatal("trigger value exceeds 1 MiB"));
        }
    };
    let values = raw_values
        .join("\n")
        .split(TRIGGER_VALUE_SEPARATOR)
        .map(|value| E2RawValue::String(value.to_string()))
        .collect();
    let mut children = Vec::with_capacity(children_count.min(1024));
    for _ in 0..children_count {
        children.push(parse_node(lines, cursor, depth + 1, count)?);
    }
    let end = lines
        .get(*cursor)
        .ok_or_else(|| fatal("trigger truncated before END"))?
        .trim();
    if end != "END" {
        return Err(fatal(format!("expected trigger END, found {end:?}")));
    }
    *cursor += 1;
    Ok(E2TriggerNode {
        id,
        enabled: !disabled,
        folded,
        not_condition,
        kind,
        name,
        values,
        children,
    })
}

fn trigger_bool(value: &str) -> Result<bool, Vec<E2Issue>> {
    match value.to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(fatal(format!("invalid trigger boolean {value:?}"))),
    }
}

fn collect_resources(
    sections: &[Section],
    gates: &[bool],
) -> Result<Vec<E2ResourceReference>, Vec<E2Issue>> {
    let mut result = Vec::new();
    for section in sections {
        let (feature, kind, active) = match section.name.as_str() {
            "ProjectSET" => (E2Feature::ProjectMap, ProjectResourceKind::Map, true),
            "BinEditSET" => (E2Feature::Bin, ProjectResourceKind::Bin, gates[2]),
            "SoundPlayerSET" => (
                E2Feature::SoundBgmReferences,
                ProjectResourceKind::Audio,
                gates[4],
            ),
            "GRPEditorSET" => (E2Feature::Grp, ProjectResourceKind::File, gates[5]),
            "TileSET" => (E2Feature::CustomTiles, ProjectResourceKind::Tile, gates[3]),
            "PluginSET" => (E2Feature::Plugin, ProjectResourceKind::Plugin, gates[7]),
            "FileManagerSET" => (E2Feature::FileManager, ProjectResourceKind::File, gates[8]),
            _ => continue,
        };
        if !active {
            continue;
        }
        for (key, value) in &section.fields {
            let E2RawValue::String(path) = value else {
                continue;
            };
            if !resource_field(&section.name, key, path) {
                continue;
            }
            check_reference(path)?;
            result.push(E2ResourceReference {
                id: format!("{}:{key}", section.name),
                feature,
                kind,
                source_path: path.clone(),
            });
        }
        if section.name == "SoundPlayerSET" {
            for (index, line) in section.lines.iter().enumerate() {
                let path = line.trim();
                if path.is_empty() || path.contains(':') || !resource_like(path) {
                    continue;
                }
                check_reference(path)?;
                result.push(E2ResourceReference {
                    id: format!("SoundPlayerSET:{index}"),
                    feature,
                    kind,
                    source_path: path.to_string(),
                });
            }
        }
        if result.len() > MAX_RESOURCES {
            return Err(fatal("resource reference count exceeds 4,096"));
        }
    }
    Ok(result)
}

fn resource_field(section: &str, key: &str, value: &str) -> bool {
    match section {
        "ProjectSET" => matches!(key, "InputMap" | "OutputMap"),
        "TileSET" => key == "ProjectTileSetFileName" && !value.is_empty(),
        "SoundPlayerSET" => {
            key.starts_with("Sound") && key != "Soundinterval" && resource_like(value)
        }
        "GRPEditorSET" => key.contains("Filename") && !value.is_empty(),
        "PluginSET" => !key.ends_with("Enabled") && resource_like(value),
        "FileManagerSET" | "BinEditSET" => resource_like(value),
        _ => false,
    }
}

fn resource_like(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        ".scx", ".scm", ".ogg", ".wav", ".bin", ".bmp", ".grp", ".eps", ".tbl", ".dat",
    ]
    .iter()
    .any(|extension| value.ends_with(extension))
}

fn check_reference(path: &str) -> Result<(), Vec<E2Issue>> {
    if path.len() > MAX_PATH_BYTES {
        Err(fatal("resource path exceeds 4,096 bytes"))
    } else {
        Ok(())
    }
}

fn feature_inventory(
    sections: &[Section],
    gates: &[bool],
    scdb_use: bool,
    grp_is_effective: bool,
) -> Vec<E2FeatureState> {
    let specs = [
        (E2Feature::ProjectMap, None, "ProjectSET"),
        (E2Feature::Dat, Some(0), "DatEditSET"),
        (E2Feature::FireGraft, Some(1), "FireGraftSET"),
        (E2Feature::Bin, Some(2), "BinEditSET"),
        (E2Feature::CustomTiles, Some(3), "TileSET"),
        (E2Feature::ButtonsBgm, Some(4), "BtnSET"),
        (E2Feature::Grp, Some(5), "GRPEditorSET"),
        (E2Feature::StructGui, Some(6), "TriggerEditorSET"),
        (E2Feature::Plugin, Some(7), "PluginSET"),
        (E2Feature::FileManager, Some(8), "FileManagerSET"),
        (E2Feature::Scdb, None, "SCDBSet"),
        (E2Feature::SoundBgmReferences, Some(4), "SoundPlayerSET"),
    ];
    let mut result = specs
        .into_iter()
        .map(|(feature, gate, name)| {
            let section = find_section(sections, name);
            let enabled = match feature {
                E2Feature::ProjectMap => true,
                E2Feature::Scdb => scdb_use,
                E2Feature::Grp => gates[5] && grp_is_effective,
                _ => gate.is_some_and(|index| gates[index]),
            };
            let has_payload = section.is_some_and(|section| feature_has_payload(name, section));
            let activation = if enabled {
                E2Activation::Active
            } else if has_payload || (feature == E2Feature::Grp && gates[5]) {
                E2Activation::Inert
            } else {
                E2Activation::Inactive
            };
            E2FeatureState {
                feature,
                activation,
                raw_payload: section.map(section_payload),
            }
        })
        .collect::<Vec<_>>();
    let has_inert = result
        .iter()
        .any(|state| state.activation == E2Activation::Inert);
    result.push(E2FeatureState {
        feature: E2Feature::DisabledPayload,
        activation: if has_inert {
            E2Activation::Inert
        } else {
            E2Activation::Inactive
        },
        raw_payload: None,
    });
    result
}

fn section_activation(
    section: &Section,
    gates: &[bool],
    scdb_use: bool,
    grp_is_effective: bool,
) -> E2Activation {
    let enabled = match section.name.as_str() {
        "ProjectSET" => true,
        "DatEditSET" => gates[0],
        "FireGraftSET" | "BtnSET" | "ReqSET" => gates[1],
        "BinEditSET" => gates[2],
        "TileSET" => gates[3],
        "SoundPlayerSET" => gates[4],
        "GRPEditorSET" => gates[5] && grp_is_effective,
        "TriggerEditorSET" => gates[6],
        "PluginSET" => gates[7],
        "FileManagerSET" => gates[8],
        "SCDBSet" => scdb_use,
        _ => false,
    };
    if enabled {
        E2Activation::Active
    } else if feature_has_payload(&section.name, section)
        || (section.name == "GRPEditorSET" && gates[5])
    {
        E2Activation::Inert
    } else {
        E2Activation::Inactive
    }
}

fn grp_effective(section: &Section) -> bool {
    section.fields.iter().any(|(key, value)| {
        if key.starts_with("GRPUse") {
            matches!(value, E2RawValue::Bool(true))
        } else if matches!(
            key.as_str(),
            "GRPEditorDATCount" | "GRPEditorUsingDATCount" | "GRPEditorUsingindexDATCount"
        ) {
            match value {
                E2RawValue::Integer(number) => *number != 0,
                E2RawValue::String(number) => number != "0",
                _ => false,
            }
        } else {
            false
        }
    })
}

fn section_has_payload(section: &Section) -> bool {
    section.lines.iter().any(|line| {
        let line = line.trim();
        if line.is_empty() {
            false
        } else if let Some((_, value)) = line.split_once(':') {
            let value = value.trim();
            !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
        } else {
            true
        }
    })
}

fn feature_has_payload(name: &str, section: &Section) -> bool {
    match name {
        "SoundPlayerSET" => section.lines.iter().any(|line| {
            let line = line.trim();
            !line.is_empty()
                && !line.starts_with("Soundinterval")
                && !line.starts_with("SoundCount")
                && (resource_like(line)
                    || line
                        .split_once(':')
                        .is_some_and(|(_, value)| resource_like(value.trim())))
        }),
        "GRPEditorSET" => grp_effective(section),
        "TileSET" => {
            bool_field(&section.fields, "ProjectTileUseFile", false).unwrap_or(false)
                || [
                    "ProjectTileSetFileName",
                    "ProjectTIleMSetArray",
                    "ProjectMTXMDATAArray",
                ]
                .iter()
                .any(|key| {
                    string_field(&section.fields, key).is_some_and(|value| !value.is_empty())
                })
                || string_field(&section.fields, "ProjectTileSetDataCount")
                    .is_some_and(|value| value != "0")
        }
        "SCDBSet" => ["SCDBDeath", "SCDBLoc", "SCDBVariable", "SCDBLocLoad"]
            .iter()
            .any(|key| string_field(&section.fields, key).is_some_and(|value| !value.is_empty())),
        "PluginSET" => section.fields.iter().any(|(_, value)| match value {
            E2RawValue::Bool(value) => *value,
            E2RawValue::Integer(value) => *value != 0,
            E2RawValue::String(value) => {
                !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
            }
            E2RawValue::Null | E2RawValue::List(_) | E2RawValue::Object(_) => false,
        }),
        _ => section_has_payload(section),
    }
}

fn section_payload(section: &Section) -> E2RawValue {
    E2RawValue::List(
        section
            .lines
            .iter()
            .cloned()
            .map(E2RawValue::String)
            .collect(),
    )
}

fn trigger_has(nodes: &[E2TriggerNode], name: &str) -> bool {
    nodes
        .iter()
        .any(|node| node.enabled && (node.name == name || trigger_has(&node.children, name)))
}

fn trigger_has_prefix(nodes: &[E2TriggerNode], prefix: &str) -> bool {
    nodes.iter().any(|node| {
        node.enabled
            && (node.name.starts_with(prefix) || trigger_has_prefix(&node.children, prefix))
    })
}

fn find_section<'a>(sections: &'a [Section], name: &str) -> Option<&'a Section> {
    sections.iter().find(|section| section.name == name)
}

fn required_section<'a>(sections: &'a [Section], name: &str) -> Result<&'a Section, Vec<E2Issue>> {
    find_section(sections, name).ok_or_else(|| fatal(format!("missing section {name}")))
}

fn string_field(fields: &BTreeMap<String, E2RawValue>, key: &str) -> Option<String> {
    fields.get(key).map(|value| match value {
        E2RawValue::Null => String::new(),
        E2RawValue::Bool(value) => if *value { "True" } else { "False" }.to_string(),
        E2RawValue::Integer(value) => value.to_string(),
        E2RawValue::String(value) => value.clone(),
        E2RawValue::List(_) | E2RawValue::Object(_) => String::new(),
    })
}

fn required_string(
    fields: &BTreeMap<String, E2RawValue>,
    key: &str,
) -> Result<String, Vec<E2Issue>> {
    string_field(fields, key).ok_or_else(|| fatal(format!("missing field {key}")))
}

fn bool_field(
    fields: &BTreeMap<String, E2RawValue>,
    key: &str,
    default: bool,
) -> Result<bool, Vec<E2Issue>> {
    match fields.get(key) {
        None => Ok(default),
        Some(E2RawValue::Bool(value)) => Ok(*value),
        Some(E2RawValue::String(value)) if value.eq_ignore_ascii_case("true") => Ok(true),
        Some(E2RawValue::String(value)) if value.eq_ignore_ascii_case("false") => Ok(false),
        Some(_) => Err(fatal(format!("field {key} must be True or False"))),
    }
}

fn select_source(root: &Path) -> Result<PathBuf, Vec<E2Issue>> {
    let preferred = root.join("project.e2s");
    if preferred.is_file() {
        return Ok(preferred);
    }
    let entries =
        fs::read_dir(root).map_err(|error| fatal(format!("cannot enumerate package: {error}")))?;
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| fatal(format!("cannot enumerate package: {error}")))?;
        if !entry
            .file_type()
            .map_err(|error| fatal(format!("cannot inspect package member: {error}")))?
            .is_file()
        {
            continue;
        }
        let path = entry.path();
        if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("e2s") || ext.eq_ignore_ascii_case("e2p"))
        {
            candidates.push(path);
            if candidates.len() > MAX_SOURCE_CANDIDATES {
                return Err(fatal("source candidate count exceeds 128"));
            }
        }
    }
    candidates.sort();
    match candidates.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(fatal("package has no source member")),
        _ => Err(fatal("package has ambiguous source members")),
    }
}

fn check_path(path: &Path) -> Result<(), Vec<E2Issue>> {
    if path.as_os_str().to_string_lossy().len() > MAX_PATH_BYTES {
        Err(fatal("source path exceeds 4,096 bytes"))
    } else {
        Ok(())
    }
}

fn feature_issue(code: E2IssueCode, feature: E2Feature, message: &str) -> E2Issue {
    E2Issue {
        code,
        severity: E2IssueSeverity::Fatal,
        message: message.to_string(),
        path: None,
        feature: Some(feature),
        need_id: None,
    }
}

fn make_issue(message: impl Into<String>, severity: E2IssueSeverity) -> E2Issue {
    E2Issue {
        code: E2IssueCode::MalformedInput,
        severity,
        message: message.into(),
        path: None,
        feature: None,
        need_id: None,
    }
}

fn fatal(message: impl Into<String>) -> Vec<E2Issue> {
    vec![make_issue(message, E2IssueSeverity::Fatal)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    mod e2_fixture {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/e2_fixture.rs"
        ));
    }

    fn source(project_fields: &[&str], extra: &str) -> String {
        let mut text = String::from(
            "S_ProjectSET\r\nVersion : 0.17.9.7\r\nInputMap : Map/input.scx\r\nOutputMap : Map/output.scx\r\n",
        );
        for index in 0..=8 {
            let key = format!("UsedSetting{index}");
            if !project_fields.iter().any(|field| field.starts_with(&key)) {
                text.push_str(&format!("{key} : False\r\n"));
            }
        }
        for field in project_fields {
            text.push_str(field);
            text.push_str("\r\n");
        }
        text.push_str("E_ProjectSET\r\n");
        text.push_str(extra);
        text
    }

    fn parse(text: &str) -> Result<E2ParseOutput, Vec<E2Issue>> {
        parse_e2_bytes(text.as_bytes(), E2SourceKind::E2s)
    }

    fn activation(output: &E2ParseOutput, feature: E2Feature) -> E2Activation {
        output
            .document
            .features
            .iter()
            .find(|state| state.feature == feature)
            .map(|state| state.activation)
            .unwrap_or(E2Activation::Inactive)
    }

    pub(super) fn all_sections_package_contract() {
        let extra = concat!(
            "S_DatEditSET\r\n0,7,42,3\r\nE_DatEditSET\r\n",
            "S_FireGraftSET\r\nFireGraft7 : 1,2,3\r\nE_FireGraftSET\r\n",
            "S_BtnSET\r\nBtnUse0 : True\r\nE_BtnSET\r\n",
            "S_ReqSET\r\nReqUse0,0 : 3\r\nE_ReqSET\r\n",
            "S_BinEditSET\r\nBin0 : fixture.bin\r\nE_BinEditSET\r\n",
            "S_SoundPlayerSET\r\nSoundinterval : 3\r\nM0_0.ogg\r\nE_SoundPlayerSET\r\n",
            "S_GRPEditorSET\r\nGRPUse0 : True\r\nE_GRPEditorSET\r\n",
            "S_TileSET\r\nProjectTileUseFile : True\r\nProjectTileSetFileName : tiles.bmp\r\nE_TileSET\r\n",
            "S_SCDBSet\r\nSCDBUse : True\r\nE_SCDBSet\r\n",
            "S_FileManagerSET\r\nstat_txt : stat_txt.tbl\r\nE_FileManagerSET\r\n",
            "S_TriggerEditorSET\r\nType:8,False,False,False\r\nact:DisplayText\r\nhello\r\nElementsCount:1\r\nType:7,False,True,True\r\ncon:Always\r\nvalue\r\nElementsCount:0\r\nEND\r\nEND\r\nE_TriggerEditorSET\r\n",
            "S_PluginSET\r\nfixturePlugin : fixture.eps\r\nE_PluginSET\r\n",
        );
        let fields = (0..=8)
            .map(|index| format!("UsedSetting{index} : True"))
            .collect::<Vec<_>>();
        let refs = fields.iter().map(String::as_str).collect::<Vec<_>>();
        let output = parse(&source(&refs, extra)).expect("all sections parse");
        assert_eq!(output.document.raw_sections.len(), 13);
        assert_eq!(output.document.triggers[0].children.len(), 1);
        assert_eq!(activation(&output, E2Feature::Scdb), E2Activation::Active);
        assert_eq!(activation(&output, E2Feature::Grp), E2Activation::Active);
        assert!(output
            .issues
            .iter()
            .any(|issue| issue.code == E2IssueCode::ActiveScdbUnsupported));
        assert!(output
            .issues
            .iter()
            .any(|issue| issue.code == E2IssueCode::ActiveGrpUnsupported));
        assert!(output
            .document
            .resources
            .iter()
            .any(|resource| resource.source_path == "tiles.bmp"));

        let fixture_root =
            std::env::temp_dir().join(format!("e2-parser-package-{}", std::process::id()));
        let fixture = e2_fixture::build_complete_fixture(&fixture_root, None)
            .expect("build complete fixture package");
        fixture
            .verify_oracle()
            .expect("fixture oracle remains stable");
        let packaged = parse_e2_path(&fixture.e2p_root).expect("fixture package parses");
        assert_eq!(packaged.document.source_kind, E2SourceKind::E2p);
        assert_eq!(
            packaged.document.raw_sections.len(),
            fixture.oracle.sections.len()
        );
        assert!(fixture.audit_paths().expect("fixture paths stay confined") > 1);
        drop(fixture);
        fs::remove_dir_all(&fixture_root).expect("remove fixture package");
    }

    #[test]
    fn disabled_payload_and_empty_grp_are_inert() {
        let extra = concat!(
            "S_DatEditSET\n0,1,2,3\nE_DatEditSET\n",
            "S_GRPEditorSET\nGRPEditorDATCount : 0\nGRPEditorUsingDATCount : 0\nGRPEditorUsingindexDATCount : 0\nE_GRPEditorSET\n",
        );
        let output = parse(&source(&["UsedSetting5 : True"], extra)).expect("gated payload parses");
        assert_eq!(activation(&output, E2Feature::Dat), E2Activation::Inert);
        assert_eq!(activation(&output, E2Feature::Grp), E2Activation::Inert);
        assert!(!output
            .issues
            .iter()
            .any(|issue| issue.code == E2IssueCode::ActiveGrpUnsupported));
    }

    #[test]
    fn enabled_scdb_node_reference_activates_preflight_but_disabled_node_is_inert() {
        let trigger = |disabled| {
            format!(
                "S_TriggerEditorSET\nType:8,{disabled},False,False\nact:SCDB:Exec\nvalue\nElementsCount:0\nEND\nE_TriggerEditorSET\n"
            )
        };
        let active = parse(&source(&["UsedSetting6 : True"], &trigger("False")))
            .expect("enabled SCDB node parses");
        assert_eq!(activation(&active, E2Feature::Scdb), E2Activation::Active);
        assert!(active
            .issues
            .iter()
            .any(|issue| issue.code == E2IssueCode::ActiveScdbUnsupported));

        let disabled = parse(&source(&["UsedSetting6 : True"], &trigger("True")))
            .expect("disabled SCDB node parses");
        assert_eq!(
            activation(&disabled, E2Feature::Scdb),
            E2Activation::Inactive
        );
        assert!(!disabled
            .issues
            .iter()
            .any(|issue| issue.code == E2IssueCode::ActiveScdbUnsupported));
    }

    pub(super) fn disabled_ancestor_activation_contract() {
        let trigger = |disabled| {
            format!(
                concat!(
                    "S_TriggerEditorSET\n",
                    "Type:0,{disabled},False,False\n",
                    "ElementsCount:2\n",
                    "Type:8,False,False,False\nact:SCDB:Exec\nvalue\nElementsCount:0\nEND\n",
                    "Type:8,False,False,False\nact:ChatWrite\nvalue\nElementsCount:0\nEND\n",
                    "END\n",
                    "E_TriggerEditorSET\n",
                ),
                disabled = disabled
            )
        };
        let output = parse(&source(&["UsedSetting6 : True"], &trigger("True")))
            .expect("disabled ancestor trigger tree parses");
        assert_eq!(activation(&output, E2Feature::Scdb), E2Activation::Inactive);
        assert!(!output.issues.iter().any(|issue| matches!(
            issue.code,
            E2IssueCode::ActiveScdbUnsupported | E2IssueCode::ActiveChatwriteUnverified
        )));

        let active = parse(&source(&["UsedSetting6 : True"], &trigger("False")))
            .expect("enabled ancestor trigger tree parses");
        assert_eq!(activation(&active, E2Feature::Scdb), E2Activation::Active);
        assert!(active
            .issues
            .iter()
            .any(|issue| issue.code == E2IssueCode::ActiveScdbUnsupported));
        assert!(active
            .issues
            .iter()
            .any(|issue| issue.code == E2IssueCode::ActiveChatwriteUnverified));
    }

    pub(super) fn trigger_section_marker_contract() {
        let trigger = concat!(
            "S_TriggerEditorSET\n",
            "&AddText&\nType:23,False,False,False\nadd-text\nElementsCount:0\nEND\n",
            "&functions&\nType:15,False,False,False\nElementsCount:2\n",
            "Type:16,False,False,False\nfixture-definition\nElementsCount:0\nEND\n",
            "Type:19,False,False,False\nfixture-call\nElementsCount:0\nEND\nEND\n",
            "&GlobalVar&\nType:15,False,False,False\nElementsCount:0\nEND\n",
            "&RawTriggers&\nType:25,False,False,False\nElementsCount:0\nEND\n",
            "&onPluginStart&\nType:23,False,False,False\nHOOK_START\nElementsCount:0\nEND\n",
            "&beforeTriggerExec&\nType:23,False,False,False\nHOOK_BEFORE\nElementsCount:0\nEND\n",
            "&afterTriggerExec&\nType:23,False,False,False\nHOOK_AFTER\nElementsCount:0\nEND\n",
            "E_TriggerEditorSET\n",
        );
        let output = parse(&source(&["UsedSetting6 : True"], trigger))
            .expect("wire-faithful trigger sections parse");
        let expected = [
            ("section:AddText", E2TriggerNodeKind::Group),
            ("section:functions", E2TriggerNodeKind::Group),
            ("section:GlobalVar", E2TriggerNodeKind::Group),
            ("section:RawTriggers", E2TriggerNodeKind::Group),
            ("onPluginStart", E2TriggerNodeKind::Lifecycle),
            ("beforeTriggerExec", E2TriggerNodeKind::Lifecycle),
            ("afterTriggerExec", E2TriggerNodeKind::Lifecycle),
        ];
        assert_eq!(output.document.triggers.len(), expected.len());
        for (wrapper, (name, kind)) in output.document.triggers.iter().zip(expected) {
            assert_eq!(wrapper.name, name);
            assert_eq!(wrapper.kind, kind);
            assert_eq!(wrapper.children.len(), 1);
        }
        let functions = &output.document.triggers[1].children[0];
        assert_eq!(functions.children[0].kind, E2TriggerNodeKind::Group);
        assert_eq!(functions.children[0].name, "type-16");
        assert_eq!(
            functions.children[0].values,
            vec![E2RawValue::String("fixture-definition".to_string())]
        );
        assert_eq!(functions.children[1].kind, E2TriggerNodeKind::Group);
        assert_eq!(functions.children[1].name, "type-19");
        assert_eq!(
            functions.children[1].values,
            vec![E2RawValue::String("fixture-call".to_string())]
        );

        let compiled = crate::e2_import::compiler::compile_trigger_tree(&output.document)
            .expect("parsed section wrappers compile");
        let main = &compiled.files["src/main.eps"];
        let start = main.find("function onPluginStart()").expect("start hook");
        let before = main
            .find("function beforeTriggerExec()")
            .expect("before hook");
        let after = main
            .find("function afterTriggerExec()")
            .expect("after hook");
        assert!(main[start..before].contains("HOOK_START"));
        assert!(main[before..after].contains("HOOK_BEFORE"));
        assert!(main[after..].contains("HOOK_AFTER"));
    }

    #[test]
    fn authentic_v0179_modify_unit_energy_wire_preserves_all_five_values_and_compiles() {
        let trigger = concat!(
            "S_TriggerEditorSET\n",
            "Type:8,False,False,False\n",
            "act:ModifyUnitEnergy\n",
            "All\u{0b90}230\u{0b90}17\u{0b90}64\u{0b90}100\n",
            "ElementsCount:0\nEND\n",
            "E_TriggerEditorSET\n",
        );
        let output = parse(&source(&["UsedSetting6 : True"], trigger))
            .expect("authentic v0.17.9.7 action wire parses");
        let action = output.document.triggers.first().expect("action node");
        assert_eq!(action.name, "ModifyUnitEnergy");
        assert_eq!(
            action.values,
            vec![
                E2RawValue::String("All".to_string()),
                E2RawValue::String("230".to_string()),
                E2RawValue::String("17".to_string()),
                E2RawValue::String("64".to_string()),
                E2RawValue::String("100".to_string()),
            ]
        );
        crate::e2_import::compiler::compile_trigger_tree(&output.document)
            .expect("authentic five-value action wire compiles");
    }

    #[test]
    fn authentic_v0179_modify_unit_energy_truncated_wire_preserves_strict_arity_failure() {
        let trigger = concat!(
            "S_TriggerEditorSET\n",
            "Type:8,False,False,False\n",
            "act:ModifyUnitEnergy\n",
            "All\u{0b90}230\u{0b90}17\u{0b90}64\n",
            "ElementsCount:0\nEND\n",
            "E_TriggerEditorSET\n",
        );
        let output = parse(&source(&["UsedSetting6 : True"], trigger))
            .expect("truncated v0.17.9.7 action wire remains parseable");
        assert_eq!(
            output.document.triggers[0].values,
            vec![
                E2RawValue::String("All".to_string()),
                E2RawValue::String("230".to_string()),
                E2RawValue::String("17".to_string()),
                E2RawValue::String("64".to_string()),
            ]
        );
        let error = crate::e2_import::compiler::compile_trigger_tree(&output.document)
            .expect_err("truncated action must not bypass serialized value arity");
        assert_eq!(error.code, E2IssueCode::MalformedInput);
        assert_eq!(error.feature, Some(E2Feature::StructGui));
        assert!(error.message.contains("ModifyUnitEnergy"), "{error:?}");
        assert!(
            error
                .message
                .contains("expects 5 serialized values, found 4"),
            "{error:?}"
        );
    }

    pub(super) fn actual_e2p_text_file_contract() {
        let root =
            std::env::temp_dir().join(format!("e2-parser-actual-package-{}", std::process::id()));
        let package_root = root.join("fixture");
        let source_path = package_root.join("fixture.e2p");
        let destination = root.join("destination");
        fs::create_dir_all(package_root.join("Map/input.scx"))
            .expect("create unread resource marker");
        fs::create_dir_all(package_root.join("Resource")).expect("create Resource companion");
        fs::create_dir_all(package_root.join("Sound")).expect("create Sound companion");
        fs::write(&source_path, source(&[], "")).expect("write nested E2P text source");
        let source_before = Sha256::digest(fs::read(&source_path).expect("hash source before"));

        let output = parse_e2_path(&source_path).expect("actual nested E2P text file parses");

        assert_eq!(output.document.source_kind, E2SourceKind::E2p);
        assert_eq!(source_path.parent(), Some(package_root.as_path()));
        assert_eq!(
            source_before.as_slice(),
            Sha256::digest(fs::read(&source_path).expect("hash source after")).as_slice()
        );
        assert!(package_root.join("Map/input.scx").is_dir());
        assert!(!destination.exists());
        fs::remove_dir_all(&root).expect("remove actual package fixture");
    }

    #[test]
    fn cp949_bom_and_newlines_are_normalized() {
        let text = source(&["ProjectName : 테스트"], "");
        let (encoded, _, errors) = EUC_KR.encode(&text);
        assert!(!errors);
        let output = parse_e2_bytes(&encoded, E2SourceKind::E2s).expect("CP949 parses");
        assert_eq!(output.document.settings.name, "테스트");
        let mut utf8 = vec![0xef, 0xbb, 0xbf];
        utf8.extend_from_slice(source(&[], "").replace("\r\n", "\r").as_bytes());
        assert!(parse_e2_bytes(&utf8, E2SourceKind::E2s).is_ok());
    }

    pub(super) fn rejection_matrix() {
        let duplicate = source(&["Version : TEST"], "");
        assert_eq!(
            parse(&duplicate).expect_err("duplicate fails")[0].code,
            E2IssueCode::MalformedInput
        );
        let unsupported = source(&[], "").replace(VERSION, "TEST-0.17.9.7");
        assert_eq!(
            parse(&unsupported).expect_err("TEST fails")[0].code,
            E2IssueCode::UnsupportedVersion
        );
        let unknown = source(&[], "").replace(VERSION, "0.17.9.8");
        assert_eq!(
            parse(&unknown).expect_err("unknown fails")[0].code,
            E2IssueCode::UnsupportedVersion
        );
        let malformed = source(&[], "S_DatEditSET\nnot,a,dat,row\nE_DatEditSET\n");
        assert!(parse(&malformed).is_err());
        let truncated = source(
            &[],
            "S_TriggerEditorSET\nType:8,False,False,False\nact:X\nElementsCount:0\n",
        );
        assert!(parse(&truncated).is_err());
        let destination =
            std::env::temp_dir().join(format!("e2-parser-no-destination-{}", std::process::id()));
        assert!(!destination.exists());
    }

    #[test]
    fn text_section_line_and_value_limits_are_exact() {
        let mut exact = source(&[], "S_Unknown\nE_Unknown\n").into_bytes();
        exact.resize(MAX_TEXT_BYTES, b' ');
        let exact_error = parse_e2_bytes(&exact, E2SourceKind::E2s)
            .expect_err("padding outside sections is malformed");
        assert!(!exact_error[0].message.contains("4 MiB"));
        exact.push(b' ');
        assert!(
            parse_e2_bytes(&exact, E2SourceKind::E2s).expect_err("over text limit")[0]
                .message
                .contains("4 MiB")
        );

        let line = "x".repeat(MAX_LINE_BYTES);
        assert!(parse(&source(&[], &format!("S_U\n{line}\nE_U\n"))).is_ok());
        let line = "x".repeat(MAX_LINE_BYTES + 1);
        assert!(parse(&source(&[], &format!("S_U\n{line}\nE_U\n"))).is_err());

        let sections = (0..(MAX_SECTIONS - 1))
            .map(|index| format!("S_U{index}\nE_U{index}\n"))
            .collect::<String>();
        assert!(parse(&source(&[], &sections)).is_ok());
        assert!(parse(&source(&[], &(sections + "S_over\nE_over\n"))).is_err());

        let mut trigger_lines = vec!["Type:23,False,False,False".to_string()];
        trigger_lines.extend((0..16).map(|_| "x".repeat(MAX_LINE_BYTES - 1)));
        trigger_lines.push("ElementsCount:0".to_string());
        trigger_lines.push("END".to_string());
        let mut count = 0;
        assert!(parse_triggers(&trigger_lines, &mut count).is_ok());
        trigger_lines.insert(17, "x".to_string());
        let mut count = 0;
        assert!(parse_triggers(&trigger_lines, &mut count).is_err());
    }

    #[test]
    fn line_dat_trigger_and_resource_count_limits_are_exact() {
        let project_lines = source(&[], "").split('\n').count();
        let extra_lines = MAX_LINES - project_lines - 2;
        let exact = source(&[], &format!("S_U\n{}E_U\n", "\n".repeat(extra_lines)));
        assert!(parse(&exact).is_ok());
        assert!(parse(&(exact + "\n")).is_err());

        let dat_rows = "0,0,0,1\n".repeat(MAX_DAT_RECORDS);
        assert!(parse(&source(
            &[],
            &format!("S_DatEditSET\n{dat_rows}E_DatEditSET\n")
        ))
        .is_ok());
        let dat_rows = "0,0,0,1\n".repeat(MAX_DAT_RECORDS + 1);
        assert!(parse(&source(
            &[],
            &format!("S_DatEditSET\n{dat_rows}E_DatEditSET\n")
        ))
        .is_err());

        let roots = (0..MAX_TRIGGER_NODES)
            .flat_map(|_| {
                [
                    "Type:0,False,False,False".to_string(),
                    "ElementsCount:0".to_string(),
                    "END".to_string(),
                ]
            })
            .collect::<Vec<_>>();
        let mut count = 0;
        assert!(parse_triggers(&roots, &mut count).is_ok());
        let mut over = roots;
        over.extend([
            "Type:0,False,False,False".to_string(),
            "ElementsCount:0".to_string(),
            "END".to_string(),
        ]);
        let mut count = 0;
        assert!(parse_triggers(&over, &mut count).is_err());

        assert!(validate_resource_budget(&vec![0; MAX_RESOURCES]).is_ok());
        assert!(validate_resource_budget(&vec![0; MAX_RESOURCES + 1]).is_err());
        assert!(validate_resource_budget(&[MAX_RESOURCE_BYTES]).is_ok());
        assert!(validate_resource_budget(&[MAX_RESOURCE_BYTES + 1]).is_err());
        assert!(validate_resource_budget(&[MAX_RESOURCE_BYTES; 8]).is_ok());
        assert!(validate_resource_budget(&[MAX_RESOURCE_BYTES; 9]).is_err());
    }

    #[test]
    fn trigger_depth_path_and_candidate_limits_are_exact() {
        fn nested(depth: usize) -> Vec<String> {
            let mut lines = Vec::new();
            for level in 0..depth {
                lines.push("Type:0,False,False,False".to_string());
                lines.push(format!("ElementsCount:{}", usize::from(level + 1 < depth)));
            }
            lines.extend((0..depth).map(|_| "END".to_string()));
            lines
        }
        let mut count = 0;
        assert!(parse_triggers(&nested(MAX_TRIGGER_DEPTH), &mut count).is_ok());
        let mut count = 0;
        assert!(parse_triggers(&nested(MAX_TRIGGER_DEPTH + 1), &mut count).is_err());
        assert!(check_reference(&"x".repeat(MAX_PATH_BYTES)).is_ok());
        assert!(check_reference(&"x".repeat(MAX_PATH_BYTES + 1)).is_err());

        let root =
            std::env::temp_dir().join(format!("e2-parser-candidates-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create candidate root");
        for index in 0..MAX_SOURCE_CANDIDATES {
            fs::write(root.join(format!("{index}.e2s")), b"").expect("write candidate");
        }
        assert!(select_source(&root).is_err());
        fs::write(root.join("over.e2s"), b"").expect("write over-limit candidate");
        assert!(select_source(&root).expect_err("over candidates")[0]
            .message
            .contains("128"));
        fs::remove_dir_all(&root).expect("remove candidate root");
    }

    pub(super) fn real_sample_contract() {
        let Some(path) = std::env::var_os("EUD_AGENT_E2_SAMPLE") else {
            return;
        };
        let output = parse_e2_path(Path::new(&path)).expect("real sample parses");
        assert_eq!(output.document.version, E2Version::V0_17_9_7);
        let dat = output
            .document
            .raw_sections
            .iter()
            .find(|section| section.name == "DatEditSET")
            .expect("DAT section");
        let E2RawValue::List(rows) = &dat.payload else {
            panic!("DAT payload list")
        };
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(row, E2RawValue::String(value) if !value.is_empty()))
                .count(),
            12
        );
        assert_eq!(activation(&output, E2Feature::Scdb), E2Activation::Inactive);
        assert_eq!(activation(&output, E2Feature::Grp), E2Activation::Inactive);
        assert_eq!(
            activation(&output, E2Feature::ButtonsBgm),
            E2Activation::Inactive
        );
        assert_eq!(
            activation(&output, E2Feature::Plugin),
            E2Activation::Inactive
        );
        assert_eq!(
            activation(&output, E2Feature::CustomTiles),
            E2Activation::Inactive
        );
        let source_bytes = fs::read(&path).expect("read sample for protected hash");
        let source_sha256 = Sha256::digest(&source_bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            source_sha256,
            "b6f38e17b9a68a7d494d490a820bb5cd2d96dc86a97bc35792be369576ce2113"
        );
        if let Some(artifact) = std::env::var_os("EUD_AGENT_E2_ARTIFACT") {
            let summary = serde_json::json!({
                "sourceSha256": source_sha256,
                "expectedSourceSha256": "b6f38e17b9a68a7d494d490a820bb5cd2d96dc86a97bc35792be369576ce2113",
                "version": output.document.version,
                "sourceKind": output.document.source_kind,
                "datRows": 12,
                "expectedDatRows": 12,
                "featureMatrix": [
                    {"feature": "dat", "actual": activation(&output, E2Feature::Dat), "expected": "active"},
                    {"feature": "scdb", "actual": activation(&output, E2Feature::Scdb), "expected": "inactive"},
                    {"feature": "grp", "actual": activation(&output, E2Feature::Grp), "expected": "inactive"},
                    {"feature": "buttonsBgm", "actual": activation(&output, E2Feature::ButtonsBgm), "expected": "inactive"},
                    {"feature": "plugin", "actual": activation(&output, E2Feature::Plugin), "expected": "inactive"},
                    {"feature": "customTiles", "actual": activation(&output, E2Feature::CustomTiles), "expected": "inactive"}
                ],
                "rawSections": output.document.raw_sections.iter().map(|section| &section.name).collect::<Vec<_>>(),
                "triggerRoots": output.document.triggers.len(),
                "resourceReferences": output.document.resources,
                "issues": output.issues,
            });
            fs::write(
                artifact,
                serde_json::to_vec_pretty(&summary).expect("serialize sample summary"),
            )
            .expect("write sample summary");
        }
    }
}

#[cfg(test)]
#[test]
fn all_sections_package_contract() {
    tests::all_sections_package_contract();
}

#[cfg(test)]
#[test]
fn rejection_matrix() {
    tests::rejection_matrix();
}

#[cfg(test)]
#[test]
fn real_sample_contract() {
    tests::real_sample_contract();
}

#[cfg(test)]
#[test]
fn disabled_ancestor_activation_contract() {
    tests::disabled_ancestor_activation_contract();
}

#[cfg(test)]
#[test]
fn trigger_section_marker_contract() {
    tests::trigger_section_marker_contract();
}

#[cfg(test)]
#[test]
fn actual_e2p_text_file_contract() {
    tests::actual_e2p_text_file_contract();
}
