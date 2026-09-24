#![windows_subsystem = "windows"]

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::time::{Duration, Instant};

const PROCESS_ACCESS: u32 = 0x0008 | 0x0010 | 0x0020 | 0x0400;
const THREAD_SUSPEND_RESUME: u32 = 0x0002;
const TH32CS_SNAPTHREAD: u32 = 0x0000_0004;
const TH32CS_SNAPMODULE: u32 = 0x0000_0008;
const TH32CS_SNAPMODULE32: u32 = 0x0000_0010;
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const PROCESS_BASIC_INFORMATION_CLASS: u32 = 0;
const PEB_IMAGE_BASE_OFFSET: usize = 0x08;
const MAX_IMAGE_BYTES: usize = 256 * 1024 * 1024;
const IMAGE_READ_CHUNK: usize = 64 * 1024;
const MAX_INSTANCE_NAME_MATCHES: usize = 4;
/// SC:R's single-instance kernel object name (UTF-16 in the client image). The
/// trailing `Instances` word is rewritten per test PID so an isolated client
/// never observes, or is observed by, the user's own running game.
const INSTANCE_OBJECT_NAME: &str = "Starcraft Check For Other Instances";
const INSTANCE_OBJECT_SUFFIX: &str = "Instances";

type Handle = *mut c_void;

#[repr(C)]
struct ModuleEntry32W {
    size: u32,
    module_id: u32,
    pid: u32,
    global_usage: u32,
    process_usage: u32,
    base: *mut u8,
    base_size: u32,
    module: Handle,
    module_name: [u16; 256],
    image_path: [u16; 260],
}

#[repr(C)]
struct ThreadEntry32 {
    size: u32,
    usage: u32,
    thread_id: u32,
    owner_pid: u32,
    base_priority: i32,
    priority_delta: i32,
    flags: u32,
}

#[repr(C)]
struct ProcessBasicInformation {
    exit_status: i32,
    peb_base: *mut c_void,
    affinity_mask: usize,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}

type NtQueryInformationProcessFn = unsafe extern "system" fn(
    process: Handle,
    class: u32,
    information: *mut c_void,
    length: u32,
    returned: *mut u32,
) -> i32;

#[link(name = "kernel32")]
extern "system" {
    fn CloseHandle(handle: Handle) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn FlushInstructionCache(process: Handle, address: *const c_void, size: usize) -> i32;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn LoadLibraryW(path: *const u16) -> Handle;
    fn Module32FirstW(snapshot: Handle, entry: *mut ModuleEntry32W) -> i32;
    fn Module32NextW(snapshot: Handle, entry: *mut ModuleEntry32W) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn OpenThread(access: u32, inherit: i32, thread_id: u32) -> Handle;
    fn QueryFullProcessImageNameW(
        process: Handle,
        flags: u32,
        path: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn ReadProcessMemory(
        process: Handle,
        address: *const c_void,
        buffer: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
    fn ResumeThread(thread: Handle) -> u32;
    fn SuspendThread(thread: Handle) -> u32;
    fn Thread32First(snapshot: Handle, entry: *mut ThreadEntry32) -> i32;
    fn Thread32Next(snapshot: Handle, entry: *mut ThreadEntry32) -> i32;
    fn VirtualProtectEx(
        process: Handle,
        address: *mut c_void,
        size: usize,
        protection: u32,
        old_protection: *mut u32,
    ) -> i32;
    fn WriteProcessMemory(
        process: Handle,
        address: *mut c_void,
        buffer: *const c_void,
        size: usize,
        written: *mut usize,
    ) -> i32;
}

struct HandleGuard(Handle);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn wide_array(value: &[u16]) -> String {
    let end = value
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}

fn primary_thread(pid: u32) -> Result<HandleGuard, u8> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(6);
    }
    let snapshot = HandleGuard(snapshot);
    let mut entry: ThreadEntry32 = unsafe { std::mem::zeroed() };
    entry.size = std::mem::size_of::<ThreadEntry32>() as u32;
    let mut ok = unsafe { Thread32First(snapshot.0, &mut entry) };
    while ok != 0 {
        if entry.owner_pid == pid {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.thread_id) };
            if thread.is_null() {
                return Err(6);
            }
            return Ok(HandleGuard(thread));
        }
        ok = unsafe { Thread32Next(snapshot.0, &mut entry) };
    }
    Err(6)
}

fn find_remote_user32(pid: u32) -> Option<usize> {
    let snapshot =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let snapshot = HandleGuard(snapshot);
    let mut entry: ModuleEntry32W = unsafe { std::mem::zeroed() };
    entry.size = std::mem::size_of::<ModuleEntry32W>() as u32;
    let mut ok = unsafe { Module32FirstW(snapshot.0, &mut entry) };
    while ok != 0 {
        if wide_array(&entry.module_name).eq_ignore_ascii_case("user32.dll") {
            return Some(entry.base as usize);
        }
        ok = unsafe { Module32NextW(snapshot.0, &mut entry) };
    }
    None
}

