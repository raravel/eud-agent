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
//! Location NAME bytes inside the `ops` buffer for [`locedit`] / [`playeredit`]
//! are passed through to C RAW — never re-encoded in Rust (rules.md).

use std::ffi::{CString, NulError};
use std::os::raw::c_int;
use std::path::Path;

/// Typed errors mapped from the C `IsomStatus` codes (and the few Rust-side
/// failures that mean the call could never reach the engine).
#[derive(Debug, thiserror::Error)]
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
    Ok(CString::new(s)?)
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
    let c_path = path_cstring(map_path)?;
    // SAFETY: `c_path` and `ops` both outlive the synchronous call; `ops` is
    // read-only on the C side. Empty `ops` => valid (ptr, 0) pair.
    let code = unsafe { isom_sys::isom_locedit(c_path.as_ptr(), ops.as_ptr(), ops.len()) };
    status(code)
}

/// Apply a batch of player ops (start locations + OWNR controllers) to a map,
/// saved IN PLACE. Same RAW-`ops` / save-safety contract as [`locedit`].
pub fn playeredit(map_path: &Path, ops: &[u8]) -> Result<(), IsomError> {
    let c_path = path_cstring(map_path)?;
    // SAFETY: see `locedit` — identical buffer/lifetime contract.
    let code = unsafe { isom_sys::isom_playeredit(c_path.as_ptr(), ops.as_ptr(), ops.len()) };
    status(code)
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
}
