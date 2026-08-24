use std::collections::BTreeMap;
#[cfg(windows)]
use std::ffi::c_void;
use std::fs;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

const OGG: &[u8] = include_bytes!("fixtures/tone.ogg");

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("map_agent_rich.scx")
}

fn temp_map(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("eud-map-sound-{tag}-{}.scx", uuid_like_stamp()))
}

fn uuid_like_stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[cfg(windows)]
unsafe extern "system" {
    fn SFileOpenArchive(
        archive_name: *const u16,
        priority: u32,
        flags: u32,
        archive: *mut *mut c_void,
    ) -> bool;
    fn SFileCloseArchive(archive: *mut c_void) -> bool;
    fn SFileAddFile(
        archive: *mut c_void,
        local_name: *const u16,
        archived_name: *const std::ffi::c_char,
        flags: u32,
    ) -> bool;
    fn SFileRemoveFile(
        archive: *mut c_void,
        archived_name: *const std::ffi::c_char,
        search_scope: u32,
    ) -> bool;
}

#[cfg(windows)]
fn edit_archive(map: &Path, operation: impl FnOnce(*mut c_void)) {
    let wide = map
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut archive = std::ptr::null_mut();
    // SAFETY: NUL-terminated path and valid out pointer; the handle is closed below.
    assert!(unsafe { SFileOpenArchive(wide.as_ptr(), 0, 0, &mut archive) });
    operation(archive);
    // SAFETY: `archive` is the successful handle returned above.
    assert!(unsafe { SFileCloseArchive(archive) });
}

#[cfg(windows)]
fn add_archive_file(map: &Path, local: &Path, archived: &str) {
    let local = local
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let archived = std::ffi::CString::new(archived).unwrap();
    edit_archive(map, |archive| {
        const MPQ_FILE_COMPRESS: u32 = 0x0000_0200;
        const MPQ_FILE_REPLACE_EXISTING: u32 = 0x8000_0000;
        // SAFETY: paths remain alive for this synchronous call.
        assert!(unsafe {
            SFileAddFile(
                archive,
                local.as_ptr(),
                archived.as_ptr(),
                MPQ_FILE_COMPRESS | MPQ_FILE_REPLACE_EXISTING,
            )
        });
    });
}

#[cfg(windows)]
fn remove_archive_file(map: &Path, archived: &str) {
    let archived = std::ffi::CString::new(archived).unwrap();
    edit_archive(map, |archive| {
        // SAFETY: path remains alive for this synchronous call.
        assert!(unsafe { SFileRemoveFile(archive, archived.as_ptr(), 0) });
    });
}

fn chk_with_used_wav_slots(chk: &[u8], used: usize) -> Vec<u8> {
    assert!(used <= 512);
    let mut result = chk.to_vec();
    let mut offset = 0usize;
    while offset + 8 <= result.len() {
        let size = i32::from_le_bytes(result[offset + 4..offset + 8].try_into().unwrap());
        assert!(size >= 0);
        let start = offset + 8;
        let end = start + size as usize;
        assert!(end <= result.len());
        if &result[offset..offset + 4] == b"WAV " {
            assert!(end - start >= 512 * 4);
            result[start..start + 512 * 4].fill(0);
            for slot in result[start..start + used * 4].chunks_exact_mut(4) {
                slot.copy_from_slice(&1u32.to_le_bytes());
            }
            return result;
        }
        offset = end;
    }
    panic!("fixture has no WAV section");
}

#[cfg(windows)]
fn map_with_used_wav_slots(used: usize, tag: &str) -> PathBuf {
    let map = temp_map(tag);
    fs::copy(fixture(), &map).unwrap();
    let chk = isom::chk_extract(&map).unwrap();
    let modified = chk_with_used_wav_slots(&chk, used);
    let chk_path = map.with_extension("scenario.chk");
    fs::write(&chk_path, modified).unwrap();
    add_archive_file(&map, &chk_path, "staredit\\scenario.chk");
    fs::remove_file(chk_path).ok();
    map
}

fn file_hash(path: &Path) -> String {
    let value: Value = serde_json::from_str(&isom::map_digest(path).unwrap()).unwrap();
    value["fileSha256"].as_str().unwrap().to_string()
}

fn sections(chk: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut result = BTreeMap::new();
    let mut offset = 0usize;
    while offset + 8 <= chk.len() {
        let name = String::from_utf8_lossy(&chk[offset..offset + 4]).into_owned();
        let size = i32::from_le_bytes(chk[offset + 4..offset + 8].try_into().unwrap());
        assert!(size >= 0);
        let start = offset + 8;
        let end = start + size as usize;
        assert!(end <= chk.len());
        result.insert(name, chk[start..end].to_vec());
        offset = end;
    }
    result
}

