//! Safe wrapper over the raw [`isom_sys`] FFI (the C ABI shim over the vendored
//! isom-poc map engine).
//!
//! The whole public surface is safe: paths are turned into NUL-terminated
//! [`CString`]s, the C `IsomStatus` codes are mapped to the typed [`IsomError`]
//! enum, and the one heap buffer the C side returns ([`chk_extract`]) is copied
//! into an owned [`Vec<u8>`] and released through `isom_free` on EVERY exit path
//! (RAII guard) — no leak, no double-free.
//!
//! The map-write SAFETY RAILS (backup, lock probe, compiling guard, journal /
//! rollback) are NOT here — they live in the separate `mapsafe` layer
//! (rules.md). This crate is the thin, leak-free FFI translation only.
//!
//! Map-text bytes inside the `ops` buffers for [`locedit`], [`playeredit`], and
//! [`switchedit`] are passed through to C RAW — never re-encoded in Rust.

use sha2::{Digest as _, Sha256};
use std::ffi::{CString, NulError};
use std::os::raw::c_int;
use std::path::Path;

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;
#[cfg(windows)]
use std::path::{Component, Prefix};

static NATIVE_CALL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn native_call_guard() -> std::sync::MutexGuard<'static, ()> {
    NATIVE_CALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Typed errors mapped from the C `IsomStatus` codes (and the few Rust-side
/// failures that mean the call could never reach the engine).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IsomError {
    /// `ISOM_ERR_INVALID_ARG` (1): null pointer / empty path / bad length.
    /// Also raised Rust-side when the path cannot be made into a C string
    /// (embedded NUL) — it could never have reached the engine.
    #[error("invalid argument (null/empty path or bad length)")]
    InvalidArg,
    /// `ISOM_ERR_OPEN_MAP` (2): the map could not be opened or is empty.
    #[error("map could not be opened or is empty")]
    OpenMap,
    /// `ISOM_ERR_IO` (3): temp-file read/write or other I/O failure.
    #[error("map I/O failure")]
    Io,
    /// `ISOM_ERR_ENGINE` (4): the engine returned a nonzero op/save failure.
    #[error("engine returned a failure (bad op or save error)")]
    Engine,
    /// `ISOM_ERR_EXCEPTION` (5): a C++ exception was caught at the shim.
    #[error("a C++ exception was caught at the C ABI shim")]
    Exception,
    /// `ISOM_ERR_FAULT` (6): a structured (SEH) fault was caught at the shim.
    #[error("a structured (SEH) fault was caught at the C ABI shim")]
    Fault,
    /// A nonzero status the current ABI does not define.
    #[error("unknown isom status code {0}")]
    UnknownCode(i32),
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{status}{detail_suffix}")]
pub struct NativeCallError {
    pub status: IsomError,
    pub detail: Option<String>,
    detail_suffix: String,
}

impl NativeCallError {
    fn new(status: IsomError, detail: Option<String>) -> Self {
        let detail_suffix = detail
            .as_deref()
            .map(|value| format!(": {value}"))
            .unwrap_or_default();
        Self {
            status,
            detail,
            detail_suffix,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageQuantizeResult {
    pub width: u16,
    pub height: u16,
    pub tiles: Vec<u16>,
    pub preview_rgb: Vec<u8>,
    pub unique_tile_count: u32,
    pub walkability_changed_cells: u32,
    pub height_changed_cells: u32,
}

impl From<NulError> for IsomError {
    /// A path with an embedded NUL can never be a valid C string, so the engine
    /// would have rejected it as `ISOM_ERR_INVALID_ARG`; mirror that.
    fn from(_: NulError) -> Self {
        IsomError::InvalidArg
    }
}

/// Translate a C `IsomStatus` return code into `Ok(())` (on `ISOM_OK`) or the
/// matching typed error. The `isom_*` functions return `c_int`, and bindgen
/// emits the `IsomStatus::*` consts as `i32` (same width on the MSVC target),
/// so they pattern-match directly.
fn status(code: c_int) -> Result<(), IsomError> {
    use isom_sys::IsomStatus as S;
    match code {
        S::ISOM_OK => Ok(()),
        S::ISOM_ERR_INVALID_ARG => Err(IsomError::InvalidArg),
        S::ISOM_ERR_OPEN_MAP => Err(IsomError::OpenMap),
        S::ISOM_ERR_IO => Err(IsomError::Io),
        S::ISOM_ERR_ENGINE => Err(IsomError::Engine),
        S::ISOM_ERR_EXCEPTION => Err(IsomError::Exception),
        S::ISOM_ERR_FAULT => Err(IsomError::Fault),
        other => Err(IsomError::UnknownCode(other)),
    }
}

/// Build a NUL-terminated C string from a path's UTF-8 bytes. The C ABI takes a
/// `const char*` (UTF-8); on Windows `Path::to_str` yields the UTF-8 form.
fn path_cstring(map_path: &Path) -> Result<CString, IsomError> {
    let s = map_path.to_str().ok_or(IsomError::InvalidArg)?;
    if s.is_empty() {
        return Err(IsomError::InvalidArg);
    }
    let validated = CString::new(s)?;
    #[cfg(windows)]
    {
        drop(validated);
        CString::new(windows_native_path(map_path, s)?).map_err(IsomError::from)
    }
    #[cfg(not(windows))]
    Ok(validated)
}

#[cfg(windows)]
fn windows_native_path(path: &Path, utf8: &str) -> Result<String, IsomError> {
    let prefix = path.components().next();
    if matches!(
        prefix,
        Some(Component::Prefix(prefix))
            if matches!(
                prefix.kind(),
                Prefix::Verbatim(_)
                    | Prefix::VerbatimUNC(_, _)
                    | Prefix::VerbatimDisk(_)
                    | Prefix::DeviceNS(_)
            )
    ) {
        return Ok(utf8.to_owned());
    }

    let input = utf8
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut output = vec![0_u16; 260];
    loop {
        // SAFETY: [Category 8 — FFI boundary] `input` is NUL-terminated and remains
        // alive for the call. `output` is initialized writable memory whose element
        // count is passed exactly; the optional file-part pointer is intentionally null.
        let written = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetFullPathNameW(
                input.as_ptr(),
                u32::try_from(output.len()).map_err(|_| IsomError::InvalidArg)?,
                output.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if written == 0 {
            return Err(IsomError::InvalidArg);
        }
        let written = usize::try_from(written).map_err(|_| IsomError::InvalidArg)?;
        if written < output.len() {
            output.truncate(written);
            break;
        }
        if written <= output.len() {
            return Err(IsomError::InvalidArg);
        }
        output.resize(written, 0);
    }

    let normalized = OsString::from_wide(&output);
    let normalized_path = Path::new(&normalized);
    let normalized_utf8 = normalized_path.to_str().ok_or(IsomError::InvalidArg)?;
    if output.len() < 260 {
        return Ok(normalized_utf8.to_owned());
    }
    match normalized_path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(_) => Ok(format!(r"\\?\{normalized_utf8}")),
            Prefix::UNC(_, _) => normalized_utf8
                .strip_prefix(r"\\")
                .map(|suffix| format!(r"\\?\UNC\{suffix}"))
                .ok_or(IsomError::InvalidArg),
            Prefix::Verbatim(_)
            | Prefix::VerbatimUNC(_, _)
            | Prefix::VerbatimDisk(_)
            | Prefix::DeviceNS(_) => Ok(normalized_utf8.to_owned()),
        },
        _ => Err(IsomError::InvalidArg),
    }
}

/// RAII guard that frees a C-allocated `out` buffer via `isom_free` exactly once
/// on drop — so [`chk_extract`] never leaks regardless of which path it returns
/// through (success copy, panic, or any error). `isom_free` is documented safe
/// on NULL, so an untouched (still-null) pointer is fine to "free".
struct CBuf(*mut u8);

impl Drop for CBuf {
    fn drop(&mut self) {
        // SAFETY: `self.0` is either null or a pointer the matching
        // `isom_chk_extract` allocated; `isom_free` is the matching deallocator
        // and is explicitly NULL-safe. Drop runs once, so no double-free.
        unsafe { isom_sys::isom_free(self.0) };
    }
}

/// Extract the raw CHK (Remastered `.chk`) bytes from a map file.
///
/// On success the C-allocated buffer is copied into an owned `Vec<u8>` and then
/// freed via `isom_free`; on any error path the (NULL per the C contract)
/// buffer is still handed to `isom_free` by the [`CBuf`] guard — no leak, no
/// double-free.
pub fn chk_extract(map_path: &Path) -> Result<Vec<u8>, IsomError> {
    let _native_call = native_call_guard();
    let c_path = path_cstring(map_path)?;

    let mut out: *mut u8 = std::ptr::null_mut();
    let mut out_len: usize = 0;

    // SAFETY: `c_path` outlives the call; `out`/`out_len` are valid out-params.
    // The returned buffer is owned by us and released below via the guard.
    let code = unsafe { isom_sys::isom_chk_extract(c_path.as_ptr(), &mut out, &mut out_len) };

    // Take ownership of whatever `out` points at NOW (null on failure) so it is
    // freed on every subsequent return — including the `?` below.
    let buf = CBuf(out);

    status(code)?;

    // SAFETY: on ISOM_OK the C side guarantees `out` points to `out_len` valid
    // bytes (or is null with len 0). Copy them out before `buf` drops & frees.
    let bytes = if buf.0.is_null() || out_len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(buf.0, out_len).to_vec() }
    };

    // `buf` drops here -> isom_free(out). `bytes` is an independent copy.
    Ok(bytes)
}

/// Apply a batch of MRGN location ops to a map, saved IN PLACE.
///
/// `ops` is passed to the engine RAW (`ops.as_ptr()` / `ops.len()`); the
/// location NAME bytes inside it are NEVER re-encoded here (rules.md). The save
/// keeps `autoDefragmentLocations=false` / `lockAnywhere=true` (handled in C).
pub fn locedit(map_path: &Path, ops: &[u8]) -> Result<(), IsomError> {
    let _native_call = native_call_guard();
    let c_path = path_cstring(map_path)?;
    // SAFETY: `c_path` and `ops` both outlive the synchronous call; `ops` is
    // read-only on the C side. Empty `ops` => valid (ptr, 0) pair.
    let code = unsafe { isom_sys::isom_locedit(c_path.as_ptr(), ops.as_ptr(), ops.len()) };
    status(code)
}

/// Apply a batch of player ops (start locations + OWNR controllers) to a map,
/// saved IN PLACE. Same RAW-`ops` / save-safety contract as [`locedit`].
pub fn playeredit(map_path: &Path, ops: &[u8]) -> Result<(), IsomError> {
    let _native_call = native_call_guard();
    let c_path = path_cstring(map_path)?;
    // SAFETY: see `locedit` — identical buffer/lifetime contract.
    let code = unsafe { isom_sys::isom_playeredit(c_path.as_ptr(), ops.as_ptr(), ops.len()) };
    status(code)
}

/// Rename switches in a map, saved IN PLACE. Trigger references use numeric
/// switch ids and are therefore unchanged by this operation.
pub fn switchedit(map_path: &Path, ops: &[u8]) -> Result<(), IsomError> {
    let _native_call = native_call_guard();
    let c_path = path_cstring(map_path)?;
    // SAFETY: see `locedit` — identical buffer/lifetime contract.
    let code = unsafe { isom_sys::isom_switchedit(c_path.as_ptr(), ops.as_ptr(), ops.len()) };
    status(code)
}

/// Render a map through the native tileset renderer and return its 24-bpp BMP.
pub fn render_map(
    map_path: &Path,
    starcraft_path: &Path,
    scale: u32,
) -> Result<Vec<u8>, IsomError> {
    let _native_call = native_call_guard();
    let c_map_path = path_cstring(map_path)?;
    let c_starcraft_path = path_cstring(starcraft_path)?;
    let mut out: *mut u8 = std::ptr::null_mut();
    let mut out_len: usize = 0;

    // SAFETY: both C strings outlive the synchronous call; `out` and `out_len`
    // are valid out-params. The returned allocation is owned by `buf`.
    let code = unsafe {
        isom_sys::isom_render_map(
            c_map_path.as_ptr(),
            c_starcraft_path.as_ptr(),
            scale,
            &mut out,
            &mut out_len,
        )
    };
    let buf = CBuf(out);
    status(code)?;

    let bytes = if buf.0.is_null() || out_len == 0 {
        Vec::new()
    } else {
        // SAFETY: on ISOM_OK the C side guarantees `out_len` readable bytes.
        unsafe { std::slice::from_raw_parts(buf.0, out_len).to_vec() }
    };
    Ok(bytes)
}

fn buffer_bytes(buffer: &CBuf, length: usize) -> Vec<u8> {
    if buffer.0.is_null() || length == 0 {
        Vec::new()
    } else {
        // SAFETY: successful native calls guarantee `length` readable bytes.
        unsafe { std::slice::from_raw_parts(buffer.0, length).to_vec() }
    }
}

fn native_detail(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    if text.is_empty() {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| value.get("error")?.as_str().map(str::to_owned))
        .or_else(|| Some(text.to_owned()))
}

/// The engine writes the edited map to a temporary path and promotes it only
/// after verifying it. On Windows a scanner or indexer holding a handle on a
/// file the engine just wrote makes that write fail, and the engine reports it
/// as its own error rather than an OS one — this exact message.
const SAVE_CONTENTION_DETAIL: &str = "map save failed before output promotion";
const SAVE_CONTENTION_RETRIES: u32 = 4;

/// True when the engine failed for a reason that a moment's wait clears.
///
/// Retrying is running the same operation, not a second one: the batch is
/// deterministic, nothing has been promoted yet, and the engine deletes the
/// temporary it could not write.
fn is_save_contention(error: &NativeCallError) -> bool {
    error.status == IsomError::Engine && error.detail.as_deref() == Some(SAVE_CONTENTION_DETAIL)
}

pub fn mapedit(
    input_map_path: &Path,
    output_map_path: &Path,
    starcraft_path: &Path,
    batch_json: &[u8],
) -> Result<String, NativeCallError> {
    let _native_call = native_call_guard();
    let input = path_cstring(input_map_path).map_err(|error| NativeCallError::new(error, None))?;
    let output =
        path_cstring(output_map_path).map_err(|error| NativeCallError::new(error, None))?;
    let starcraft =
        path_cstring(starcraft_path).map_err(|error| NativeCallError::new(error, None))?;
    let mut backoff = std::time::Duration::from_millis(20);
    for _ in 0..SAVE_CONTENTION_RETRIES {
        match mapedit_once(&input, &output, &starcraft, batch_json) {
            Err(error) if is_save_contention(&error) => {
                std::thread::sleep(backoff);
                backoff *= 2;
            }
            settled => return settled,
        }
    }
    // The last attempt reports whatever it hits, so a save that is genuinely
    // broken still fails with its own message.
    mapedit_once(&input, &output, &starcraft, batch_json)
}

fn mapedit_once(
    input: &CString,
    output: &CString,
    starcraft: &CString,
    batch_json: &[u8],
) -> Result<String, NativeCallError> {
    let mut report: *mut u8 = std::ptr::null_mut();
    let mut report_len = 0_usize;
    // SAFETY: both paths and `batch_json` outlive this synchronous call. The
    // returned report uses the matching isom allocator and is guarded below.
    let code = unsafe {
        isom_sys::isom_mapedit(
            input.as_ptr(),
            output.as_ptr(),
            starcraft.as_ptr(),
            batch_json.as_ptr(),
            batch_json.len(),
            &mut report,
            &mut report_len,
        )
    };
    let report = CBuf(report);
    let bytes = buffer_bytes(&report, report_len);
    if let Err(error) = status(code) {
        return Err(NativeCallError::new(error, native_detail(&bytes)));
    }
    String::from_utf8(bytes)
        .map_err(|error| NativeCallError::new(IsomError::Engine, Some(error.to_string())))
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapNewStart {
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapNewPlayer {
    /// CHK slot 0..11; slots the spec omits become `inactive`.
    pub slot: u8,
    /// `human`, `computer`, `rescuable`, `neutral`, `inactive`, or `closed`.
    pub r#type: String,
    /// `zerg`, `terran`, `protoss`, `userSelectable`, `random`, `independent`,
    /// `neutral`, or `inactive`.
    pub race: String,
    /// Index into `forces`; required for slots 0..7 and absent for 8..11
    /// (FORC has eight entries).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<u8>,
    /// Only slots 0..7 may carry a start location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<MapNewStart>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapNewForce {
    /// Force name already encoded for the map's string table, 1..256 bytes,
    /// lowercase/uppercase hex; written verbatim by the engine.
    pub name_bytes_hex: String,
    pub allied: bool,
    pub allied_victory: bool,
    pub shared_vision: bool,
    pub random_start: bool,
}

/// Strict `eud-map-new/1` request. Serialized verbatim for the native engine,
/// which re-validates every field before touching the filesystem. Text fields
/// carry bytes the caller already encoded for the new map's legacy `STR `
/// table (see `chk::encode_chk_text`); nothing is re-encoded here or in C.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapNewSpec {
    /// `remastered` (default) or `broodWar`.
    pub version: String,
    pub tileset: u8,
    pub width: u16,
    pub height: u16,
    pub terrain_type: u16,
    /// Scenario title bytes as hex, 1..1024 bytes.
    pub title_bytes_hex: String,
    /// Scenario description bytes as hex, 0..4096 bytes.
    pub description_bytes_hex: String,
    /// Up to 12 entries with unique slots.
    pub players: Vec<MapNewPlayer>,
    pub forces: Vec<MapNewForce>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapNewReport {
    pub schema: String,
    pub ok: bool,
    pub tileset: String,
    pub width: u16,
    pub height: u16,
    pub terrain_type: u16,
    pub players: u64,
    pub start_locations: u64,
    pub output_sha256: String,
}

/// Create a brand-new map at `output_map_path` from `spec`. The path must not
/// exist; the engine promotes the file only after save and re-open verification.
pub fn map_new(
    output_map_path: &Path,
    starcraft_path: &Path,
    spec: &MapNewSpec,
) -> Result<MapNewReport, NativeCallError> {
    if !(64..=256).contains(&spec.width)
        || !(64..=256).contains(&spec.height)
        || spec.tileset > 7
        || spec.terrain_type == 0
        || spec.players.len() > 12
        || spec.forces.is_empty()
        || spec.forces.len() > 4
        || output_map_path.exists()
    {
        return Err(NativeCallError::new(IsomError::InvalidArg, None));
    }
    let mut request = serde_json::to_value(spec)
        .map_err(|error| NativeCallError::new(IsomError::InvalidArg, Some(error.to_string())))?;
    request["schema"] = serde_json::Value::String("eud-map-new/1".to_string());
    let request = serde_json::to_vec(&request)
        .map_err(|error| NativeCallError::new(IsomError::InvalidArg, Some(error.to_string())))?;
    let _native_call = native_call_guard();
    let output =
        path_cstring(output_map_path).map_err(|error| NativeCallError::new(error, None))?;
    let starcraft =
        path_cstring(starcraft_path).map_err(|error| NativeCallError::new(error, None))?;
    let mut report: *mut u8 = std::ptr::null_mut();
    let mut report_len = 0_usize;
    // SAFETY: both paths and the request buffer outlive this synchronous call.
    // The report is allocated by the C ABI and released by `CBuf` on every path.
    let code = unsafe {
        isom_sys::isom_map_new(
            output.as_ptr(),
            starcraft.as_ptr(),
            request.as_ptr(),
            request.len(),
            &mut report,
            &mut report_len,
        )
    };
    let report = CBuf(report);
    let bytes = buffer_bytes(&report, report_len);
    if let Err(error) = status(code) {
        return Err(NativeCallError::new(error, native_detail(&bytes)));
    }
    let parsed: MapNewReport = serde_json::from_slice(&bytes).map_err(|error| {
        NativeCallError::new(
            IsomError::Engine,
            Some(format!("invalid map-new report: {error}")),
        )
    })?;
    let expected_starts = spec
        .players
        .iter()
        .filter(|player| player.start.is_some())
        .count() as u64;
    let valid = parsed.schema == "eud-map-new-report/1"
        && parsed.ok
        && parsed.width == spec.width
        && parsed.height == spec.height
        && parsed.terrain_type == spec.terrain_type
        && parsed.players == spec.players.len() as u64
        && parsed.start_locations == expected_starts
        && exact_lower_hex(&parsed.output_sha256, 64);
    if !valid {
        return Err(NativeCallError::new(
            IsomError::Engine,
            Some("map-new report invariant mismatch".to_string()),
        ));
    }
    Ok(parsed)
}

pub fn render_region(
    map_path: &Path,
    starcraft_path: &Path,
    request_json: &[u8],
) -> Result<RgbaImage, NativeCallError> {
    let _native_call = native_call_guard();
    let map = path_cstring(map_path).map_err(|error| NativeCallError::new(error, None))?;
    let starcraft =
        path_cstring(starcraft_path).map_err(|error| NativeCallError::new(error, None))?;
    let mut rgba: *mut u8 = std::ptr::null_mut();
    let mut rgba_len = 0_usize;
    let mut width = 0_u32;
    let mut height = 0_u32;
    // SAFETY: input buffers outlive the call and every out-param is valid.
    let code = unsafe {
        isom_sys::isom_render_region(
            map.as_ptr(),
            starcraft.as_ptr(),
            request_json.as_ptr(),
            request_json.len(),
            &mut rgba,
            &mut rgba_len,
            &mut width,
            &mut height,
        )
    };
    let rgba = CBuf(rgba);
    let bytes = buffer_bytes(&rgba, rgba_len);
    if let Err(error) = status(code) {
        return Err(NativeCallError::new(error, native_detail(&bytes)));
    }
    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| {
            NativeCallError::new(
                IsomError::Engine,
                Some("RGBA dimensions overflow".to_string()),
            )
        })?;
    if bytes.len() != expected {
        return Err(NativeCallError::new(
            IsomError::Engine,
            Some(format!(
                "native RGBA length {} does not match {width}x{height}",
                bytes.len()
            )),
        ));
    }
    Ok(RgbaImage {
        width,
        height,
        rgba: bytes,
    })
}

pub fn catalog_query(
    starcraft_path: &Path,
    request_json: &[u8],
) -> Result<String, NativeCallError> {
    let _native_call = native_call_guard();
    let starcraft =
        path_cstring(starcraft_path).map_err(|error| NativeCallError::new(error, None))?;
    let mut output: *mut u8 = std::ptr::null_mut();
    let mut output_len = 0_usize;
    // SAFETY: the path/request and output pointers satisfy the synchronous ABI.
    let code = unsafe {
        isom_sys::isom_catalog_query(
            starcraft.as_ptr(),
            request_json.as_ptr(),
            request_json.len(),
            &mut output,
            &mut output_len,
        )
    };
    let output = CBuf(output);
    let bytes = buffer_bytes(&output, output_len);
    if let Err(error) = status(code) {
        return Err(NativeCallError::new(error, native_detail(&bytes)));
    }
    String::from_utf8(bytes)
        .map_err(|error| NativeCallError::new(IsomError::Engine, Some(error.to_string())))
}

/// Read one raw asset out of the installed StarCraft data (CASC/MPQ) verbatim.
///
/// `archive_path` is an ARCHIVE-internal path (e.g. `scripts\iscript.bin`), not
/// a filesystem path: it is handed to C as-is and must NOT go through
/// [`path_cstring`], whose Windows branch would resolve it against the current
/// directory. The bytes are returned uninterpreted — the caller owns the format.
pub fn game_asset(starcraft_path: &Path, archive_path: &str) -> Result<Vec<u8>, NativeCallError> {
    let _native_call = native_call_guard();
    let starcraft =
        path_cstring(starcraft_path).map_err(|error| NativeCallError::new(error, None))?;
    let asset = CString::new(archive_path)
        .map_err(|_| NativeCallError::new(IsomError::InvalidArg, None))?;
    let mut output: *mut u8 = std::ptr::null_mut();
    let mut output_len = 0_usize;
    // SAFETY: both C strings stay alive for the call and the output pointers
    // satisfy the synchronous ABI; the buffer is released by the `CBuf` guard.
    let code = unsafe {
        isom_sys::isom_game_asset(
            starcraft.as_ptr(),
            asset.as_ptr(),
            &mut output,
            &mut output_len,
        )
    };
    let output = CBuf(output);
    let bytes = buffer_bytes(&output, output_len);
    if let Err(error) = status(code) {
        return Err(NativeCallError::new(error, native_detail(&bytes)));
    }
    Ok(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapSoundAddReport {
    pub schema: String,
    pub ok: bool,
    pub reused: bool,
    pub sound_index: u64,
    pub sound_string_id: u64,
    pub mpq_path: String,
    pub asset_sha256: String,
    pub asset_bytes: u64,
    pub input_sha256: String,
    pub output_sha256: String,
    pub unrelated_chk_digest_before: String,
    pub unrelated_chk_digest_after: String,
    pub unrelated_asset_digest_before: String,
    pub unrelated_asset_digest_after: String,
}

/// One sound of a [`map_sound_add_batch`] report, in input order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapSoundBatchItemReport {
    pub reused: bool,
    pub sound_index: u64,
    pub sound_string_id: u64,
    pub mpq_path: String,
    pub asset_sha256: String,
    pub asset_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapSoundAddBatchReport {
    pub schema: String,
    pub ok: bool,
    pub sounds: Vec<MapSoundBatchItemReport>,
    pub input_sha256: String,
    pub output_sha256: String,
    pub unrelated_chk_digest_before: String,
    pub unrelated_chk_digest_after: String,
    pub unrelated_asset_digest_before: String,
    pub unrelated_asset_digest_after: String,
}

/// Most sounds one [`map_sound_add_batch`] call registers.
pub const MAX_SOUND_BATCH_ITEMS: usize = 128;

/// Add or exactly reuse several canonical managed OGG sounds in one copied map:
/// one native load, mutation, save, and reopen verification for the batch.
/// `items` are `(destination MPQ path, OGG bytes)` with distinct destinations.
pub fn map_sound_add_batch(
    input_map_path: &Path,
    output_map_path: &Path,
    expected_input_sha256: &str,
    items: &[(&str, &[u8])],
) -> Result<MapSoundAddBatchReport, NativeCallError> {
    const MAX_OGG_BYTES: usize = 64 * 1024 * 1024;
    let mut destinations = std::collections::BTreeSet::new();
    if !exact_lower_hex(expected_input_sha256, 64)
        || items.is_empty()
        || items.len() > MAX_SOUND_BATCH_ITEMS
        || input_map_path == output_map_path
        || items.iter().any(|(path, ogg)| {
            !valid_managed_sound_path(path)
                || !destinations.insert(path.to_ascii_lowercase())
                || ogg.len() < 4
                || ogg.len() > MAX_OGG_BYTES
                || !ogg.starts_with(b"OggS")
        })
    {
        return Err(NativeCallError::new(IsomError::InvalidArg, None));
    }
    let _native_call = native_call_guard();
    let input = path_cstring(input_map_path).map_err(|error| NativeCallError::new(error, None))?;
    let output =
        path_cstring(output_map_path).map_err(|error| NativeCallError::new(error, None))?;
    let expected = CString::new(expected_input_sha256)
        .map_err(|_| NativeCallError::new(IsomError::InvalidArg, None))?;
    let paths = items
        .iter()
        .map(|(path, _)| CString::new(*path))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| NativeCallError::new(IsomError::InvalidArg, None))?;
    let path_pointers = paths.iter().map(|path| path.as_ptr()).collect::<Vec<_>>();
    let ogg_pointers = items
        .iter()
        .map(|(_, ogg)| ogg.as_ptr())
        .collect::<Vec<_>>();
    let ogg_lengths = items.iter().map(|(_, ogg)| ogg.len()).collect::<Vec<_>>();
    let mut report: *mut u8 = std::ptr::null_mut();
    let mut report_len = 0_usize;
    // SAFETY: every C string, pointer array, and OGG buffer outlives this
    // synchronous call and the three arrays hold exactly `items.len()` entries.
    // The report is allocated by the C ABI and released by `CBuf` on every path.
    let code = unsafe {
        isom_sys::isom_map_sound_add_batch(
            input.as_ptr(),
            output.as_ptr(),
            expected.as_ptr(),
            path_pointers.as_ptr(),
            ogg_pointers.as_ptr(),
            ogg_lengths.as_ptr(),
            items.len(),
            &mut report,
            &mut report_len,
        )
    };
    let report = CBuf(report);
    let bytes = buffer_bytes(&report, report_len);
    if let Err(error) = status(code) {
        return Err(NativeCallError::new(error, native_detail(&bytes)));
    }
    let parsed: MapSoundAddBatchReport = serde_json::from_slice(&bytes).map_err(|error| {
        NativeCallError::new(
            IsomError::Engine,
            Some(format!("invalid sound-batch report: {error}")),
        )
    })?;
    let sounds_valid = parsed.sounds.len() == items.len()
        && parsed.sounds.iter().zip(items).all(|(sound, (path, ogg))| {
            sound.sound_index < 512
                && sound.sound_string_id > 0
                && sound.mpq_path == *path
                && sound.asset_sha256 == format!("{:x}", Sha256::digest(ogg))
                && sound.asset_bytes == ogg.len() as u64
        });
    let all_reused = parsed.sounds.iter().all(|sound| sound.reused);
    let valid = parsed.schema == "eud-map-sound-add-batch-report/1"
        && parsed.ok
        && sounds_valid
        && parsed.input_sha256 == expected_input_sha256
        && exact_lower_hex(&parsed.output_sha256, 64)
        && exact_lower_hex(&parsed.unrelated_chk_digest_before, 64)
        && parsed.unrelated_chk_digest_before == parsed.unrelated_chk_digest_after
        && exact_lower_hex(&parsed.unrelated_asset_digest_before, 64)
        && parsed.unrelated_asset_digest_before == parsed.unrelated_asset_digest_after
        && (!all_reused || parsed.output_sha256 == parsed.input_sha256);
    if !valid {
        return Err(NativeCallError::new(
            IsomError::Engine,
            Some("sound-batch report invariant mismatch".to_string()),
        ));
    }
    Ok(parsed)
}

pub fn map_sound_add(
    input_map_path: &Path,
    output_map_path: &Path,
    expected_input_sha256: &str,
    destination_mpq_path: &str,
    ogg_bytes: &[u8],
) -> Result<MapSoundAddReport, NativeCallError> {
    const MAX_OGG_BYTES: usize = 64 * 1024 * 1024;
    if !exact_lower_hex(expected_input_sha256, 64)
        || !valid_managed_sound_path(destination_mpq_path)
        || ogg_bytes.len() < 4
        || ogg_bytes.len() > MAX_OGG_BYTES
        || !ogg_bytes.starts_with(b"OggS")
        || input_map_path == output_map_path
    {
        return Err(NativeCallError::new(IsomError::InvalidArg, None));
    }
    let _native_call = native_call_guard();
    let input = path_cstring(input_map_path).map_err(|error| NativeCallError::new(error, None))?;
    let output =
        path_cstring(output_map_path).map_err(|error| NativeCallError::new(error, None))?;
    let expected = CString::new(expected_input_sha256)
        .map_err(|_| NativeCallError::new(IsomError::InvalidArg, None))?;
    let destination = CString::new(destination_mpq_path)
        .map_err(|_| NativeCallError::new(IsomError::InvalidArg, None))?;
    let mut report: *mut u8 = std::ptr::null_mut();
    let mut report_len = 0_usize;
    // SAFETY: all input buffers outlive this synchronous call. The report is
    // allocated by the C ABI and released by `CBuf` on every path.
    let code = unsafe {
        isom_sys::isom_map_sound_add(
            input.as_ptr(),
            output.as_ptr(),
            expected.as_ptr(),
            destination.as_ptr(),
            ogg_bytes.as_ptr(),
            ogg_bytes.len(),
            &mut report,
            &mut report_len,
        )
    };
    let report = CBuf(report);
    let bytes = buffer_bytes(&report, report_len);
    if let Err(error) = status(code) {
        return Err(NativeCallError::new(error, native_detail(&bytes)));
    }
    let parsed: MapSoundAddReport = serde_json::from_slice(&bytes).map_err(|error| {
        NativeCallError::new(
            IsomError::Engine,
            Some(format!("invalid sound-add report: {error}")),
        )
    })?;
    let asset_sha256 = format!("{:x}", Sha256::digest(ogg_bytes));
    let valid = parsed.schema == "eud-map-sound-add-report/1"
        && parsed.ok
        && parsed.sound_index < 512
        && parsed.sound_string_id > 0
        && parsed.mpq_path == destination_mpq_path
        && parsed.asset_sha256 == asset_sha256
        && parsed.asset_bytes == ogg_bytes.len() as u64
        && parsed.input_sha256 == expected_input_sha256
        && exact_lower_hex(&parsed.output_sha256, 64)
        && exact_lower_hex(&parsed.unrelated_chk_digest_before, 64)
        && parsed.unrelated_chk_digest_before == parsed.unrelated_chk_digest_after
        && exact_lower_hex(&parsed.unrelated_asset_digest_before, 64)
        && parsed.unrelated_asset_digest_before == parsed.unrelated_asset_digest_after
        && (!parsed.reused || parsed.output_sha256 == parsed.input_sha256);
    if !valid {
        return Err(NativeCallError::new(
            IsomError::Engine,
            Some("sound-add report invariant mismatch".to_string()),
        ));
    }
    Ok(parsed)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapSoundReplaceReport {
    pub schema: String,
    pub ok: bool,
    pub sound_index: u64,
    pub sound_string_id: u64,
    pub old_mpq_path: String,
    pub mpq_path: String,
    pub asset_sha256: String,
    pub asset_bytes: u64,
    pub input_sha256: String,
    pub output_sha256: String,
    pub unrelated_chk_digest_before: String,
    pub unrelated_chk_digest_after: String,
    pub unrelated_asset_digest_before: String,
    pub unrelated_asset_digest_after: String,
}

pub fn map_sound_replace(
    input_map_path: &Path,
    output_map_path: &Path,
    expected_input_sha256: &str,
    old_mpq_path: &str,
    destination_mpq_path: &str,
    ogg_bytes: &[u8],
) -> Result<MapSoundReplaceReport, NativeCallError> {
    const MAX_OGG_BYTES: usize = 64 * 1024 * 1024;
    if !exact_lower_hex(expected_input_sha256, 64)
        || !valid_managed_sound_path(old_mpq_path)
        || !valid_managed_sound_path(destination_mpq_path)
        || old_mpq_path == destination_mpq_path
        || ogg_bytes.len() < 4
        || ogg_bytes.len() > MAX_OGG_BYTES
        || !ogg_bytes.starts_with(b"OggS")
        || input_map_path == output_map_path
    {
        return Err(NativeCallError::new(IsomError::InvalidArg, None));
    }
    let _native_call = native_call_guard();
    let input = path_cstring(input_map_path).map_err(|error| NativeCallError::new(error, None))?;
    let output =
        path_cstring(output_map_path).map_err(|error| NativeCallError::new(error, None))?;
    let expected = CString::new(expected_input_sha256)
        .map_err(|_| NativeCallError::new(IsomError::InvalidArg, None))?;
    let old = CString::new(old_mpq_path)
        .map_err(|_| NativeCallError::new(IsomError::InvalidArg, None))?;
    let destination = CString::new(destination_mpq_path)
        .map_err(|_| NativeCallError::new(IsomError::InvalidArg, None))?;
    let mut report: *mut u8 = std::ptr::null_mut();
    let mut report_len = 0_usize;
    // SAFETY: all input buffers outlive this synchronous call. The report is
    // allocated by the C ABI and released by `CBuf` on every path.
    let code = unsafe {
        isom_sys::isom_map_sound_replace(
            input.as_ptr(),
            output.as_ptr(),
            expected.as_ptr(),
            old.as_ptr(),
            destination.as_ptr(),
            ogg_bytes.as_ptr(),
            ogg_bytes.len(),
            &mut report,
            &mut report_len,
        )
    };
    let report = CBuf(report);
    let bytes = buffer_bytes(&report, report_len);
    if let Err(error) = status(code) {
        return Err(NativeCallError::new(error, native_detail(&bytes)));
    }
    let parsed: MapSoundReplaceReport = serde_json::from_slice(&bytes).map_err(|error| {
        NativeCallError::new(
            IsomError::Engine,
            Some(format!("invalid sound-replace report: {error}")),
        )
    })?;
    let asset_sha256 = format!("{:x}", Sha256::digest(ogg_bytes));
    let valid = parsed.schema == "eud-map-sound-replace-report/1"
        && parsed.ok
        && parsed.sound_index < 512
        && parsed.sound_string_id > 0
        && parsed.old_mpq_path == old_mpq_path
        && parsed.mpq_path == destination_mpq_path
        && parsed.asset_sha256 == asset_sha256
        && parsed.asset_bytes == ogg_bytes.len() as u64
        && parsed.input_sha256 == expected_input_sha256
        && exact_lower_hex(&parsed.output_sha256, 64)
        && exact_lower_hex(&parsed.unrelated_chk_digest_before, 64)
        && parsed.unrelated_chk_digest_before == parsed.unrelated_chk_digest_after
        && exact_lower_hex(&parsed.unrelated_asset_digest_before, 64)
        && parsed.unrelated_asset_digest_before == parsed.unrelated_asset_digest_after;
    if !valid {
        return Err(NativeCallError::new(
            IsomError::Engine,
            Some("sound-replace report invariant mismatch".to_string()),
        ));
    }
    Ok(parsed)
}

/// One removed WAV slot of a [`map_sound_remove`] report, in input order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapSoundRemoveItemReport {
    pub sound_index: u64,
    pub sound_string_id: u64,
    /// The removed MPQ asset's SHA-256, empty when the string named no asset
    /// in the map (a virtual StarCraft sound path).
    pub asset_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapSoundRemoveReport {
    pub schema: String,
    pub ok: bool,
    pub sounds: Vec<MapSoundRemoveItemReport>,
    pub input_sha256: String,
    pub output_sha256: String,
    pub unrelated_chk_digest_before: String,
    pub unrelated_chk_digest_after: String,
    pub unrelated_asset_digest_before: String,
    pub unrelated_asset_digest_after: String,
}

/// Remove distinct WAV registrations (slot, game string, and the MPQ asset the
/// string names) from one copied map in one native load/mutate/save. A string
/// any other CHK user still references is refused before anything changes.
pub fn map_sound_remove(
    input_map_path: &Path,
    output_map_path: &Path,
    expected_input_sha256: &str,
    sound_indexes: &[u16],
) -> Result<MapSoundRemoveReport, NativeCallError> {
    let distinct = sound_indexes
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    if !exact_lower_hex(expected_input_sha256, 64)
        || sound_indexes.is_empty()
        || sound_indexes.len() > 512
        || distinct != sound_indexes.len()
        || sound_indexes.iter().any(|index| *index >= 512)
        || input_map_path == output_map_path
    {
        return Err(NativeCallError::new(IsomError::InvalidArg, None));
    }
    let _native_call = native_call_guard();
    let input = path_cstring(input_map_path).map_err(|error| NativeCallError::new(error, None))?;
    let output =
        path_cstring(output_map_path).map_err(|error| NativeCallError::new(error, None))?;
    let expected = CString::new(expected_input_sha256)
        .map_err(|_| NativeCallError::new(IsomError::InvalidArg, None))?;
    let mut report: *mut u8 = std::ptr::null_mut();
    let mut report_len = 0_usize;
    // SAFETY: the C strings and the index slice outlive this synchronous call,
    // and `count` is the slice length. The report is allocated by the C ABI and
    // released by `CBuf` on every path.
    let code = unsafe {
        isom_sys::isom_map_sound_remove(
            input.as_ptr(),
            output.as_ptr(),
            expected.as_ptr(),
            sound_indexes.as_ptr(),
            sound_indexes.len(),
            &mut report,
            &mut report_len,
        )
    };
    let report = CBuf(report);
    let bytes = buffer_bytes(&report, report_len);
    if let Err(error) = status(code) {
        return Err(NativeCallError::new(error, native_detail(&bytes)));
    }
    let parsed: MapSoundRemoveReport = serde_json::from_slice(&bytes).map_err(|error| {
        NativeCallError::new(
            IsomError::Engine,
            Some(format!("invalid sound-remove report: {error}")),
        )
    })?;
    let valid = parsed.schema == "eud-map-sound-remove-report/1"
        && parsed.ok
        && parsed.sounds.len() == sound_indexes.len()
        && parsed
            .sounds
            .iter()
            .zip(sound_indexes)
            .all(|(sound, index)| {
                sound.sound_index == u64::from(*index)
                    && sound.sound_string_id > 0
                    && (sound.asset_sha256.is_empty() || exact_lower_hex(&sound.asset_sha256, 64))
            })
        && parsed.input_sha256 == expected_input_sha256
        && exact_lower_hex(&parsed.output_sha256, 64)
        && parsed.output_sha256 != parsed.input_sha256
        && exact_lower_hex(&parsed.unrelated_chk_digest_before, 64)
        && parsed.unrelated_chk_digest_before == parsed.unrelated_chk_digest_after
        && exact_lower_hex(&parsed.unrelated_asset_digest_before, 64)
        && parsed.unrelated_asset_digest_before == parsed.unrelated_asset_digest_after;
    if !valid {
        return Err(NativeCallError::new(
            IsomError::Engine,
            Some("sound-remove report invariant mismatch".to_string()),
        ));
    }
    Ok(parsed)
}

fn exact_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_managed_sound_path(path: &str) -> bool {
    let Some(hash) = path
        .strip_prefix("staredit\\wav\\ea_")
        .and_then(|path| path.strip_suffix(".ogg"))
    else {
        return false;
    };
    matches!(hash.len(), 16 | 24 | 32 | 64)
        && exact_lower_hex(hash, hash.len())
        && path.is_ascii()
        && !path.contains('/')
        && !path.contains(':')
        && !path.contains("..")
        && !path.contains("\\\\")
}

pub fn map_digest(map_path: &Path) -> Result<String, NativeCallError> {
    let _native_call = native_call_guard();
    let map = path_cstring(map_path).map_err(|error| NativeCallError::new(error, None))?;
    let mut output: *mut u8 = std::ptr::null_mut();
    let mut output_len = 0_usize;
    // SAFETY: the C string and output pointers satisfy the synchronous ABI.
    let code = unsafe { isom_sys::isom_map_digest(map.as_ptr(), &mut output, &mut output_len) };
    let output = CBuf(output);
    let bytes = buffer_bytes(&output, output_len);
    if let Err(error) = status(code) {
        return Err(NativeCallError::new(error, native_detail(&bytes)));
    }
    String::from_utf8(bytes)
        .map_err(|error| NativeCallError::new(IsomError::Engine, Some(error.to_string())))
}

/// Read one named extra asset out of a map's MPQ, verbatim.
///
/// `mpq_path` is an MPQ-internal name (e.g. `staredit\wav\a.ogg`), handed to C
/// as-is like [`game_asset`]'s archive path. The bytes are exactly what
/// [`map_digest`] hashes for that path; an asset larger than `max_bytes` is
/// refused natively before it is read.
pub fn map_asset(
    map_path: &Path,
    mpq_path: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, NativeCallError> {
    if max_bytes == 0 {
        return Err(NativeCallError::new(IsomError::InvalidArg, None));
    }
    let _native_call = native_call_guard();
    let map = path_cstring(map_path).map_err(|error| NativeCallError::new(error, None))?;
    let asset =
        CString::new(mpq_path).map_err(|_| NativeCallError::new(IsomError::InvalidArg, None))?;
    let mut output: *mut u8 = std::ptr::null_mut();
    let mut output_len = 0_usize;
    // SAFETY: both C strings stay alive for the call and the output pointers
    // satisfy the synchronous ABI; the buffer is released by the `CBuf` guard.
    let code = unsafe {
        isom_sys::isom_map_asset(
            map.as_ptr(),
            asset.as_ptr(),
            max_bytes,
            &mut output,
            &mut output_len,
        )
    };
    let output = CBuf(output);
    let bytes = buffer_bytes(&output, output_len);
    if let Err(error) = status(code) {
        return Err(NativeCallError::new(error, native_detail(&bytes)));
    }
    if bytes.len() > max_bytes {
        return Err(NativeCallError::new(
            IsomError::Engine,
            Some("native map asset exceeded the requested size limit".to_string()),
        ));
    }
    Ok(bytes)
}

pub fn image_quantize(
    starcraft_path: &Path,
    tileset: u16,
    rgba: &[u8],
    width: u16,
    height: u16,
    before_tiles: &[u16],
) -> Result<ImageQuantizeResult, NativeCallError> {
    let cells = usize::from(width)
        .checked_mul(usize::from(height))
        .filter(|cells| {
            width > 0 && height > 0 && width <= 256 && height <= 256 && *cells <= 65_536
        })
        .ok_or_else(|| {
            NativeCallError::new(
                IsomError::InvalidArg,
                Some("image quantizer dimensions are outside 1..=256".to_string()),
            )
        })?;
    let rgba_len = cells.checked_mul(4).ok_or_else(|| {
        NativeCallError::new(
            IsomError::InvalidArg,
            Some("image RGBA length overflow".to_string()),
        )
    })?;
    if rgba.len() != rgba_len || before_tiles.len() != cells {
        return Err(NativeCallError::new(
            IsomError::InvalidArg,
            Some("image quantizer input lengths do not match dimensions".to_string()),
        ));
    }

    let _native_call = native_call_guard();
    let starcraft =
        path_cstring(starcraft_path).map_err(|error| NativeCallError::new(error, None))?;
    let mut output: *mut u8 = std::ptr::null_mut();
    let mut output_len = 0_usize;
    // SAFETY: every input slice and out-param outlives this synchronous call.
    let code = unsafe {
        isom_sys::isom_image_quantize(
            starcraft.as_ptr(),
            tileset,
            rgba.as_ptr(),
            rgba.len(),
            width,
            height,
            before_tiles.as_ptr(),
            before_tiles.len(),
            &mut output,
            &mut output_len,
        )
    };
    let output = CBuf(output);
    let bytes = if output.0.is_null() || output_len == 0 {
        &[][..]
    } else {
        // SAFETY: the native ABI returns exactly `output_len` readable bytes.
        unsafe { std::slice::from_raw_parts(output.0, output_len) }
    };
    if let Err(error) = status(code) {
        return Err(NativeCallError::new(error, native_detail(bytes)));
    }
    let expected = 20_usize
        .checked_add(cells.checked_mul(5).ok_or_else(|| {
            NativeCallError::new(
                IsomError::Engine,
                Some("image quantizer result length overflow".to_string()),
            )
        })?)
        .ok_or_else(|| {
            NativeCallError::new(
                IsomError::Engine,
                Some("image quantizer result length overflow".to_string()),
            )
        })?;
    if bytes.len() != expected || bytes.get(..4) != Some(b"MIQ1") {
        return Err(NativeCallError::new(
            IsomError::Engine,
            Some("native image quantizer returned an invalid envelope".to_string()),
        ));
    }
    let read_u16 = |offset: usize| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    let read_u32 = |offset: usize| {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    };
    let result_width = read_u16(4);
    let result_height = read_u16(6);
    if result_width != width || result_height != height {
        return Err(NativeCallError::new(
            IsomError::Engine,
            Some("native image quantizer changed output dimensions".to_string()),
        ));
    }
    let tile_start = 20;
    let preview_start = tile_start + cells * 2;
    let mut tiles = Vec::with_capacity(cells);
    for offset in (tile_start..preview_start).step_by(2) {
        tiles.push(read_u16(offset));
    }
    Ok(ImageQuantizeResult {
        width,
        height,
        tiles,
        preview_rgb: bytes[preview_start..].to_vec(),
        unique_tile_count: read_u32(8),
        walkability_changed_cells: read_u32(12),
        height_changed_cells: read_u32(16),
    })
}

pub const EXPECTED_ABI_VERSION: i32 = 10;

pub fn assert_abi_version() -> Result<(), IsomError> {
    let actual = abi_version();
    if actual == EXPECTED_ABI_VERSION {
        Ok(())
    } else {
        Err(IsomError::UnknownCode(actual))
    }
}

/// ABI version of the linked static lib — a load-time sanity check that the
/// `.lib` matches the bindings.
pub fn abi_version() -> i32 {
    // SAFETY: a pure, side-effect-free C accessor returning a constant int.
    unsafe { isom_sys::isom_abi_version() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the save race is waited out. Any other engine failure is the
    /// engine telling us the edit is wrong, and repeating it would hide that.
    #[test]
    fn only_the_save_race_is_retried() {
        assert!(is_save_contention(&NativeCallError::new(
            IsomError::Engine,
            Some(SAVE_CONTENTION_DETAIL.to_string())
        )));
        assert!(!is_save_contention(&NativeCallError::new(
            IsomError::Engine,
            Some("unsupported operation 'terrain.set'".to_string())
        )));
        assert!(!is_save_contention(&NativeCallError::new(
            IsomError::Engine,
            None
        )));
        assert!(!is_save_contention(&NativeCallError::new(
            IsomError::Io,
            Some(SAVE_CONTENTION_DETAIL.to_string())
        )));
    }

    #[test]
    fn status_maps_every_known_code() {
        use isom_sys::IsomStatus as S;
        assert!(status(S::ISOM_OK).is_ok());
        assert!(matches!(
            status(S::ISOM_ERR_INVALID_ARG),
            Err(IsomError::InvalidArg)
        ));
        assert!(matches!(
            status(S::ISOM_ERR_OPEN_MAP),
            Err(IsomError::OpenMap)
        ));
        assert!(matches!(status(S::ISOM_ERR_IO), Err(IsomError::Io)));
        assert!(matches!(status(S::ISOM_ERR_ENGINE), Err(IsomError::Engine)));
        assert!(matches!(
            status(S::ISOM_ERR_EXCEPTION),
            Err(IsomError::Exception)
        ));
        assert!(matches!(status(S::ISOM_ERR_FAULT), Err(IsomError::Fault)));
    }

    #[test]
    fn status_maps_unknown_code() {
        assert!(matches!(status(99), Err(IsomError::UnknownCode(99))));
    }

    #[test]
    fn embedded_nul_path_maps_to_invalid_arg() {
        let err = chk_extract(Path::new("a\0b.scx")).expect_err("NUL path must error");
        assert!(matches!(err, IsomError::InvalidArg));
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_paths_apply_extended_prefix_only_when_needed() {
        use std::path::PathBuf;

        let relative = path_cstring(Path::new(r".\nested\..\output.scx")).unwrap();
        let relative = relative.to_str().unwrap();
        assert!(Path::new(relative).is_absolute());
        assert!(relative.ends_with(r"\output.scx"));
        assert!(!relative.contains(r"\nested\..\"));

        let short_nonexistent = Path::new(r"C:\isom-path-control\folder\..\output.scx");
        assert!(!short_nonexistent.exists());
        let short_nonexistent = path_cstring(short_nonexistent).unwrap();
        assert_eq!(
            short_nonexistent.to_str().unwrap(),
            r"C:\isom-path-control\output.scx"
        );

        let unc = path_cstring(Path::new(r"\\server\share\folder\..\output.scx")).unwrap();
        assert_eq!(unc.to_str().unwrap(), r"\\server\share\output.scx");

        let long_relative = PathBuf::from(r".\nested\..")
            .join("r".repeat(140))
            .join("s".repeat(140))
            .join("missing.scx");
        let long_relative = path_cstring(&long_relative).unwrap();
        let long_relative = long_relative.to_str().unwrap();
        assert!(long_relative.starts_with(r"\\?\"));
        assert!(!long_relative.contains(r"\nested\..\"));

        let long_nonexistent = std::env::temp_dir()
            .join("d".repeat(140))
            .join("e".repeat(140))
            .join("missing.scx");
        assert!(!long_nonexistent.exists());
        let long_nonexistent = path_cstring(&long_nonexistent).unwrap();
        assert!(long_nonexistent.to_str().unwrap().starts_with(r"\\?\"));

        let long_unc = format!(
            r"\\server\share\{}\{}\output.scx",
            "u".repeat(120),
            "v".repeat(120)
        );
        let long_unc = path_cstring(Path::new(&long_unc)).unwrap();
        assert!(long_unc
            .to_str()
            .unwrap()
            .starts_with(r"\\?\UNC\server\share\"));

        for namespaced in [
            r"\\?\C:\already\extended.scx",
            r"\\?\UNC\server\share\already.scx",
            r"\\.\pipe\already-device",
        ] {
            assert_eq!(
                path_cstring(Path::new(namespaced))
                    .unwrap()
                    .to_str()
                    .unwrap(),
                namespaced
            );
        }
        assert!(matches!(
            path_cstring(Path::new("")),
            Err(IsomError::InvalidArg)
        ));
        assert!(matches!(
            path_cstring(Path::new("a\0b.scx")),
            Err(IsomError::InvalidArg)
        ));
    }
}
