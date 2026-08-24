//! Integration smoke tests for the safe `isom` wrapper.
//!
//! `abi_version` and the error-mapping assertions run by default (they only
//! touch the C `isom_abi_version` and pure Rust). `ffi_smoke` is `#[ignore]`d
//! because it requires the COLD ~10-15 min isom-sys MSBuild of ICU/CascLib and a
//! real sample map; run it explicitly with `-- --ignored`.

use std::fs;
use std::path::PathBuf;

/// The linked static lib reports ABI version 6 (and matches the -sys const).
#[test]
fn abi_version_is_six() {
    assert_eq!(isom::abi_version(), 6);
    assert_eq!(isom::abi_version(), isom_sys::ISOM_ABI_VERSION as i32);
    isom::assert_abi_version().expect("ABI v6 startup assertion must pass");
}

/// A NUL byte inside the path can never reach the C side — the CString build
/// fails and maps to InvalidArg, with no FFI call and no allocation to free.
#[test]
fn embedded_nul_path_is_invalid_arg() {
    let bad = PathBuf::from("a\0b.scx");
    let err = isom::chk_extract(&bad).expect_err("a NUL-bearing path must error");
    assert!(matches!(err, isom::IsomError::InvalidArg), "got {err:?}");
}

/// Exercise CHK extraction, switch rename, and terrain render against a real map.
#[test]
#[ignore = "needs the cold isom-sys MSBuild + sample.scx fixture + StarCraft data"]
fn ffi_smoke() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample.scx");
    let render_fixture = std::env::var_os("ISOM_SMOKE_MAP")
        .map(PathBuf::from)
        .unwrap_or_else(|| fixture.clone());
    assert!(
        fixture.is_file(),
        "missing fixture {} — copy a real .scx there",
        fixture.display()
    );

    let chk = isom::chk_extract(&fixture).expect("chk_extract should succeed on a valid map");
    eprintln!("ffi_smoke: extracted {} CHK bytes", chk.len());
    assert!(!chk.is_empty(), "extracted CHK must be non-empty");
    // A real CHK always carries the mandatory sections; a few bytes would mean a
    // truncated/empty extract slipped past. Guard against a degenerate buffer.
    assert!(
        chk.len() > 16,
        "CHK suspiciously small: {} bytes",
        chk.len()
    );

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let edited = std::env::temp_dir().join(format!("isom-switch-smoke-{stamp}.scx"));
    fs::copy(&fixture, &edited).expect("fixture copy should succeed");
    isom::switchedit(&edited, b"rename|1|EUD Agent FFI Smoke")
        .expect("switchedit should rename switch #1 on a copied map");
    let edited_chk = isom::chk_extract(&edited).expect("edited map should remain readable");
    assert_ne!(edited_chk, chk, "switch rename should change the CHK");
    fs::remove_file(&edited).ok();

    assert!(render_fixture.is_file(), "render fixture must exist");
    let starcraft = PathBuf::from(r"C:\Program Files (x86)\StarCraft");
    let bmp = isom::render_map(&render_fixture, &starcraft, 8)
        .expect("render_map should produce terrain pixels from installed game data");
    assert!(bmp.len() > 54, "rendered BMP must contain pixels");
    assert_eq!(&bmp[0..2], b"BM");
    if let Some(output) = std::env::var_os("ISOM_SMOKE_BMP_OUT") {
        fs::write(output, &bmp).expect("requested smoke BMP should be writable");
    }
}