fn strings(sections: &BTreeMap<String, Vec<u8>>) -> Vec<Vec<u8>> {
    let (data, width) = if let Some(data) = sections.get("STRx") {
        (data, 4usize)
    } else {
        (&sections["STR "], 2usize)
    };
    let read_offset = |offset: usize| -> usize {
        match width {
            2 => u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize,
            4 => u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize,
            _ => unreachable!(),
        }
    };
    let count = read_offset(0).min((data.len() - width) / width);
    (0..count)
        .map(|index| {
            let offset = read_offset(width * (index + 1));
            if offset == 0 || offset >= data.len() {
                return Vec::new();
            }
            let end = data[offset..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|length| offset + length)
                .unwrap_or(data.len());
            data[offset..end].to_vec()
        })
        .collect()
}

fn sound_path_and_slot(path: &Path, expected_path: &str) -> (usize, usize) {
    let sections = sections(&isom::chk_extract(path).unwrap());
    let strings = strings(&sections);
    let string_id = strings
        .iter()
        .position(|value| value == expected_path.as_bytes())
        .map(|index| index + 1)
        .expect("managed game string must exist");
    let wav = &sections["WAV "];
    let slots = wav
        .chunks_exact(4)
        .map(|slot| u32::from_le_bytes(slot.try_into().unwrap()) as usize)
        .collect::<Vec<_>>();
    let indices = slots
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (*value == string_id).then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(indices.len(), 1);
    (string_id, indices[0])
}