fn wait_remote_user32(pid: u32) -> Result<usize, u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(base) = find_remote_user32(pid) {
            return Ok(base);
        }
        if Instant::now() >= deadline {
            return Err(7);
        }
        std::thread::yield_now();
    }
}

fn write_remote(process: Handle, remote: *mut c_void, replacement: &[u8]) -> Result<(), ()> {
    let mut old_protection = 0;
    if unsafe {
        VirtualProtectEx(
            process,
            remote,
            replacement.len(),
            PAGE_EXECUTE_READWRITE,
            &mut old_protection,
        )
    } == 0
    {
        return Err(());
    }
    let mut written = 0;
    let wrote = unsafe {
        WriteProcessMemory(
            process,
            remote,
            replacement.as_ptr().cast(),
            replacement.len(),
            &mut written,
        )
    };
    let mut ignored = 0;
    unsafe {
        VirtualProtectEx(
            process,
            remote,
            replacement.len(),
            old_protection,
            &mut ignored,
        );
        FlushInstructionCache(process, remote, replacement.len());
    }
    if wrote == 0 || written != replacement.len() {
        Err(())
    } else {
        Ok(())
    }
}

fn patch_api(
    process: Handle,
    remote_user32: usize,
    local_user32: usize,
    name: &[u8],
    replacement: &[u8],
) -> Result<(), ()> {
    let local = unsafe { GetProcAddress(local_user32 as Handle, name.as_ptr()) };
    if local.is_null() {
        return Err(());
    }
    let remote = (remote_user32 + local as usize - local_user32) as *mut c_void;
    write_remote(process, remote, replacement)
}

fn read_remote(process: Handle, address: usize, buffer: &mut [u8]) -> Result<(), ()> {
    let mut read = 0;
    let ok = unsafe {
        ReadProcessMemory(
            process,
            address as *const c_void,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut read,
        )
    };
    if ok == 0 || read != buffer.len() {
        Err(())
    } else {
        Ok(())
    }
}

