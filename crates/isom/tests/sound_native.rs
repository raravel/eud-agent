use std::collections::BTreeMap;
use std::ffi::c_void;
use std::fs;
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

/// A NUL-terminated path in StormLib's `TCHAR` encoding: UTF-16 on Windows,
/// UTF-8 elsewhere.
#[cfg(windows)]
type NativeChar = u16;
#[cfg(not(windows))]
type NativeChar = std::ffi::c_char;

#[cfg(windows)]
fn native_path(path: &Path) -> Vec<NativeChar> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(not(windows))]
fn native_path(path: &Path) -> Vec<NativeChar> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str()
        .as_bytes()
        .iter()
        .map(|byte| *byte as NativeChar)
        .chain(Some(0))
        .collect()
}

unsafe extern "system" {
    fn SFileOpenArchive(
        archive_name: *const NativeChar,
        priority: u32,
        flags: u32,
        archive: *mut *mut c_void,
    ) -> bool;
    fn SFileCloseArchive(archive: *mut c_void) -> bool;
    fn SFileAddFile(
        archive: *mut c_void,
        local_name: *const NativeChar,
        archived_name: *const std::ffi::c_char,
        flags: u32,
    ) -> bool;
    #[cfg(windows)]
    fn SFileRemoveFile(
        archive: *mut c_void,
        archived_name: *const std::ffi::c_char,
        search_scope: u32,
    ) -> bool;
}

fn edit_archive(map: &Path, operation: impl FnOnce(*mut c_void)) {
    let path = native_path(map);
    let mut archive = std::ptr::null_mut();
    // SAFETY: NUL-terminated path and valid out pointer; the handle is closed below.
    assert!(unsafe { SFileOpenArchive(path.as_ptr(), 0, 0, &mut archive) });
    operation(archive);
    // SAFETY: `archive` is the successful handle returned above.
    assert!(unsafe { SFileCloseArchive(archive) });
}

