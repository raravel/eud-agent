use std::ffi::{c_void, CString};
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

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
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn section_mut<'a>(chk: &'a mut [u8], name: &[u8; 4]) -> &'a mut [u8] {
    let mut offset = 0;
    while offset + 8 <= chk.len() {
        let size = u32::from_le_bytes(chk[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let start = offset + 8;
        let end = start + size;
        assert!(end <= chk.len(), "CHK section exceeds fixture bounds");
        if &chk[offset..offset + 4] == name {
            return &mut chk[start..end];
        }
        offset = end;
    }
    panic!("fixture has no {:?} section", name);
}

pub(super) fn rewrite_owned_forces(map: &Path, owners: [u8; 8], forces: [u8; 8]) {
    let mut chk = isom::chk_extract(map).expect("copied map extracts CHK");
    section_mut(&mut chk, b"OWNR")[..8].copy_from_slice(&owners);
    section_mut(&mut chk, b"FORC")[..8].copy_from_slice(&forces);
    let scratch = map.with_extension("force-scenario.chk");
    fs::write(&scratch, chk).expect("write owned CHK scratch");
    let map_path = wide(map);
    let scratch_path = wide(&scratch);
    let archived = CString::new("staredit\\scenario.chk").unwrap();
    let mut archive = std::ptr::null_mut();
    assert!(unsafe { SFileOpenArchive(map_path.as_ptr(), 0, 0, &mut archive) });
    assert!(unsafe {
        SFileAddFile(
            archive,
            scratch_path.as_ptr(),
            archived.as_ptr(),
            0x8000_0200,
        )
    });
    assert!(unsafe { SFileCloseArchive(archive) });
    fs::remove_file(scratch).expect("remove owned CHK scratch");
}
