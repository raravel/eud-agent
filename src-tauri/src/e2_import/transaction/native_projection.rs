use std::collections::BTreeMap;
use std::path::Path;

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::config::DataDirs;
use crate::native_build::DatCatalog;
use crate::native_project::{ProjectManifest, ProjectSettings, PROJECT_SCHEMA_VERSION};

use super::super::bgm::{convert_bgm, E2BgmError, E2BgmRequest, LIFECYCLE_CALL};
use super::super::compat::{compile_compat, E2ArtifactConsumer, E2CompatRequest};
use super::super::compiler::{compile_trigger_tree_with_context, CompileContext};
use super::super::dat_projection::{decode_dat, project_dat};
use super::super::resources::E2PlannedCopy;
use super::super::{
    E2Activation, E2ImportManifest, E2ImportProvenance, E2Issue, E2RawValue, ProjectResourceKind,
};
use super::player_scope;
use super::preflight::PreparedImport;

pub(super) struct NativeProjection {
    pub manifest: ProjectManifest,
    pub dat: crate::native_project::NativeDatState,
    pub files: BTreeMap<String, Vec<u8>>,
    pub copies: Vec<E2PlannedCopy>,
}

pub(super) fn compile(
    prepared: &PreparedImport,
    dirs: &DataDirs,
    compat_root: &Path,
    cancellation: &ProcessCancellation,
) -> Result<NativeProjection, E2Issue> {
    if cancellation.is_cancelled() {
        return Err(cancelled_issue());
    }
    let catalog = DatCatalog::load(compat_root).map_err(invalid)?;
    let projection = decode_dat(&prepared.document, &catalog)?;
    let dat = project_dat(&projection, &catalog)?.state;
    let player_scope = player_scope::force_selected_node(&prepared.document)
        .map(|node| player_scope::resolve(&prepared.resources.copies, dirs, node))
        .transpose()?;
    let context = CompileContext { player_scope };
    let mut compiled = compile_trigger_tree_with_context(&prepared.document, &context)?
        .files
        .into_iter()
        .map(|(path, text)| (path, text.into_bytes()))
        .collect::<BTreeMap<_, _>>();
    let compat = compile_compat(E2CompatRequest {
        document: &prepared.document,
        resources: &prepared.resources.copies,
        player_race: player_race(&prepared.document),
    })?;
    let mut python_entrypoints = Vec::new();
    for artifact in compat.artifacts {
        if artifact.consumer == E2ArtifactConsumer::PythonEntrypoint {
            python_entrypoints.push(artifact.path.clone());
        }
        compiled.insert(artifact.path, artifact.bytes);
    }
    let bgm = convert_bgm(
        E2BgmRequest {
            document: &prepared.document,
            resources: &prepared.resources,
            data_dirs: dirs,
            temp_root: &dirs.audio_temp_dir(),
        },
        || cancellation.is_cancelled(),
    )
    .map_err(|error| match error {
        E2BgmError::Cancelled => cancelled_issue(),
        E2BgmError::Fatal(issue) => issue,
    })?;
    let mut copies = prepared.resources.copies.clone();
    if let Some(output) = bgm {
        for (path, text) in output.sources.files {
            compiled.insert(path, text.into_bytes());
        }
        copies.extend(output.generated_copies);
        {
            let main = compiled
                .get_mut("src/main.eps")
                .ok_or_else(|| invalid("compiled sources have no main EPS"))?;
            let text = String::from_utf8(main.clone())
                .map_err(|_| invalid("compiled main EPS is not UTF-8"))?;
            let text = if text.contains("import e2.bgm as BGM;\n") {
                text
            } else {
                text.replacen(
                    "import e2.tct as tct;\n",
                    "import e2.tct as tct;\nimport e2.bgm as BGM;\n",
                    1,
                )
            };
            let markers = text.matches("// E2-BGM-LIFECYCLE").count();
            if markers != 1 {
                return Err(invalid(format!(
                    "compiled main EPS has {markers} BGM lifecycle markers"
                )));
            }
            *main = text
                .replacen("// E2-BGM-LIFECYCLE", LIFECYCLE_CALL, 1)
                .into_bytes();
        }
    }
    copies.sort_by(|left, right| left.resource.id.cmp(&right.resource.id));
    python_entrypoints.sort();
    let map = copies
        .iter()
        .find(|copy| copy.resource.kind == ProjectResourceKind::Map)
        .ok_or_else(|| invalid("resolved E2 project has no input map"))?;
    let extension = Path::new(&map.resource.path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("scx");
    let disabled_sections = prepared
        .document
        .features
        .iter()
        .filter_map(|state| {
            (state.activation != E2Activation::Active)
                .then(|| {
                    state.raw_payload.clone().map(|raw_payload| {
                        super::super::E2DisabledSectionProvenance {
                            feature: state.feature,
                            activation: state.activation,
                            raw_payload,
                        }
                    })
                })
                .flatten()
        })
        .collect();
    let resources = copies.iter().map(|copy| copy.resource.clone()).collect();
    let manifest = ProjectManifest {
        schema_version: PROJECT_SCHEMA_VERSION,
        name: prepared.document.settings.name.clone(),
        source_map: map.resource.path.clone(),
        output_map: crate::native_project::default_output_map(
            &prepared.document.settings.name,
            extension,
        ),
        main_file: "src/main.eps".to_string(),
        settings: ProjectSettings::default(),
        plugins: Vec::new(),
        python_entrypoints,
        python_dependencies: Vec::new(),
        python_lock: None,
        editor_compatibility: None,
        e2_import: Some(E2ImportManifest {
            version: prepared.document.version,
            source_kind: prepared.document.source_kind,
            fingerprint: prepared.resources.fingerprint.clone(),
            resources,
            provenance: E2ImportProvenance {
                source_name: prepared.source_name.clone(),
                disabled_sections,
            },
        }),
    };
    Ok(NativeProjection {
        manifest,
        dat,
        files: compiled,
        copies,
    })
}

fn player_race(document: &super::super::E2Document) -> Option<u8> {
    let section = document.raw_sections.iter().find(|section| {
        section.name == "BinEditSET" && section.activation == E2Activation::Active
    })?;
    let E2RawValue::List(rows) = &section.payload else {
        return None;
    };
    rows.iter()
        .filter_map(|row| match row {
            E2RawValue::String(value) => value.split("PlayerRace=").nth(1),
            _ => None,
        })
        .filter_map(|tail| {
            tail.split(|character: char| !character.is_ascii_digit())
                .next()
        })
        .find_map(|value| value.parse().ok())
}

fn invalid(message: impl Into<String>) -> E2Issue {
    super::issue(super::E2IssueCode::MalformedInput, message)
}
fn cancelled_issue() -> E2Issue {
    invalid("E2 import was cancelled")
}