fn add_archive_file(map: &Path, local: &Path, archived: &str) {
    let local = native_path(local);
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

#[cfg(windows)]
#[test]
fn real_scx_adds_a_sound_through_an_extended_length_project_path() {
    // Given: a map addressed the way a canonicalized native project root is,
    // with the `\\?\` extended-length prefix on input and output alike.
    let plain = temp_map("verbatim-input");
    fs::copy(fixture(), &plain).unwrap();
    let verbatim = |path: &Path| PathBuf::from(format!(r"\\?\{}", path.display()));
    let input = verbatim(&plain);
    let output_plain = temp_map("verbatim-output");
    let output = verbatim(&output_plain);
    let ogg_hash = format!("{:x}", Sha256::digest(OGG));
    let mpq_path = format!("staredit\\wav\\ea_{}.ogg", &ogg_hash[..16]);

    // When: the sound is added.
    let report = isom::map_sound_add(&input, &output, &file_hash(&plain), &mpq_path, OGG);

    // Then: the asset, string, and WAV slot land exactly as with a plain path.
    let report = report.unwrap();
    assert!(!report.reused);
    let (string_id, sound_index) = sound_path_and_slot(&output_plain, &mpq_path);
    assert_eq!(report.sound_string_id, string_id as u64);
    assert_eq!(report.sound_index, sound_index as u64);
    assert_eq!(extra_assets(&output_plain).get(&mpq_path), Some(&ogg_hash));

    fs::remove_file(plain).ok();
    fs::remove_file(output_plain).ok();
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
fn real_scx_reads_one_named_asset_back_exactly_as_the_digest_hashes_it() {
    // Given: a real map carrying one added sound asset.
    let input = fixture();
    let ogg_hash = format!("{:x}", Sha256::digest(OGG));
    let mpq_path = format!("staredit\\wav\\ea_{}.ogg", &ogg_hash[..16]);
    let output = temp_map("asset-read");
    isom::map_sound_add(&input, &output, &file_hash(&input), &mpq_path, OGG).unwrap();

    // When: that asset is read back by its MPQ path.
    let bytes = isom::map_asset(&output, &mpq_path, OGG.len()).unwrap();

    // Then: the bytes are the ones stored, and their hash is the digest's.
    assert_eq!(bytes, OGG);
    assert_eq!(
        extra_assets(&output).get(&mpq_path),
        Some(&format!("{:x}", Sha256::digest(&bytes)))
    );
    // A missing asset, an asset over the limit, and the reserved scenario
    // entry are refused with a native reason instead of empty bytes.
    let missing = isom::map_asset(&output, "staredit\\wav\\missing.ogg", OGG.len()).unwrap_err();
    assert!(missing.to_string().contains("no MPQ asset"), "{missing}");
    let oversized = isom::map_asset(&output, &mpq_path, OGG.len() - 1).unwrap_err();
    assert!(oversized.to_string().contains("size limit"), "{oversized}");
    let reserved =
        isom::map_asset(&output, "staredit\\scenario.chk", 64 * 1024 * 1024).unwrap_err();
    assert!(reserved.to_string().contains("reserved"), "{reserved}");
    assert!(isom::map_asset(&output, &mpq_path, 0).is_err());

    fs::remove_file(output).ok();
}

#[test]
fn real_scx_replaces_managed_sound_without_leaving_the_old_registration() {
    let input = fixture();
    let old_hash = format!("{:x}", Sha256::digest(OGG));
    let old_path = format!("staredit\\wav\\ea_{}.ogg", &old_hash[..16]);
    let added = temp_map("replace-base");
    let added_report =
        isom::map_sound_add(&input, &added, &file_hash(&input), &old_path, OGG).unwrap();
    let added_before = fs::read(&added).unwrap();

    let mut edited_ogg = OGG.to_vec();
    *edited_ogg.last_mut().unwrap() ^= 1;
    let edited_hash = format!("{:x}", Sha256::digest(&edited_ogg));
    let edited_path = format!("staredit\\wav\\ea_{}.ogg", &edited_hash[..16]);
    let replaced = temp_map("replace-output");
    let report = isom::map_sound_replace(
        &added,
        &replaced,
        &file_hash(&added),
        &old_path,
        &edited_path,
        &edited_ogg,
    )
    .unwrap();

    assert_eq!(report.sound_index, added_report.sound_index);
    assert_eq!(report.sound_string_id, added_report.sound_string_id);
    assert_eq!(report.old_mpq_path, old_path);
    assert_eq!(report.mpq_path, edited_path);
    assert_eq!(report.asset_sha256, edited_hash);
    assert_eq!(fs::read(&added).unwrap(), added_before);
    let assets = extra_assets(&replaced);
    assert!(!assets.contains_key(&old_path));
    assert_eq!(assets.get(&edited_path), Some(&edited_hash));
    assert_eq!(
        sound_path_and_slot(&replaced, &edited_path),
        (
            added_report.sound_string_id as usize,
            added_report.sound_index as usize,
        )
    );

    fs::remove_file(added).ok();
    fs::remove_file(replaced).ok();
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

/// `count` distinct OGG byte strings (the native side checks only the OggS
/// magic and hashes the bytes) with their content-addressed managed paths.
fn distinct_oggs(count: usize) -> Vec<(String, Vec<u8>)> {
    (0..count)
        .map(|index| {
            let mut ogg = OGG.to_vec();
            ogg.extend_from_slice(format!("piece{index:03}").as_bytes());
            let hash = format!("{:x}", Sha256::digest(&ogg));
            (format!("staredit\\wav\\ea_{}.ogg", &hash[..16]), ogg)
        })
        .collect()
}

fn batch_items(oggs: &[(String, Vec<u8>)]) -> Vec<(&str, &[u8])> {
    oggs.iter()
        .map(|(path, ogg)| (path.as_str(), ogg.as_slice()))
        .collect()
}

#[test]
fn real_scx_batch_adds_every_sound_in_one_save_and_reports_input_order() {
    // Given: a real map and three new sounds plus one already registered.
    let input = fixture();
    let existing_hash = format!("{:x}", Sha256::digest(OGG));
    let existing_path = format!("staredit\\wav\\ea_{}.ogg", &existing_hash[..16]);
    let with_one = temp_map("batch-base");
    isom::map_sound_add(&input, &with_one, &file_hash(&input), &existing_path, OGG).unwrap();
    let before_bytes = fs::read(&with_one).unwrap();
    let before_sections = sections(&isom::chk_extract(&with_one).unwrap());
    let before_assets = extra_assets(&with_one);
    let fresh = distinct_oggs(3);
    let mut items = batch_items(&fresh);
    items.insert(1, (existing_path.as_str(), OGG));
    let output = temp_map("batch-output");

    // When: the batch is added in one native call.
    let report =
        isom::map_sound_add_batch(&with_one, &output, &file_hash(&with_one), &items).unwrap();

    // Then: the report follows input order, the registered sound is reused,
    // each new sound owns one new slot and string, and nothing else changed.
    assert_eq!(
        report
            .sounds
            .iter()
            .map(|sound| (sound.mpq_path.as_str(), sound.reused))
            .collect::<Vec<_>>(),
        items
            .iter()
            .map(|(path, _)| (*path, *path == existing_path))
            .collect::<Vec<_>>()
    );
    let mut expected_assets = before_assets.clone();
    for (path, ogg) in &fresh {
        expected_assets.insert(path.clone(), format!("{:x}", Sha256::digest(ogg)));
        let (string_id, sound_index) = sound_path_and_slot(&output, path);
        let sound = report
            .sounds
            .iter()
            .find(|sound| &sound.mpq_path == path)
            .unwrap();
        assert_eq!(sound.sound_index, sound_index as u64);
        assert_eq!(sound.sound_string_id, string_id as u64);
        assert_eq!(isom::map_asset(&output, path, ogg.len()).unwrap(), *ogg);
    }
    assert_eq!(extra_assets(&output), expected_assets);
    sound_path_and_slot(&output, &existing_path);
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
    assert_eq!(report.output_sha256, file_hash(&output));
    assert_eq!(fs::read(&with_one).unwrap(), before_bytes);

    // And: a batch of only registered sounds is an exact idempotent copy.
    let reused_output = temp_map("batch-reused");
    let reused = isom::map_sound_add_batch(
        &output,
        &reused_output,
        &file_hash(&output),
        &batch_items(&fresh),
    )
    .unwrap();
    assert!(reused.sounds.iter().all(|sound| sound.reused));
    assert_eq!(
        fs::read(&reused_output).unwrap(),
        fs::read(&output).unwrap()
    );

    fs::remove_file(with_one).ok();
    fs::remove_file(output).ok();
    fs::remove_file(reused_output).ok();
}

#[test]
fn real_scx_batch_is_all_or_nothing_on_one_bad_item() {
    // Given: a map with one registered sound and a batch whose last item
    // names that path with different bytes.
    let input = fixture();
    let existing_hash = format!("{:x}", Sha256::digest(OGG));
    let existing_path = format!("staredit\\wav\\ea_{}.ogg", &existing_hash[..16]);
    let with_one = temp_map("batch-conflict-base");
    isom::map_sound_add(&input, &with_one, &file_hash(&input), &existing_path, OGG).unwrap();
    let before = fs::read(&with_one).unwrap();
    let fresh = distinct_oggs(2);
    let mut other = OGG.to_vec();
    *other.last_mut().unwrap() ^= 1;
    let mut items = batch_items(&fresh);
    items.push((existing_path.as_str(), other.as_slice()));
    let output = temp_map("batch-conflict-output");

    // When / Then: the whole batch is refused, no output exists, the input is
    // untouched, and a repeated destination is refused the same way.
    let error =
        isom::map_sound_add_batch(&with_one, &output, &file_hash(&with_one), &items).unwrap_err();
    assert!(error.to_string().contains("different bytes"), "{error}");
    assert!(!output.exists());
    assert_eq!(fs::read(&with_one).unwrap(), before);
    let repeated = [items[0], items[0]];
    assert!(
        isom::map_sound_add_batch(&with_one, &output, &file_hash(&with_one), &repeated).is_err()
    );
    assert!(!output.exists());
    assert_eq!(fs::read(&with_one).unwrap(), before);

    fs::remove_file(with_one).ok();
}

#[cfg(windows)]
#[test]
fn real_scx_batch_refuses_more_sounds_than_free_wav_slots_before_any_write() {
    let used = map_with_used_wav_slots(510, "batch-slots");
    let before = fs::read(&used).unwrap();
    let fresh = distinct_oggs(3);
    let output = temp_map("batch-slots-output");
    let error = isom::map_sound_add_batch(&used, &output, &file_hash(&used), &batch_items(&fresh))
        .unwrap_err();
    assert!(error.to_string().contains("512 WAV"), "{error}");
    assert!(!output.exists());
    assert_eq!(fs::read(&used).unwrap(), before);
    fs::remove_file(used).ok();
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

#[test]
fn real_scx_removes_sounds_leaving_every_other_slot_string_and_asset() {
    // Given: a real map carrying three added sounds.
    let input = fixture();
    let oggs = distinct_oggs(3);
    let added = temp_map("remove-base");
    let added_report =
        isom::map_sound_add_batch(&input, &added, &file_hash(&input), &batch_items(&oggs)).unwrap();
    let added_before = fs::read(&added).unwrap();
    let added_sections = sections(&isom::chk_extract(&added).unwrap());
    let added_strings = strings(&added_sections);

    // When: the first and last are removed in one call.
    let removed = temp_map("remove-output");
    let indexes = [
        added_report.sounds[2].sound_index as u16,
        added_report.sounds[0].sound_index as u16,
    ];
    let report = isom::map_sound_remove(&added, &removed, &file_hash(&added), &indexes).unwrap();

    // Then: the report follows input order and names each removed asset.
    assert_eq!(report.sounds.len(), 2);
    for (sound, index) in report.sounds.iter().zip([2, 0]) {
        assert_eq!(sound.sound_index, added_report.sounds[index].sound_index);
        assert_eq!(
            sound.sound_string_id,
            added_report.sounds[index].sound_string_id
        );
        assert_eq!(sound.asset_sha256, added_report.sounds[index].asset_sha256);
    }
    assert_eq!(fs::read(&added).unwrap(), added_before);
    // Only the kept sound's asset remains beside the map's original assets.
    let mut expected_assets = extra_assets(&input);
    expected_assets.insert(
        oggs[1].0.clone(),
        added_report.sounds[1].asset_sha256.clone(),
    );
    assert_eq!(extra_assets(&removed), expected_assets);
    // The removed slots are empty, the kept one is untouched.
    let after_sections = sections(&isom::chk_extract(&removed).unwrap());
    let slots = after_sections["WAV "]
        .chunks_exact(4)
        .map(|slot| u32::from_le_bytes(slot.try_into().unwrap()) as u64)
        .collect::<Vec<_>>();
    for sound in [&added_report.sounds[0], &added_report.sounds[2]] {
        assert_eq!(slots[sound.sound_index as usize], 0);
    }
    assert_eq!(
        sound_path_and_slot(&removed, &oggs[1].0),
        (
            added_report.sounds[1].sound_string_id as usize,
            added_report.sounds[1].sound_index as usize,
        )
    );
    // Every other game string keeps its id and bytes; the removed ones are gone.
    let after_strings = strings(&after_sections);
    for (index, before) in added_strings.iter().enumerate() {
        let id = index as u64 + 1;
        let after = after_strings.get(index).cloned().unwrap_or_default();
        if id == added_report.sounds[0].sound_string_id
            || id == added_report.sounds[2].sound_string_id
        {
            assert!(after.is_empty(), "removed string {id} survived");
        } else {
            assert_eq!(&after, before, "string {id} changed");
        }
    }
    for (name, body) in &added_sections {
        if !matches!(name.as_str(), "STR " | "STRx" | "WAV ") {
            assert_eq!(
                after_sections.get(name),
                Some(body),
                "section {name} changed"
            );
        }
    }

    fs::remove_file(added).ok();
    fs::remove_file(removed).ok();
}

#[test]
fn real_scx_sound_removal_refuses_bad_slots_before_any_write() {
    // Given: a real map with one added sound.
    let input = fixture();
    let oggs = distinct_oggs(1);
    let added = temp_map("remove-refuse-base");
    let added_report =
        isom::map_sound_add_batch(&input, &added, &file_hash(&input), &batch_items(&oggs)).unwrap();
    let before = fs::read(&added).unwrap();
    let hash = file_hash(&added);
    let registered = added_report.sounds[0].sound_index as u16;
    let empty = (0..512_u16)
        .find(|index| {
            let slots = sections(&isom::chk_extract(&added).unwrap())["WAV "].clone();
            slots[*index as usize * 4..*index as usize * 4 + 4] == [0, 0, 0, 0]
        })
        .unwrap();

    // When/Then: an empty slot, a repeated slot, an out-of-range slot, and a
    // stale hash are refused, and nothing is written.
    let output = temp_map("remove-refuse-output");
    let unregistered =
        isom::map_sound_remove(&added, &output, &hash, &[registered, empty]).unwrap_err();
    assert!(
        unregistered.to_string().contains("is not registered"),
        "{unregistered}"
    );
    assert!(isom::map_sound_remove(&added, &output, &hash, &[registered, registered]).is_err());
    assert!(isom::map_sound_remove(&added, &output, &hash, &[512]).is_err());
    assert!(isom::map_sound_remove(&added, &output, &hash, &[]).is_err());
    let stale =
        isom::map_sound_remove(&added, &output, &"0".repeat(64), &[registered]).unwrap_err();
    assert!(stale.to_string().contains("stale"), "{stale}");
    assert!(!output.exists());
    assert_eq!(fs::read(&added).unwrap(), before);

    fs::remove_file(added).ok();
}

/// `chk` with its TRIG section replaced (or added) by one trigger whose first
/// action plays the WAV game string `sound_string_id` for all players.
fn chk_with_play_wav_trigger(chk: &[u8], sound_string_id: u32) -> Vec<u8> {
    const PLAY_WAV: u8 = 8;
    const ALL_PLAYERS: usize = 17;
    let mut trigger = vec![0u8; 2400];
    let action = 16 * 20;
    trigger[action + 8..action + 12].copy_from_slice(&sound_string_id.to_le_bytes());
    trigger[action + 26] = PLAY_WAV;
    trigger[16 * 20 + 64 * 32 + 4 + ALL_PLAYERS] = 1;
    let mut result = Vec::new();
    let mut offset = 0usize;
    while offset + 8 <= chk.len() {
        let size = i32::from_le_bytes(chk[offset + 4..offset + 8].try_into().unwrap());
        let end = offset + 8 + size as usize;
        if &chk[offset..offset + 4] != b"TRIG" {
            result.extend_from_slice(&chk[offset..end]);
        }
        offset = end;
    }
    result.extend_from_slice(b"TRIG");
    result.extend_from_slice(&(trigger.len() as i32).to_le_bytes());
    result.extend_from_slice(&trigger);
    result
}

#[test]
fn real_scx_sound_removal_refuses_a_sound_a_trigger_still_plays() {
    // Given: a real map whose added sound is played by a Play WAV trigger.
    let input = fixture();
    let oggs = distinct_oggs(1);
    let map = temp_map("remove-trigger");
    let added =
        isom::map_sound_add_batch(&input, &map, &file_hash(&input), &batch_items(&oggs)).unwrap();
    let chk = chk_with_play_wav_trigger(
        &isom::chk_extract(&map).unwrap(),
        added.sounds[0].sound_string_id as u32,
    );
    let chk_path = map.with_extension("scenario.chk");
    fs::write(&chk_path, chk).unwrap();
    add_archive_file(&map, &chk_path, "staredit\\scenario.chk");
    fs::remove_file(chk_path).ok();
    let before = fs::read(&map).unwrap();

    // When: that sound is removed.
    let output = temp_map("remove-trigger-output");
    let error = isom::map_sound_remove(
        &map,
        &output,
        &file_hash(&map),
        &[added.sounds[0].sound_index as u16],
    )
    .unwrap_err();

    // Then: the trigger's reference refuses it and nothing is written.
    assert!(
        error.to_string().contains("still used by a trigger"),
        "{error}"
    );
    assert!(!output.exists());
    assert_eq!(fs::read(&map).unwrap(), before);

    fs::remove_file(map).ok();
}
