//! Integration smoke tests for the safe `isom` wrapper.
//!
//! `abi_version` and the error-mapping assertions run by default (they only
//! touch the C `isom_abi_version` and pure Rust). `ffi_smoke` is `#[ignore]`d
//! because it requires the COLD ~10-15 min isom-sys MSBuild of ICU/CascLib and a
//! real sample map; run it explicitly with `-- --ignored`.

use std::path::PathBuf;

/// The linked static lib reports ABI version 1 (and matches the -sys const).
#[test]
fn abi_version_is_one() {
    assert_eq!(isom::abi_version(), 1);
    assert_eq!(isom::abi_version(), isom_sys::ISOM_ABI_VERSION as i32);
}

/// A NUL byte inside the path can never reach the C side — the CString build
/// fails and maps to InvalidArg, with no FFI call and no allocation to free.
#[test]
fn embedded_nul_path_is_invalid_arg() {
    let bad = PathBuf::from("a\0b.scx");
    let err = isom::chk_extract(&bad).expect_err("a NUL-bearing path must error");
    assert!(matches!(err, isom::IsomError::InvalidArg), "got {err:?}");
}

/// Extract the CHK from a real EUD map fixture and assert a non-empty buffer
/// comes back (proving the alloc-copy-free round trip through the C ABI).
#[test]
#[ignore = "needs the cold isom-sys MSBuild + sample.scx fixture"]
fn ffi_smoke() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample.scx");
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
    assert!(chk.len() > 16, "CHK suspiciously small: {} bytes", chk.len());
}
