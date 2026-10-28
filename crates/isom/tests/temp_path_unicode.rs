//! The C shim stages CHK/ops/BMP buffers in temp files under %TEMP%. A Windows
//! user folder with a Korean name puts Hangul into that path, which the engine
//! must receive as UTF-8 like every other path. This binary is its own process
//! so pointing TMP/TEMP at a Hangul folder cannot disturb other tests.

use std::fs;
use std::path::PathBuf;

#[test]
fn shim_temp_files_work_under_a_hangul_temp_folder() {
    let base = std::env::temp_dir().join(format!("isom-임시-사용자-{}", std::process::id()));
    fs::create_dir_all(&base).expect("hangul temp folder");
    // GetTempPathW reads TMP first, then TEMP.
    std::env::set_var("TMP", &base);
    std::env::set_var("TEMP", &base);

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample.scx");
    let chk = isom::chk_extract(&fixture).expect("CHK extract through a hangul temp path");
    assert!(chk.len() > 16, "CHK suspiciously small: {} bytes", chk.len());

    // The ops editors stage their program in a temp file the engine reads.
    let edited = base.join("편집.scx");
    fs::copy(&fixture, &edited).expect("fixture copy");
    isom::switchedit(&edited, b"rename|1|EUD Agent Temp Path")
        .expect("switchedit through a hangul temp path");
    let edited_chk = isom::chk_extract(&edited).expect("edited map stays readable");
    assert_ne!(edited_chk, chk, "switch rename should change the CHK");

    let leftovers: Vec<_> = fs::read_dir(&base)
        .expect("list temp folder")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name != "편집.scx")
        .collect();
    assert!(leftovers.is_empty(), "shim temp files left behind: {leftovers:?}");
    fs::remove_dir_all(&base).ok();
}
