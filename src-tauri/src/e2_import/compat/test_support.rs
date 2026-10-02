use std::{collections::BTreeMap, fs, path::Path};

use sha2::{Digest, Sha256};

use super::{compile_compat, E2CompatPlan, E2CompatRequest};
use crate::e2_import::resources::E2PlannedCopy;
use crate::e2_import::{
    E2Activation, E2Document, E2Feature, E2ProjectSettings, E2RawSection, E2RawValue, E2SourceKind,
    E2Version, ProjectResource, ProjectResourceConsumer, ProjectResourceKind,
    ProjectResourceProvenance,
};

pub(super) fn section(name: &str, fields: &[(&str, E2RawValue)]) -> E2RawSection {
    E2RawSection {
        name: name.into(),
        activation: E2Activation::Active,
        fields: fields
            .iter()
            .map(|(key, value)| ((*key).into(), value.clone()))
            .collect(),
        payload: E2RawValue::Null,
    }
}

pub(super) fn document(sections: Vec<E2RawSection>) -> E2Document {
    E2Document {
        version: E2Version::V0_17_9_7,
        source_kind: E2SourceKind::E2s,
        settings: E2ProjectSettings {
            name: "compat".into(),
            input_map: "source.scx".into(),
            output_map: "output.scx".into(),
            sound_interval: 0,
            raw_fields: BTreeMap::new(),
        },
        features: Vec::new(),
        triggers: Vec::new(),
        resources: Vec::new(),
        raw_sections: sections,
    }
}

pub(super) fn copy(id: &str, kind: ProjectResourceKind, path: &str, bytes: &[u8]) -> E2PlannedCopy {
    let (consumer, feature) = match kind {
        ProjectResourceKind::Bin => (ProjectResourceConsumer::BinCompat, E2Feature::Bin),
        ProjectResourceKind::Tile => (ProjectResourceConsumer::TileCompat, E2Feature::CustomTiles),
        ProjectResourceKind::Plugin => (ProjectResourceConsumer::Plugin, E2Feature::Plugin),
        ProjectResourceKind::File => (ProjectResourceConsumer::FileManager, E2Feature::FileManager),
        _ => panic!("unsupported fixture resource kind"),
    };
    E2PlannedCopy {
        resource: ProjectResource {
            id: id.into(),
            path: path.into(),
            kind,
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            consumer,
            build_destination: path.into(),
            provenance: ProjectResourceProvenance {
                feature,
                original_path: path.into(),
            },
        },
        bytes: bytes.to_vec(),
    }
}

pub(super) fn compile(
    document: &E2Document,
    resources: &[E2PlannedCopy],
    race: Option<u8>,
) -> Result<E2CompatPlan, crate::e2_import::E2Issue> {
    compile_compat(E2CompatRequest {
        document,
        resources,
        player_race: race,
    })
}

pub(super) fn text(plan: &E2CompatPlan, path: &str) -> String {
    let artifact = plan
        .artifacts
        .iter()
        .find(|artifact| artifact.path == path)
        .unwrap_or_else(|| panic!("missing {path}"));
    String::from_utf8(artifact.bytes.clone()).expect("generated source is UTF-8")
}

pub(super) fn destinations(plan: &E2CompatPlan) -> Vec<&str> {
    plan.artifacts
        .iter()
        .map(|artifact| artifact.build_destination.as_str())
        .collect()
}

pub(super) fn error_code(
    document: &E2Document,
    resources: &[E2PlannedCopy],
) -> crate::e2_import::E2IssueCode {
    compile(document, resources, None).unwrap_err().code
}

pub(super) fn record(plan: &E2CompatPlan, label: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join(".omo/evidence/task-10-e2-project-import-v1-0-2/representative");
    fs::create_dir_all(&root).expect("create representative evidence");
    for artifact in &plan.artifacts {
        let name = format!("{label}-{}", artifact.path.replace('/', "_"));
        fs::write(root.join(name), &artifact.bytes).expect("write generated evidence");
    }
}

pub(super) fn bmp() -> Vec<u8> {
    let mut bytes = vec![0u8; 0x436 + 2_048];
    bytes[0..2].copy_from_slice(b"BM");
    bytes[10..14].copy_from_slice(&0x436u32.to_le_bytes());
    bytes[18..22].copy_from_slice(&32u32.to_le_bytes());
    bytes[22..26].copy_from_slice(&64u32.to_le_bytes());
    bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
    bytes[28..30].copy_from_slice(&8u16.to_le_bytes());
    bytes[0x436..0x836].fill(0x11);
    bytes[0x836..].fill(0x22);
    bytes
}
