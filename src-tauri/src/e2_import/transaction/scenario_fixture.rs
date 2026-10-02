use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::config::DataDirs;
use crate::e2_import::{E2ImportRequest, E2PreflightRequest};

pub(super) struct ScenarioFixture {
    pub(super) root: PathBuf,
    pub(super) source: PathBuf,
    pub(super) input_map: PathBuf,
    pub(super) destination: PathBuf,
    pub(super) dirs: DataDirs,
}

impl ScenarioFixture {
    pub(super) fn create(tag: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock must follow Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "eud-agent-e2-transaction-{tag}-{}-{nonce}",
            std::process::id()
        ));
        let package = root.join("package");
        let destination = root.join("destination");
        fs::create_dir_all(&package).expect("package directory must be created");
        fs::create_dir_all(&destination).expect("destination directory must be created");
        let source = package.join("fixture.e2s");
        let input_map = package.join("fixture.scx");
        fs::write(&input_map, b"literal-map-bytes\0task-12").expect("input map must be written");
        fs::write(&source, source_text()).expect("E2 source must be written");
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs()
            .expect("data directories must be created");
        Self {
            root,
            source,
            input_map,
            destination,
            dirs,
        }
    }

    pub(super) fn preflight_request(&self) -> E2PreflightRequest {
        E2PreflightRequest {
            source_path: self.source.to_string_lossy().into_owned(),
            destination_path: self.destination.to_string_lossy().into_owned(),
            relinks: Vec::new(),
        }
    }

    pub(super) fn import_request(&self, fingerprint: String) -> E2ImportRequest {
        E2ImportRequest {
            source_path: self.source.to_string_lossy().into_owned(),
            destination_path: self.destination.to_string_lossy().into_owned(),
            fingerprint,
            relinks: Vec::new(),
        }
    }

    pub(super) fn source_hashes(&self) -> (String, String) {
        (sha256_file(&self.source), sha256_file(&self.input_map))
    }

    pub(super) fn compat_root(&self) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/eud-editor-compat")
    }
}

impl Drop for ScenarioFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("owned transaction fixture must be removed");
    }
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).expect("fixture file must remain readable");
    format!("{:x}", Sha256::digest(bytes))
}

fn source_text() -> String {
    let mut text = String::from(
        "S_ProjectSET\r\nVersion : 0.17.9.7\r\nProjectName : Transaction Fixture\r\nInputMap : fixture.scx\r\nOutputMap : output.scx\r\nSoundinterval : 3\r\n",
    );
    for index in 0..=8 {
        text.push_str(&format!("UsedSetting{index} : False\r\n"));
    }
    text.push_str("E_ProjectSET\r\n");
    text
}
