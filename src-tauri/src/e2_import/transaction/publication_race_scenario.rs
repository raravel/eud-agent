use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct RaceFixture {
    root: PathBuf,
    destination: PathBuf,
    external: PathBuf,
    link: PathBuf,
}

impl RaceFixture {
    fn create(tag: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must follow Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "eud-agent-e2-publication-race-{tag}-{}-{nonce}",
            std::process::id()
        ));
        let destination = root.join("destination");
        let external = root.join("external");
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(root.join("source.e2s"), b"source-before").unwrap();
        fs::write(root.join("config.json"), b"config-before").unwrap();
        fs::write(external.join("sentinel.bin"), b"external-before").unwrap();
        let link = destination.join("maps");
        Self {
            root,
            destination,
            external,
            link,
        }
    }

    fn preserved(&self) {
        assert_eq!(
            fs::read(self.root.join("source.e2s")).unwrap(),
            b"source-before"
        );
        assert_eq!(
            fs::read(self.root.join("config.json")).unwrap(),
            b"config-before"
        );
        assert_eq!(
            fs::read(self.external.join("sentinel.bin")).unwrap(),
            b"external-before"
        );
    }
}

impl Drop for RaceFixture {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.link)
            .is_ok_and(|metadata| crate::memory::is_reparse_point(&metadata))
        {
            fs::remove_dir(&self.link).expect("junction must be removed without traversal");
        }
        fs::remove_dir_all(&self.root).expect("owned race fixture must be removed");
    }
}

pub(super) fn run() {
    root_and_receipt_owned_directories_are_accepted();
    concurrent_junction_cannot_escape();
    concurrent_ordinary_child_is_rejected();
}

fn root_and_receipt_owned_directories_are_accepted() {
    let fixture = RaceFixture::create("owned");
    let mut receipt = Vec::new();
    super::publication::create_parents_for_test(
        &fixture.destination,
        &fixture.destination,
        &mut receipt,
    )
    .unwrap();
    let nested = fixture.destination.join("maps/nested");
    super::publication::create_parents_for_test(&fixture.destination, &nested, &mut receipt)
        .unwrap();
    super::publication::create_parents_for_test(&fixture.destination, &nested, &mut receipt)
        .expect("receipt-owned directories must be reusable");
    assert_eq!(receipt, vec![fixture.destination.join("maps"), nested]);
    fixture.preserved();
}

fn concurrent_ordinary_child_is_rejected() {
    let fixture = RaceFixture::create("ordinary");
    fs::create_dir(&fixture.link).unwrap();
    fs::write(fixture.link.join("concurrent.bin"), b"concurrent-before").unwrap();
    let mut receipt = Vec::new();
    let result = super::publication::create_parents_for_test(
        &fixture.destination,
        &fixture.link,
        &mut receipt,
    );
    assert!(result.is_err(), "concurrent child directory must collide");
    assert!(receipt.is_empty());
    assert_eq!(
        fs::read(fixture.link.join("concurrent.bin")).unwrap(),
        b"concurrent-before"
    );
    fixture.preserved();
}

#[cfg(windows)]
fn concurrent_junction_cannot_escape() {
    use std::io::Write;

    let fixture = RaceFixture::create("junction");
    let status = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:E2_RACE_LINK -Target $env:E2_RACE_TARGET | Out-Null",
        ])
        .env("E2_RACE_LINK", &fixture.link)
        .env("E2_RACE_TARGET", &fixture.external)
        .status()
        .expect("junction command must start");
    assert!(status.success(), "junction must be created");
    fs::write(
        fixture.destination.join("concurrent.bin"),
        b"concurrent-before",
    )
    .unwrap();
    let mut receipt = Vec::new();
    let admitted = super::publication::create_parents_for_test(
        &fixture.destination,
        &fixture.link,
        &mut receipt,
    )
    .is_ok();
    if admitted {
        let escaped = fixture.link.join("escaped.bin");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&escaped)
            .expect("admitted parent redirects publication outside destination");
        file.write_all(b"escaped-write").unwrap();
        file.sync_all().unwrap();
    }
    let escaped_during_admission = fixture.external.join("escaped.bin").exists();
    if escaped_during_admission {
        fs::remove_file(fixture.external.join("escaped.bin")).unwrap();
    }
    assert!(!admitted, "junction parent must be rejected");
    assert!(
        !escaped_during_admission,
        "publication must not write outside destination"
    );
    assert_eq!(
        fs::read(fixture.destination.join("concurrent.bin")).unwrap(),
        b"concurrent-before"
    );
    assert!(receipt.is_empty());
    fixture.preserved();
}

#[cfg(not(windows))]
fn concurrent_junction_cannot_escape() {}
