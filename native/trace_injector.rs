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

#[link(name = "kernel32")]
extern "system" {
    fn CloseHandle(handle: Handle) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn FlushInstructionCache(process: Handle, address: *const c_void, size: usize) -> i32;
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