fn read_remote_u32(process: Handle, address: usize) -> Result<u32, ()> {
    let mut bytes = [0u8; 4];
    read_remote(process, address, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

/// Locate the suspended client's main image through its PEB. Toolhelp module
/// snapshots are not populated before the loader runs, but the image itself is
/// already mapped at process creation.
fn main_image(process: Handle) -> Result<(usize, usize), u8> {
    let ntdll_name = wide(OsStr::new("ntdll.dll"));
    let ntdll = unsafe { GetModuleHandleW(ntdll_name.as_ptr()) };
    if ntdll.is_null() {
        return Err(16);
    }
    let query = unsafe { GetProcAddress(ntdll, b"NtQueryInformationProcess\0".as_ptr()) };
    if query.is_null() {
        return Err(16);
    }
    let query: NtQueryInformationProcessFn = unsafe { std::mem::transmute(query) };
    let mut information: ProcessBasicInformation = unsafe { std::mem::zeroed() };
    let mut returned = 0u32;
    let status = unsafe {
        query(
            process,
            PROCESS_BASIC_INFORMATION_CLASS,
            (&mut information as *mut ProcessBasicInformation).cast(),
            std::mem::size_of::<ProcessBasicInformation>() as u32,
            &mut returned,
        )
    };
    if status < 0 || information.peb_base.is_null() {
        return Err(16);
    }
    let base = read_remote_u32(
        process,
        information.peb_base as usize + PEB_IMAGE_BASE_OFFSET,
    )
    .map_err(|_| 16)? as usize;
    if base == 0 {
        return Err(16);
    }
    let mut dos = [0u8; 0x40];
    read_remote(process, base, &mut dos).map_err(|_| 16)?;
    if dos[..2] != *b"MZ" {
        return Err(16);
    }
    let nt = u32::from_le_bytes([dos[0x3c], dos[0x3d], dos[0x3e], dos[0x3f]]) as usize;
    if !(0x40..=0x1000).contains(&nt) {
        return Err(16);
    }
    let signature = read_remote_u32(process, base + nt).map_err(|_| 16)?;
    if signature != 0x0000_4550 {
        return Err(16);
    }
    let size = read_remote_u32(process, base + nt + 0x50).map_err(|_| 16)? as usize;
    if size == 0 || size > MAX_IMAGE_BYTES {
        return Err(16);
    }
    Ok((base, size))
}

fn find_all(haystack: &[u8], needle: &[u8], limit: usize) -> Vec<usize> {
    let mut matches = Vec::new();
    let mut offset = 0;
    while offset + needle.len() <= haystack.len() && matches.len() < limit {
        match haystack[offset..]
            .windows(needle.len())
            .position(|window| window == needle)
        {
            Some(found) => {
                matches.push(offset + found);
                offset += found + needle.len();
            }
            None => break,
        }
    }
    matches
}

/// Rewrite every copy of the single-instance object name inside the mapped
/// client image before the primary thread ever runs. The name keeps its length
/// and prefix; only the final word becomes `E` plus the eight-digit hex PID.
fn isolate_instance_object(process: Handle, pid: u32) -> Result<(), u8> {
    let (base, size) = main_image(process)?;
    let mut image = vec![0u8; size];
    for offset in (0..size).step_by(IMAGE_READ_CHUNK) {
        let end = (offset + IMAGE_READ_CHUNK).min(size);
        // Unreadable pages stay zero; the name lives in an ordinary data section.
        let _ = read_remote(process, base + offset, &mut image[offset..end]);
    }
    let mut needle = INSTANCE_OBJECT_NAME
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    needle.extend_from_slice(&[0, 0]);
    let matches = find_all(&image, &needle, MAX_INSTANCE_NAME_MATCHES);
    if matches.is_empty() {
        return Err(15);
    }
    let suffix = format!("E{pid:08X}");
    if suffix.encode_utf16().count() != INSTANCE_OBJECT_SUFFIX.len() {
        return Err(17);
    }
    let replacement = suffix
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let suffix_offset = (INSTANCE_OBJECT_NAME.len() - INSTANCE_OBJECT_SUFFIX.len()) * 2;
    for offset in matches {
        let remote = (base + offset + suffix_offset) as *mut c_void;
        write_remote(process, remote, &replacement).map_err(|_| 17)?;
    }
    Ok(())
}

fn install_isolation(process: Handle, remote_user32: usize) -> Result<(), u8> {
    let user32_name = wide(OsStr::new("user32.dll"));
    let local_user32 = unsafe { LoadLibraryW(user32_name.as_ptr()) } as usize;
    if local_user32 == 0 {
        return Err(8);
    }
    let return_true_4 = [0xb8, 1, 0, 0, 0, 0xc2, 4, 0];
    let return_true_8 = [0xb8, 1, 0, 0, 0, 0xc2, 8, 0];
    let return_null_4 = [0x33, 0xc0, 0xc2, 4, 0];
    for (index, (name, replacement)) in [
        (
            b"SetForegroundWindow\0".as_slice(),
            return_true_4.as_slice(),
        ),
        (b"BringWindowToTop\0".as_slice(), return_true_4.as_slice()),
        (b"SetFocus\0".as_slice(), return_null_4.as_slice()),
        (b"SetCursorPos\0".as_slice(), return_true_8.as_slice()),
        (b"ClipCursor\0".as_slice(), return_true_4.as_slice()),
        (b"SwitchToThisWindow\0".as_slice(), return_true_8.as_slice()),
    ]
    .into_iter()
    .enumerate()
    {
        patch_api(process, remote_user32, local_user32, name, replacement)
            .map_err(|_| 9 + index as u8)?;
    }
    Ok(())
}

fn run() -> Result<(), u8> {
    let mut args = std::env::args_os().skip(1);
    let pid = args
        .next()
        .and_then(|value| value.to_string_lossy().parse::<u32>().ok())
        .ok_or(2)?;
    if args.next().is_some() {
        return Err(2);
    }
    let process = unsafe { OpenProcess(PROCESS_ACCESS, 0, pid) };
    if process.is_null() {
        return Err(3);
    }
    let process = HandleGuard(process);
    let mut image = vec![0u16; 32_768];
    let mut image_len = image.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process.0, 0, image.as_mut_ptr(), &mut image_len) } == 0
    {
        return Err(4);
    }
    let image = String::from_utf16_lossy(&image[..image_len as usize]);
    if !image.to_ascii_lowercase().ends_with("\\starcraft.exe") {
        return Err(5);
    }
    isolate_instance_object(process.0, pid)?;
    let thread = primary_thread(pid)?;
    if unsafe { ResumeThread(thread.0) } == u32::MAX {
        return Err(6);
    }
    let remote_user32 = match wait_remote_user32(pid) {
        Ok(base) => base,
        Err(error) => {
            unsafe {
                SuspendThread(thread.0);
            }
            return Err(error);
        }
    };
    if unsafe { SuspendThread(thread.0) } == u32::MAX {
        return Err(6);
    }
    install_isolation(process.0, remote_user32)?;
    Ok(())
}

fn main() {
    if let Err(code) = run() {
        std::process::exit(code as i32);
    }
}