fn extra_assets(path: &Path) -> BTreeMap<String, String> {
    let value: Value = serde_json::from_str(&isom::map_digest(path).unwrap()).unwrap();
    value["extraAssets"]["assets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|asset| {
            (
                asset["path"].as_str().unwrap().to_string(),
                asset["sha256"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn real_scx_adds_exact_mpq_string_wav_and_reuses_without_duplication() {
    let input = fixture();
    let before_bytes = fs::read(&input).unwrap();
    let before_hash = file_hash(&input);
    let before_sections = sections(&isom::chk_extract(&input).unwrap());
    let before_assets = extra_assets(&input);
    let ogg_hash = format!("{:x}", Sha256::digest(OGG));
    let mpq_path = format!("staredit\\wav\\ea_{}.ogg", &ogg_hash[..16]);
    let output = temp_map("add");

    let report = isom::map_sound_add(&input, &output, &before_hash, &mpq_path, OGG).unwrap();
    assert!(!report.reused);
    assert_eq!(report.asset_sha256, ogg_hash);
    assert_eq!(report.asset_bytes, OGG.len() as u64);
    let (string_id, sound_index) = sound_path_and_slot(&output, &mpq_path);
    assert_eq!(report.sound_string_id, string_id as u64);
    assert_eq!(report.sound_index, sound_index as u64);

    let after_sections = sections(&isom::chk_extract(&output).unwrap());
    for (name, body) in &before_sections {
        if !matches!(name.as_str(), "STR " | "STRx" | "WAV ") {
            assert_eq!(
                after_sections.get(name),
                Some(body),
                "section {name} changed"
            );
        }
    }
    let mut expected_assets = before_assets.clone();
    expected_assets.insert(mpq_path.clone(), ogg_hash.clone());
    assert_eq!(extra_assets(&output), expected_assets);
    assert_eq!(fs::read(&input).unwrap(), before_bytes);

    let reused_output = temp_map("reuse");
    let output_hash = file_hash(&output);
    let reused =
        isom::map_sound_add(&output, &reused_output, &output_hash, &mpq_path, OGG).unwrap();
    assert!(reused.reused);
    assert_eq!(reused.sound_index, report.sound_index);
    assert_eq!(reused.sound_string_id, report.sound_string_id);
    assert_eq!(
        fs::read(&reused_output).unwrap(),
        fs::read(&output).unwrap()
    );
    sound_path_and_slot(&reused_output, &mpq_path);

    fs::remove_file(output).ok();
    fs::remove_file(reused_output).ok();
}

#[test]
fn sound_conflicts_and_invalid_inputs_leave_real_scx_unchanged() {
    let input = fixture();
    let before = fs::read(&input).unwrap();
    let before_hash = file_hash(&input);
    let ogg_hash = format!("{:x}", Sha256::digest(OGG));
    let mpq_path = format!("staredit\\wav\\ea_{}.ogg", &ogg_hash[..16]);
    let output = temp_map("conflict-base");
    isom::map_sound_add(&input, &output, &before_hash, &mpq_path, OGG).unwrap();
    let output_before = fs::read(&output).unwrap();
    let output_hash = file_hash(&output);

    let mut other_ogg = OGG.to_vec();
    *other_ogg.last_mut().unwrap() ^= 1;
    let conflict_output = temp_map("different-bytes");
    assert!(isom::map_sound_add(
        &output,
        &conflict_output,
        &output_hash,
        &mpq_path,
        &other_ogg,
    )
    .unwrap_err()
    .to_string()
    .contains("different bytes"));
    assert!(!conflict_output.exists());
    assert_eq!(fs::read(&output).unwrap(), output_before);

    let invalid_output = temp_map("invalid");
    assert!(isom::map_sound_add(
        &input,
        &invalid_output,
        &"0".repeat(64),
        "staredit\\wav\\not-managed.ogg",
        b"not ogg",
    )
    .is_err());
    assert!(!invalid_output.exists());
    assert_eq!(fs::read(&input).unwrap(), before);

    fs::remove_file(output).ok();
}

#[cfg(windows)]
#[test]
fn real_scx_enforces_last_and_exhausted_wav_slot_boundaries() {
    let ogg_hash = format!("{:x}", Sha256::digest(OGG));
    let mpq_path = format!("staredit\\wav\\ea_{}.ogg", &ogg_hash[..16]);

    let used_511 = map_with_used_wav_slots(511, "slots-511");
    let before_511 = fs::read(&used_511).unwrap();
    let output = temp_map("slots-511-output");
    let report =
        isom::map_sound_add(&used_511, &output, &file_hash(&used_511), &mpq_path, OGG).unwrap();
    assert_eq!(report.sound_index, 511);
    assert_eq!(fs::read(&used_511).unwrap(), before_511);

    let used_512 = map_with_used_wav_slots(512, "slots-512");
    let before = fs::read(&used_512).unwrap();
    let rejected = temp_map("slots-512-output");
    let error = isom::map_sound_add(&used_512, &rejected, &file_hash(&used_512), &mpq_path, OGG)
        .unwrap_err();
    assert!(error.to_string().contains("512 WAV"));
    assert!(!rejected.exists());
    assert_eq!(fs::read(&used_512).unwrap(), before);

    fs::remove_file(used_511).ok();
    fs::remove_file(output).ok();
    fs::remove_file(used_512).ok();
}

#[cfg(windows)]
#[test]
fn real_scx_rejects_mpq_string_and_wav_partial_states() {
    let ogg_hash = format!("{:x}", Sha256::digest(OGG));
    let mpq_path = format!("staredit\\wav\\ea_{}.ogg", &ogg_hash[..16]);
    let fixture_ogg = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("tone.ogg");

    let mpq_only = temp_map("partial-mpq");
    fs::copy(fixture(), &mpq_only).unwrap();
    add_archive_file(&mpq_only, &fixture_ogg, &mpq_path);
    let mpq_only_before = fs::read(&mpq_only).unwrap();
    let mpq_rejected = temp_map("partial-mpq-output");
    assert!(isom::map_sound_add(
        &mpq_only,
        &mpq_rejected,
        &file_hash(&mpq_only),
        &mpq_path,
        OGG,
    )
    .unwrap_err()
    .to_string()
    .contains("partial state"));
    assert_eq!(fs::read(&mpq_only).unwrap(), mpq_only_before);

    let complete = temp_map("partial-complete");
    isom::map_sound_add(
        &fixture(),
        &complete,
        &file_hash(&fixture()),
        &mpq_path,
        OGG,
    )
    .unwrap();

    let wav_only = temp_map("partial-wav");
    fs::copy(&complete, &wav_only).unwrap();
    remove_archive_file(&wav_only, &mpq_path);
    let wav_rejected = temp_map("partial-wav-output");
    assert!(isom::map_sound_add(
        &wav_only,
        &wav_rejected,
        &file_hash(&wav_only),
        &mpq_path,
        OGG,
    )
    .unwrap_err()
    .to_string()
    .contains("partial state"));

    let string_only = temp_map("partial-string");
    fs::copy(&complete, &string_only).unwrap();
    remove_archive_file(&string_only, &mpq_path);
    let chk = isom::chk_extract(&string_only).unwrap();
    let no_wav = chk_with_used_wav_slots(&chk, 0);
    let chk_path = string_only.with_extension("scenario.chk");
    fs::write(&chk_path, no_wav).unwrap();
    add_archive_file(&string_only, &chk_path, "staredit\\scenario.chk");
    fs::remove_file(chk_path).ok();
    let string_rejected = temp_map("partial-string-output");
    assert!(isom::map_sound_add(
        &string_only,
        &string_rejected,
        &file_hash(&string_only),
        &mpq_path,
        OGG,
    )
    .unwrap_err()
    .to_string()
    .contains("partial state"));

    for path in [
        mpq_only,
        complete,
        wav_only,
        string_only,
        mpq_rejected,
        wav_rejected,
        string_rejected,
    ] {
        fs::remove_file(path).ok();
    }
}
