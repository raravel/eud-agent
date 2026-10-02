use super::{
    active_section, artifact, issue, resource_extension, slug, E2ArtifactConsumer, E2CompatRequest,
};
use crate::e2_import::{E2Feature, E2ImportIr, E2IssueCode, E2RawValue, ProjectResourceConsumer};

const DATA_DUMPER_ORDER: [&str; 7] = [
    "grpwire", "tranwire", "wirefram", "cmdicons", "stat_txt", "AIscript", "iscript",
];

pub(super) fn reject_effective_grp(
    document: &E2ImportIr,
    resources: &[crate::e2_import::resources::E2PlannedCopy],
) -> Result<(), crate::e2_import::E2ImportIssue> {
    if let Some(section) = active_section(document, "GRPEditorSET") {
        let effective = section.fields.iter().any(|(key, value)| {
            (key.starts_with("GRPUse") && matches!(value, E2RawValue::Bool(true)))
                || (matches!(
                    key.as_str(),
                    "GRPEditorDATCount" | "GRPEditorUsingDATCount" | "GRPEditorUsingindexDATCount"
                ) && numeric_nonzero(value))
        });
        if effective {
            return Err(grp_issue("effective E2 GRP replacement is unsupported"));
        }
    }
    for copy in resources {
        if resource_extension(copy).is_some_and(|extension| extension.eq_ignore_ascii_case("grp"))
            && resource_enabled(document, &copy.resource.id)
        {
            return Err(grp_issue("active data-dumper GRP payload is unsupported"));
        }
    }
    Ok(())
}

pub(super) fn compile(
    request: &E2CompatRequest<'_>,
) -> Result<Vec<super::GeneratedE2Artifact>, crate::e2_import::E2ImportIssue> {
    let mut active = request
        .resources
        .iter()
        .filter(|copy| {
            matches!(
                copy.resource.consumer,
                ProjectResourceConsumer::Plugin | ProjectResourceConsumer::FileManager
            ) && resource_enabled(request.document, &copy.resource.id)
        })
        .collect::<Vec<_>>();
    active.sort_by_key(|copy| resource_order(&copy.resource.id));
    let mut artifacts = Vec::with_capacity(active.len());
    for copy in active {
        let key = copy
            .resource
            .id
            .rsplit_once(':')
            .map_or(copy.resource.id.as_str(), |(_, key)| key);
        let extension = resource_extension(copy)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if is_file_manager(key) || copy.resource.consumer == ProjectResourceConsumer::FileManager {
            if extension == "grp" {
                return Err(grp_issue("active data-dumper GRP payload is unsupported"));
            }
            if !matches!(extension.as_str(), "bin" | "tbl" | "dat") {
                return Err(invalid_file(
                    "file-manager resource must be BIN, TBL, or DAT",
                ));
            }
            artifacts.push(file_artifact(copy, key)?);
        } else {
            artifacts.push(plugin_artifact(copy, &extension)?);
        }
    }
    Ok(artifacts)
}

fn plugin_artifact(
    copy: &crate::e2_import::resources::E2PlannedCopy,
    extension: &str,
) -> Result<super::GeneratedE2Artifact, crate::e2_import::E2ImportIssue> {
    if copy.bytes.starts_with(b"MZ") || matches!(extension, "dll" | "exe" | "com" | "bat" | "cmd") {
        return Err(issue(
            E2IssueCode::UnsafePlugin,
            E2Feature::Plugin,
            "executable E2 plugin forms are unsafe",
        ));
    }
    let (consumer, destination) = match extension {
        "eps" => (E2ArtifactConsumer::EdsPlugin, "eud-agent.eds:before-main"),
        "py" => (
            E2ArtifactConsumer::PythonEntrypoint,
            "project.eap:pythonEntrypoints",
        ),
        _ => {
            return Err(issue(
                E2IssueCode::UnsafePlugin,
                E2Feature::Plugin,
                "E2 plugin must be EPS or Python source",
            ));
        }
    };
    std::str::from_utf8(&copy.bytes).map_err(|_| {
        issue(
            E2IssueCode::UnsafePlugin,
            E2Feature::Plugin,
            "E2 plugin source must be UTF-8 text",
        )
    })?;
    let path = format!("src/e2/plugins/{}.{}", slug(&copy.resource.id), extension);
    Ok(artifact(
        "PluginSET",
        vec![copy.resource.id.clone()],
        path,
        destination.to_string(),
        consumer,
        copy.bytes.clone(),
    ))
}

fn file_artifact(
    copy: &crate::e2_import::resources::E2PlannedCopy,
    key: &str,
) -> Result<super::GeneratedE2Artifact, crate::e2_import::E2ImportIssue> {
    let destination = file_destination(key)
        .ok_or_else(|| invalid_file("file-manager resource has no grounded runtime destination"))?;
    let bytes = format!(
        "[dataDumper]\n{} : {}, copy\n# e2-resource-sha256={}\n",
        copy.resource.path.replace(':', "\\:"),
        destination,
        copy.resource.sha256
    )
    .into_bytes();
    Ok(artifact(
        "FileManagerSET",
        vec![copy.resource.id.clone()],
        format!("src/e2/file_manager/{}.eds", slug(&copy.resource.id)),
        format!("dataDumper:{destination}"),
        E2ArtifactConsumer::EdsPlugin,
        bytes,
    ))
}

fn resource_enabled(document: &E2ImportIr, id: &str) -> bool {
    let Some((section_name, key)) = id.split_once(':') else {
        return true;
    };
    let Some(section) = active_section(document, section_name) else {
        return section_name == "PluginSET";
    };
    if let Some(family) = key.strip_prefix("dataDumper_") {
        let use_enabled = !matches!(
            section.fields.get("dataDumperuse"),
            Some(E2RawValue::Bool(false))
        );
        return use_enabled
            && section
                .fields
                .get(&format!("dataDumper_{family}_f"))
                .is_none_or(numeric_nonzero);
    }
    section
        .fields
        .get(&format!("{key}Enabled"))
        .is_none_or(|value| matches!(value, E2RawValue::Bool(true)))
}

fn resource_order(id: &str) -> (usize, &str) {
    let key = id.rsplit_once(':').map_or(id, |(_, key)| key);
    let key = key.strip_prefix("dataDumper_").unwrap_or(key);
    let index = DATA_DUMPER_ORDER
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(key))
        .unwrap_or(DATA_DUMPER_ORDER.len());
    (index, id)
}

fn is_file_manager(key: &str) -> bool {
    let key = key.strip_prefix("dataDumper_").unwrap_or(key);
    DATA_DUMPER_ORDER
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(key))
}

fn file_destination(key: &str) -> Option<&'static str> {
    let key = key.strip_prefix("dataDumper_").unwrap_or(key);
    match key.to_ascii_lowercase().as_str() {
        "stat_txt" => Some("0x6D5A30"),
        "aiscript" => Some("0x68C104"),
        "iscript" => Some("0x6D1200"),
        _ => None,
    }
}

fn numeric_nonzero(value: &E2RawValue) -> bool {
    match value {
        E2RawValue::Integer(number) => *number != 0,
        E2RawValue::String(number) => number != "0",
        E2RawValue::Bool(value) => *value,
        E2RawValue::List(_) | E2RawValue::Object(_) | E2RawValue::Null => false,
    }
}

fn grp_issue(message: &str) -> crate::e2_import::E2ImportIssue {
    issue(E2IssueCode::ActiveGrpUnsupported, E2Feature::Grp, message)
}

fn invalid_file(message: &str) -> crate::e2_import::E2ImportIssue {
    issue(
        E2IssueCode::InvalidResource,
        E2Feature::FileManager,
        message,
    )
}
