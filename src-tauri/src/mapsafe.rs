//! Map-write safety rails + journal (port of the Python `chk_info` write path +
//! `journal._rollback_location`).
//!
//! EVERY mutating map write (location_write / player_setup / switch_write) runs
//! these rails IN ORDER (rules.md "Map file writes"; features/09 "Safety rails"):
//!
//! 1. **Compiling guard** — refuse while the editor reports a build in progress
//!    (`compiling=true`); writing the map mid-build races the editor's read.
//! 2. **Lock probe** — refuse while the map file is open in another program
//!    (`CreateFileW` no-share probe → `ERROR_SHARING_VIOLATION`); SCMDraft
//!    holding the map open would corrupt an in-place save.
//! 3. **Full-file backup** — copy the whole map to
//!    `<data_dir>/map_backups/<mapname>.<timestamp>.bak` BEFORE mutating; this
//!    is the rollback source.
//! 4. **All-or-nothing apply** — apply the op buffer through the engine. The
//!    engine aborts-before-save on any bad op, so a failed apply leaves the
//!    on-disk map untouched → nothing to restore.
//! 5. **Re-digest verify** — re-extract/parse the map after the apply to confirm
//!    it is still readable; a digest failure signals corruption.
//! 6. **Journal entry** — record `{map_path, backup_path}` so the write can be
//!    reversed (changeset rollback).
//! 7. **Rollback** — restore the backed-up bytes over the map via a temp file +
//!    atomic rename, refusing while the map is locked (the SAME probe as rail 2).
//!
//! The rails live HERE in Rust, never in the C++ engine (rules.md): the C ABI
//! stays pure byte-level map ops. #64 (Anywhere) protection lives in the C ABI,
//! not here. The op buffer is passed to the engine RAW — mapsafe is generic over
//! it and NEVER re-encodes location NAME bytes (rules.md).
//!
//! External collaborators (the compiling-status source, the lock probe, and the
//! map engine) are abstracted behind traits so the full rail sequence is
//! testable with NO live editor and NO real map. Production uses the isom-backed
//! [`IsomEngine`] [`MapEngine`]; tests use a fake. The backup (rail 3) and
//! restore (rail 7) are REAL filesystem ops and are tested for real against temp
//! dirs.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Typed errors for the map-write rail sequence and rollback.
#[derive(Debug, thiserror::Error)]
pub enum MapSafeError {
    /// Rail 1: the editor reports `compiling=true`. Retry after the build finishes.
    #[error(
        "the editor is building right now; retry after the build finishes \
         (writing the map mid-build risks a corrupt read)"
    )]
    Compiling,
    /// Rail 2 / rail 7: the map file is open in another program (SCMDraft).
    #[error("map file is open in another program: {0} (close SCMDraft and retry)")]
    MapLocked(PathBuf),
    /// Rail 3 / rail 7: a filesystem operation (backup copy, temp write, rename) failed.
    #[error("map backup/restore I/O failure: {0}")]
    Io(#[from] std::io::Error),
    /// Rail 7: the recorded backup file is missing, so there is nothing to restore.
    #[error("map backup not found: {0}")]
    BackupNotFound(PathBuf),
    /// Rail 4: the engine rejected the op buffer (bad op) and aborted before save —
    /// the on-disk map is untouched, so no rollback is needed.
    #[error("map engine rejected the edit (bad op; map left untouched): {0}")]
    Apply(String),
    /// Rail 5: the post-apply re-digest failed — the map may be corrupt. The
    /// `backup` path is surfaced so the caller can recover (reconstruct a
    /// [`JournalEntry`] and [`MapSafe::restore`], or inspect the backup); the
    /// edit ALREADY saved, so auto-restore is intentionally NOT done here.
    #[error("post-write verify failed (map may be corrupt): {detail} — backup at {backup}")]
    Verify {
        /// The engine's re-digest error message.
        detail: String,
        /// The full-file backup taken before this write (the recovery source).
        backup: PathBuf,
    },
    /// Post-write sound verification failed; the exact backup was restored.
    #[error("post-write verify failed; exact backup restored: {detail} — backup at {backup}")]
    PostVerifyRestored { detail: String, backup: PathBuf },
    /// Input authority changed before backup/native mutation.
    #[error("source map SHA-256 is stale: expected {expected}, got {actual}")]
    StaleSource { expected: String, actual: String },
    /// Post-write verification failed and exact backup restoration also failed.
    #[error("post-write verify failed and rollback failed: {detail} — backup at {backup}")]
    Rollback { detail: String, backup: PathBuf },
    /// Backup/native output/atomic replacement cannot fit before mutation.
    #[error("insufficient disk space for sound import: need {required} bytes, have {available}")]
    InsufficientDisk { required: u64, available: u64 },
}

/// Source of the editor's build state (rail 1).
///
/// The real impl reads the bridge `status.txt` / a `STATUS` reply; tests inject a
/// fake. Returning `Ok(true)` means a build is in progress (refuse the write).
pub trait CompilingStatus {
    /// True iff the editor reports a build in progress (`compiling=true`).
    fn is_compiling(&self) -> bool;
}

/// Windows share-probe for whether the map is open elsewhere (rails 2 & 7).
///
/// The real impl is [`WindowsLockProbe`] (`CreateFileW` with `dwShareMode=0`);
/// tests inject a fake that flips a flag. On non-Windows the real impl reports
/// unlocked (the apply itself still fails safely if needed).
pub trait LockProbe {
    /// True iff `path` is held open by another process (sharing violation).
    fn is_locked(&self, path: &Path) -> bool;
}

/// Which isom op family a write routes to (rail 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Locedit,
    PlayerEdit,
    SwitchEdit,
}

/// The map engine: all-or-nothing apply (rail 4) + re-digest verify (rail 5).
pub trait MapEngine {
    /// Apply the RAW op buffer to `map`, saved IN PLACE, routing by `kind`. The
    /// engine aborts BEFORE save on any bad op (`Err` ⇒ the on-disk map is
    /// untouched). `ops` is passed through raw — location NAME bytes are NEVER
    /// re-encoded here.
    fn apply(&self, map: &Path, kind: OpKind, ops: &[u8]) -> Result<(), String>;

    /// Re-extract/parse `map` to confirm it is still readable (rail 5). The bytes
    /// are the verify digest; an `Err` signals corruption.
    fn digest(&self, map: &Path) -> Result<Vec<u8>, String>;
}

pub trait SoundMapEngine {
    fn add_sound(
        &self,
        input: &Path,
        output: &Path,
        expected_input_sha256: &str,
        destination_mpq_path: &str,
        ogg_bytes: &[u8],
    ) -> Result<isom::MapSoundAddReport, String>;

    fn replace_sound(
        &self,
        input: &Path,
        output: &Path,
        expected_input_sha256: &str,
        old_mpq_path: &str,
        destination_mpq_path: &str,
        ogg_bytes: &[u8],
    ) -> Result<isom::MapSoundReplaceReport, String>;

    fn verify_sound(
        &self,
        map: &Path,
        destination_mpq_path: &str,
        normalized_sha256: &str,
        sound_index: u64,
        sound_string_id: u64,
    ) -> Result<(), String>;
    fn verify_sound_replacement(
        &self,
        map: &Path,
        old_mpq_path: &str,
        destination_mpq_path: &str,
        normalized_sha256: &str,
        sound_index: u64,
        sound_string_id: u64,
    ) -> Result<(), String>;
}

/// Production [`MapEngine`] backed by the vendored isom static lib (feature 13).
/// `digest` re-extracts the CHK; `apply` routes by [`OpKind`] to the matching
/// isom op. `isom::IsomError` is mapped to its `Display` string (the rails in
/// `MapSafe` turn it into the typed `MapSafeError`). The map-write SAFETY RAILS
/// stay in `MapSafe`, never here (rules.md).
pub struct IsomEngine;

impl MapEngine for IsomEngine {
    fn apply(&self, map: &Path, kind: OpKind, ops: &[u8]) -> Result<(), String> {
        match kind {
            OpKind::Locedit => isom::locedit(map, ops),
            OpKind::PlayerEdit => isom::playeredit(map, ops),
            OpKind::SwitchEdit => isom::switchedit(map, ops),
        }
        .map_err(|e| e.to_string())
    }

    fn digest(&self, map: &Path) -> Result<Vec<u8>, String> {
        isom::chk_extract(map).map_err(|e| e.to_string())
    }
}

impl SoundMapEngine for IsomEngine {
    fn add_sound(
        &self,
        input: &Path,
        output: &Path,
        expected_input_sha256: &str,
        destination_mpq_path: &str,
        ogg_bytes: &[u8],
    ) -> Result<isom::MapSoundAddReport, String> {
        isom::map_sound_add(
            input,
            output,
            expected_input_sha256,
            destination_mpq_path,
            ogg_bytes,
        )
        .map_err(|error| error.to_string())
    }

    fn replace_sound(
        &self,
        input: &Path,
        output: &Path,
        expected_input_sha256: &str,
        old_mpq_path: &str,
        destination_mpq_path: &str,
        ogg_bytes: &[u8],
    ) -> Result<isom::MapSoundReplaceReport, String> {
        isom::map_sound_replace(
            input,
            output,
            expected_input_sha256,
            old_mpq_path,
            destination_mpq_path,
            ogg_bytes,
        )
        .map_err(|error| error.to_string())
    }

    fn verify_sound(
        &self,
        map: &Path,
        destination_mpq_path: &str,
        normalized_sha256: &str,
        sound_index: u64,
        sound_string_id: u64,
    ) -> Result<(), String> {
        let container: serde_json::Value =
            serde_json::from_str(&isom::map_digest(map).map_err(|error| error.to_string())?)
                .map_err(|error| format!("invalid map digest JSON: {error}"))?;
        let assets = container["extraAssets"]["assets"]
            .as_array()
            .ok_or_else(|| "map digest has no MPQ asset inventory".to_string())?;
        let asset_matches = assets.iter().filter(|asset| {
            asset["path"].as_str() == Some(destination_mpq_path)
                && asset["sha256"].as_str() == Some(normalized_sha256)
        });
        if asset_matches.count() != 1 {
            return Err("saved map does not contain exactly one requested MPQ asset".to_string());
        }
        let chk = isom::chk_extract(map).map_err(|error| error.to_string())?;
        let sounds = crate::chk::parse_sounds(&chk);
        let matching = sounds
            .iter()
            .filter(|sound| {
                sound.sound_index as u64 == sound_index
                    && u64::from(sound.string_id) == sound_string_id
                    && sound.mpq_path == destination_mpq_path
            })
            .count();
        if matching != 1
            || sounds
                .iter()
                .filter(|sound| sound.mpq_path == destination_mpq_path)
                .count()
                != 1
        {
            return Err("saved map sound string/WAV slot verification failed".to_string());
        }
        Ok(())
    }

    fn verify_sound_replacement(
        &self,
        map: &Path,
        old_mpq_path: &str,
        destination_mpq_path: &str,
        normalized_sha256: &str,
        sound_index: u64,
        sound_string_id: u64,
    ) -> Result<(), String> {
        self.verify_sound(
            map,
            destination_mpq_path,
            normalized_sha256,
            sound_index,
            sound_string_id,
        )?;
        let container: serde_json::Value =
            serde_json::from_str(&isom::map_digest(map).map_err(|error| error.to_string())?)
                .map_err(|error| format!("invalid map digest JSON: {error}"))?;
        let assets = container["extraAssets"]["assets"]
            .as_array()
            .ok_or_else(|| "map digest has no MPQ asset inventory".to_string())?;
        if assets
            .iter()
            .any(|asset| asset["path"].as_str() == Some(old_mpq_path))
        {
            return Err("saved map still contains the replaced MPQ asset".to_string());
        }
        let chk = isom::chk_extract(map).map_err(|error| error.to_string())?;
        if crate::chk::parse_sounds(&chk)
            .iter()
            .any(|sound| sound.mpq_path == old_mpq_path)
        {
            return Err("saved map still contains the replaced sound registration".to_string());
        }
        Ok(())
    }
}

/// A journal record for one map write (rail 6) — the rollback bookkeeping the
/// reject path needs (mirrors the Python journal `before={mapPath, backupPath}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    /// The map that was written (the restore target).
    pub map_path: PathBuf,
    /// The full-file backup taken before the write (the restore source).
    pub backup_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundJournalEntry {
    pub map_path: PathBuf,
    pub backup_path: PathBuf,
    pub report: isom::MapSoundAddReport,
    pub map_bytes_before: u64,
    pub map_bytes_after: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundReplaceJournalEntry {
    pub map_path: PathBuf,
    pub backup_path: PathBuf,
    pub report: isom::MapSoundReplaceReport,
    pub map_bytes_before: u64,
    pub map_bytes_after: u64,
}

/// Real Windows lock probe: `CreateFileW(path, GENERIC_READ, dwShareMode=0,
/// OPEN_EXISTING)`. A `ERROR_SHARING_VIOLATION` (32) means another program holds
/// the map open; otherwise the probe handle is closed immediately. On non-Windows
/// it reports unlocked (the apply still fails safely if needed).
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsLockProbe;

/// `ERROR_SHARING_VIOLATION` — another process holds the file open without
/// sharing (the value Win32 returns from `GetLastError` after a `CreateFileW`
/// with `dwShareMode=0` against a held-open file).
#[cfg(windows)]
const ERROR_SHARING_VIOLATION: u32 = 32;

// Minimal raw `extern "C"` declarations of the three Win32 calls the probe
// needs. Declaring them here (rather than via `windows-sys`/`winapi`) keeps the
// real probe dependency-free — Cargo.toml is out of scope for this task. `kernel32`
// is linked by every MSVC-target binary, so these resolve at link time.
#[cfg(windows)]
extern "system" {
    fn CreateFileW(
        lp_file_name: *const u16,
        dw_desired_access: u32,
        dw_share_mode: u32,
        lp_security_attributes: *mut core::ffi::c_void,
        dw_creation_disposition: u32,
        dw_flags_and_attributes: u32,
        h_template_file: *mut core::ffi::c_void,
    ) -> *mut core::ffi::c_void;
    fn CloseHandle(h_object: *mut core::ffi::c_void) -> i32;
    fn GetLastError() -> u32;
    fn ReplaceFileW(
        replaced_file_name: *const u16,
        replacement_file_name: *const u16,
        backup_file_name: *const u16,
        replace_flags: u32,
        exclude: *mut core::ffi::c_void,
        reserved: *mut core::ffi::c_void,
    ) -> i32;
    fn GetDiskFreeSpaceExW(
        directory_name: *const u16,
        free_bytes_available: *mut u64,
        total_number_of_bytes: *mut u64,
        total_number_of_free_bytes: *mut u64,
    ) -> i32;
}

impl LockProbe for WindowsLockProbe {
    #[cfg(windows)]
    fn is_locked(&self, path: &Path) -> bool {
        use std::os::windows::ffi::OsStrExt;

        const GENERIC_READ: u32 = 0x8000_0000;
        const OPEN_EXISTING: u32 = 3;
        // CreateFileW returns INVALID_HANDLE_VALUE (-1 as a pointer) on failure.
        let invalid_handle = usize::MAX as *mut core::ffi::c_void;

        // A wide, NUL-terminated UTF-16 copy of the path for the W (wide) API.
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();

        // SAFETY: `wide` is a valid NUL-terminated UTF-16 buffer that outlives the
        // call; the remaining args are the documented constants / null handles for
        // a read-only existence probe. On success we own the returned handle and
        // close it exactly once below; on failure no handle is produced.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ,
                0, // dwShareMode = 0: refuse to open if anyone else holds it
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };

        if handle == invalid_handle {
            // SAFETY: a pure thread-local Win32 error accessor, no pointers.
            let err = unsafe { GetLastError() };
            return err == ERROR_SHARING_VIOLATION;
        }

        // The file opened, so nobody else holds it: close our probe handle.
        // SAFETY: `handle` is a live handle CreateFileW just returned; closing it
        // exactly once is the matching teardown.
        unsafe { CloseHandle(handle) };
        false
    }

    /// On non-Windows the probe reports unlocked (the apply still fails safely if
    /// the underlying engine can't open the map).
    #[cfg(not(windows))]
    fn is_locked(&self, _path: &Path) -> bool {
        false
    }
}

/// The map-write service: runs the rail sequence and the rollback. Generic over
/// the injected collaborators so production wiring (Windows probe, bridge status,
/// isom engine) and tests (fakes) share the exact same logic.
pub struct MapSafe<S, L, E> {
    /// `%appdata%\eud-agent` — backups land under `<data_dir>/map_backups`.
    data_dir: PathBuf,
    status: S,
    lock_probe: L,
    engine: E,
}

impl<S, L, E> MapSafe<S, L, E>
where
    S: CompilingStatus,
    L: LockProbe,
    E: MapEngine,
{
    /// Construct the service from its data dir and the three collaborators.
    pub fn new(data_dir: PathBuf, status: S, lock_probe: L, engine: E) -> Self {
        Self {
            data_dir,
            status,
            lock_probe,
            engine,
        }
    }

    /// `<data_dir>/map_backups`.
    pub fn map_backups_dir(&self) -> PathBuf {
        self.data_dir.join("map_backups")
    }

    /// Apply ONE mutating map write IN PLACE, running every rail IN ORDER:
    /// compiling guard → lock probe → backup → apply → verify → journal.
    ///
    /// On success returns the [`JournalEntry`] (rail 6) recording the map and its
    /// backup so the write can later be rolled back. On a rail-1/rail-2 refusal NO
    /// backup is taken and the engine is NEVER called. On a rail-4 apply failure
    /// the on-disk map is untouched (the engine aborts before save), so no restore
    /// is performed — the error propagates. On a rail-5 verify failure the edit has
    /// ALREADY saved (the map may be corrupt), so the backup path is surfaced in
    /// [`MapSafeError::Verify`] for recovery — auto-restore is intentionally NOT
    /// done (it could overwrite forensic state and can itself fail).
    ///
    /// `kind` selects the isom op family for rail 4. `ops` is passed to the
    /// engine RAW (never re-encoded here).
    pub fn write(
        &self,
        map_path: &Path,
        kind: OpKind,
        ops: &[u8],
    ) -> Result<JournalEntry, MapSafeError> {
        // Rail 1 — compiling guard. Refuse BEFORE any backup/apply: writing the
        // map mid-build races the editor's read.
        if self.status.is_compiling() {
            return Err(MapSafeError::Compiling);
        }

        // Rail 2 — lock probe. Refuse while the map is open elsewhere (SCMDraft),
        // again BEFORE any backup/apply.
        if self.lock_probe.is_locked(map_path) {
            return Err(MapSafeError::MapLocked(map_path.to_path_buf()));
        }

        // Rail 3 — full-file backup BEFORE mutating (the rollback source).
        let backup_path = self.backup(map_path)?;

        // Rail 4 — all-or-nothing apply. The engine aborts BEFORE save on a bad op,
        // so on `Err` the on-disk map is untouched: no restore is needed, just
        // surface the error.
        self.engine
            .apply(map_path, kind, ops)
            .map_err(MapSafeError::Apply)?;

        // Rail 5 — re-digest verify. A digest failure after a successful save
        // signals corruption; surface the backup path so the caller can recover
        // (the edit already saved, so we do NOT auto-restore here).
        self.engine
            .digest(map_path)
            .map_err(|detail| MapSafeError::Verify {
                detail,
                backup: backup_path.clone(),
            })?;

        // Rail 6 — journal entry: the {map, backup} bookkeeping the reject path
        // needs to roll this write back.
        Ok(JournalEntry {
            map_path: map_path.to_path_buf(),
            backup_path,
        })
    }

    /// Roll one write back (rail 7): copy the backed-up bytes over the map via a
    /// temp file + atomic rename. Refuses while the map is locked (the SAME probe
    /// as rail 2) and errors if the backup file is gone.
    pub fn restore(&self, entry: &JournalEntry) -> Result<(), MapSafeError> {
        if !entry.backup_path.is_file() {
            return Err(MapSafeError::BackupNotFound(entry.backup_path.clone()));
        }
        if self.lock_probe.is_locked(&entry.map_path) {
            return Err(MapSafeError::MapLocked(entry.map_path.clone()));
        }

        // Stage, flush, and replace in the map's own directory. On Windows this
        // uses MoveFileExW(REPLACE_EXISTING|WRITE_THROUGH), not fs::rename.
        let bytes = std::fs::read(&entry.backup_path)?;
        atomic_replace_bytes(&entry.map_path, &bytes)?;
        Ok(())
    }

    /// Rail 3 — copy the whole map to
    /// `<data_dir>/map_backups/<mapname>.<timestamp>.bak` (creating the dir if
    /// needed) and return the backup path. Called BEFORE the apply.
    fn backup(&self, map_path: &Path) -> Result<PathBuf, MapSafeError> {
        let backup_dir = self.map_backups_dir();
        std::fs::create_dir_all(&backup_dir)?;

        let map_name = map_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "map".to_string());
        let stamp = backup_timestamp();
        let backup_path = backup_dir.join(format!("{map_name}.{stamp}.bak"));

        std::fs::copy(map_path, &backup_path)?;
        Ok(backup_path)
    }
}

impl<S, L, E> MapSafe<S, L, E>
where
    S: CompilingStatus,
    L: LockProbe,
    E: MapEngine + SoundMapEngine,
{
    pub fn write_sound(
        &self,
        map_path: &Path,
        expected_input_sha256: &str,
        destination_mpq_path: &str,
        ogg_bytes: &[u8],
    ) -> Result<SoundJournalEntry, MapSafeError> {
        if self.status.is_compiling() {
            return Err(MapSafeError::Compiling);
        }
        if self.lock_probe.is_locked(map_path) {
            return Err(MapSafeError::MapLocked(map_path.to_path_buf()));
        }
        let actual_input_sha256 = sha256_file(map_path)?;
        if actual_input_sha256 != expected_input_sha256 {
            return Err(MapSafeError::StaleSource {
                expected: expected_input_sha256.to_string(),
                actual: actual_input_sha256,
            });
        }
        let map_bytes_before = std::fs::metadata(map_path)?.len();
        ensure_sound_disk_space(map_path, map_bytes_before, ogg_bytes.len() as u64)?;
        let backup_path = self.backup(map_path)?;
        if sha256_file(&backup_path)? != expected_input_sha256 {
            return Err(MapSafeError::Apply(
                "full map backup hash does not match source".to_string(),
            ));
        }
        let parent = map_path.parent().ok_or_else(|| {
            MapSafeError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "map path has no parent",
            ))
        })?;
        let stem = map_path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "map".to_string());
        let extension = map_path
            .extension()
            .map(|extension| extension.to_string_lossy().into_owned())
            .unwrap_or_else(|| "scx".to_string());
        let native_output =
            parent.join(format!(".{stem}.{}.audio.{extension}", backup_timestamp()));
        let report = match self.engine.add_sound(
            map_path,
            &native_output,
            expected_input_sha256,
            destination_mpq_path,
            ogg_bytes,
        ) {
            Ok(report) => report,
            Err(error) => {
                let _ = std::fs::remove_file(&native_output);
                return Err(MapSafeError::Apply(error));
            }
        };
        if report.input_sha256 != expected_input_sha256
            || report.mpq_path != destination_mpq_path
            || sha256_file(&native_output)? != report.output_sha256
        {
            let _ = std::fs::remove_file(&native_output);
            return Err(MapSafeError::Apply(
                "native sound report/output invariant mismatch".to_string(),
            ));
        }
        if report.reused {
            let _ = std::fs::remove_file(&native_output);
            let _ = std::fs::remove_file(&backup_path);
            return Ok(SoundJournalEntry {
                map_path: map_path.to_path_buf(),
                backup_path,
                report,
                map_bytes_before,
                map_bytes_after: map_bytes_before,
            });
        }

        if let Err(error) = atomic_replace_file(map_path, &native_output) {
            let _ = std::fs::remove_file(&native_output);
            return Err(MapSafeError::Io(error));
        }
        let map_bytes_after = std::fs::metadata(map_path)?.len();
        let post_verify = (|| {
            let actual = sha256_file(map_path).map_err(|error| error.to_string())?;
            if actual != report.output_sha256 {
                return Err("atomic replacement output hash changed".to_string());
            }
            self.engine.verify_sound(
                map_path,
                destination_mpq_path,
                &report.asset_sha256,
                report.sound_index,
                report.sound_string_id,
            )
        })();
        if let Err(detail) = post_verify {
            if self.lock_probe.is_locked(map_path) {
                return Err(MapSafeError::Rollback {
                    detail: format!("{detail}; rollback blocked by map lock"),
                    backup: backup_path,
                });
            }
            let restore = (|| {
                let bytes = std::fs::read(&backup_path)?;
                atomic_replace_bytes(map_path, &bytes)?;
                let restored = sha256_file(map_path)?;
                if restored != expected_input_sha256 {
                    return Err(std::io::Error::other(
                        "restored map hash does not match exact before state",
                    ));
                }
                Ok::<(), std::io::Error>(())
            })();
            return match restore {
                Ok(()) => Err(MapSafeError::PostVerifyRestored {
                    detail,
                    backup: backup_path,
                }),
                Err(restore_error) => Err(MapSafeError::Rollback {
                    detail: format!("{detail}; rollback: {restore_error}"),
                    backup: backup_path,
                }),
            };
        }
        Ok(SoundJournalEntry {
            map_path: map_path.to_path_buf(),
            backup_path,
            report,
            map_bytes_before,
            map_bytes_after,
        })
    }

    pub fn replace_sound(
        &self,
        map_path: &Path,
        expected_input_sha256: &str,
        old_mpq_path: &str,
        destination_mpq_path: &str,
        ogg_bytes: &[u8],
    ) -> Result<SoundReplaceJournalEntry, MapSafeError> {
        if self.status.is_compiling() {
            return Err(MapSafeError::Compiling);
        }
        if self.lock_probe.is_locked(map_path) {
            return Err(MapSafeError::MapLocked(map_path.to_path_buf()));
        }
        let actual_input_sha256 = sha256_file(map_path)?;
        if actual_input_sha256 != expected_input_sha256 {
            return Err(MapSafeError::StaleSource {
                expected: expected_input_sha256.to_string(),
                actual: actual_input_sha256,
            });
        }
        let map_bytes_before = std::fs::metadata(map_path)?.len();
        ensure_sound_disk_space(map_path, map_bytes_before, ogg_bytes.len() as u64)?;
        let backup_path = self.backup(map_path)?;
        if sha256_file(&backup_path)? != expected_input_sha256 {
            return Err(MapSafeError::Apply(
                "full map backup hash does not match source".to_string(),
            ));
        }
        let parent = map_path.parent().ok_or_else(|| {
            MapSafeError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "map path has no parent",
            ))
        })?;
        let stem = map_path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "map".to_string());
        let extension = map_path
            .extension()
            .map(|extension| extension.to_string_lossy().into_owned())
            .unwrap_or_else(|| "scx".to_string());
        let native_output = parent.join(format!(
            ".{stem}.{}.audio-replace.{extension}",
            backup_timestamp()
        ));
        let report = match self.engine.replace_sound(
            map_path,
            &native_output,
            expected_input_sha256,
            old_mpq_path,
            destination_mpq_path,
            ogg_bytes,
        ) {
            Ok(report) => report,
            Err(error) => {
                let _ = std::fs::remove_file(&native_output);
                return Err(MapSafeError::Apply(error));
            }
        };
        if report.input_sha256 != expected_input_sha256
            || report.old_mpq_path != old_mpq_path
            || report.mpq_path != destination_mpq_path
            || sha256_file(&native_output)? != report.output_sha256
        {
            let _ = std::fs::remove_file(&native_output);
            return Err(MapSafeError::Apply(
                "native sound replacement report/output invariant mismatch".to_string(),
            ));
        }
        if let Err(error) = atomic_replace_file(map_path, &native_output) {
            let _ = std::fs::remove_file(&native_output);
            return Err(MapSafeError::Io(error));
        }
        let map_bytes_after = std::fs::metadata(map_path)?.len();
        let post_verify = (|| {
            let actual = sha256_file(map_path).map_err(|error| error.to_string())?;
            if actual != report.output_sha256 {
                return Err("atomic replacement output hash changed".to_string());
            }
            self.engine.verify_sound_replacement(
                map_path,
                old_mpq_path,
                destination_mpq_path,
                &report.asset_sha256,
                report.sound_index,
                report.sound_string_id,
            )
        })();
        if let Err(detail) = post_verify {
            if self.lock_probe.is_locked(map_path) {
                return Err(MapSafeError::Rollback {
                    detail: format!("{detail}; rollback blocked by map lock"),
                    backup: backup_path,
                });
            }
            let restore = (|| {
                let bytes = std::fs::read(&backup_path)?;
                atomic_replace_bytes(map_path, &bytes)?;
                let restored = sha256_file(map_path)?;
                if restored != expected_input_sha256 {
                    return Err(std::io::Error::other(
                        "restored map hash does not match exact before state",
                    ));
                }
                Ok::<(), std::io::Error>(())
            })();
            return match restore {
                Ok(()) => Err(MapSafeError::PostVerifyRestored {
                    detail,
                    backup: backup_path,
                }),
                Err(restore_error) => Err(MapSafeError::Rollback {
                    detail: format!("{detail}; rollback: {restore_error}"),
                    backup: backup_path,
                }),
            };
        }
        Ok(SoundReplaceJournalEntry {
            map_path: map_path.to_path_buf(),
            backup_path,
            report,
            map_bytes_before,
            map_bytes_after,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateApplyRecord {
    pub source_path: PathBuf,
    pub backup_path: PathBuf,
    pub before_sha256: String,
    pub applied_sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CandidateApplyError {
    #[error("the editor is compiling; candidate Apply is blocked")]
    Compiling,
    #[error("map file is open in another program: {0}")]
    MapLocked(PathBuf),
    #[error("source map changed after candidate creation")]
    StaleSource,
    #[error("candidate bytes changed after verification")]
    CandidateChanged,
    #[error("candidate or applied source is not a parseable SCX: {0}")]
    Verify(String),
    #[error("candidate Apply I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("candidate Apply failed and backup restore also failed: {0}")]
    RestoreFailed(String),
    #[error("last Apply cannot be undone because the source changed again")]
    UndoSourceChanged,
    #[error("Apply backup hash does not match its journal record")]
    BackupChanged,
}

pub struct CandidateMapSafe<S, L> {
    backup_dir: PathBuf,
    status: S,
    lock_probe: L,
}

impl<S, L> CandidateMapSafe<S, L>
where
    S: CompilingStatus,
    L: LockProbe,
{
    pub fn new(backup_dir: PathBuf, status: S, lock_probe: L) -> Self {
        Self {
            backup_dir,
            status,
            lock_probe,
        }
    }

    pub fn apply(
        &self,
        source: &Path,
        candidate: &Path,
        expected_source_sha256: &str,
        expected_candidate_sha256: &str,
    ) -> Result<CandidateApplyRecord, CandidateApplyError> {
        self.apply_with_post_verify(
            source,
            candidate,
            expected_source_sha256,
            expected_candidate_sha256,
            |_| Ok(()),
        )
    }

    fn apply_with_post_verify<F>(
        &self,
        source: &Path,
        candidate: &Path,
        expected_source_sha256: &str,
        expected_candidate_sha256: &str,
        post_verify: F,
    ) -> Result<CandidateApplyRecord, CandidateApplyError>
    where
        F: Fn(&Path) -> Result<(), CandidateApplyError>,
    {
        self.guard(source)?;
        let source_sha256 = sha256_file(source)?;
        if source_sha256 != expected_source_sha256 {
            return Err(CandidateApplyError::StaleSource);
        }
        let candidate_bytes = std::fs::read(candidate)?;
        let candidate_sha256 = sha256_bytes(&candidate_bytes);
        if candidate_sha256 != expected_candidate_sha256 {
            return Err(CandidateApplyError::CandidateChanged);
        }
        isom::chk_extract(candidate)
            .map_err(|error| CandidateApplyError::Verify(error.to_string()))?;

        std::fs::create_dir_all(&self.backup_dir)?;
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "map.scx".to_string());
        let backup_path = self
            .backup_dir
            .join(format!("{name}.{}.apply.bak", backup_timestamp()));
        let source_bytes = std::fs::read(source)?;
        crate::memory::write_atomic_bytes(&backup_path, &source_bytes)
            .map_err(|error| CandidateApplyError::Io(std::io::Error::other(error.to_string())))?;
        sync_file(&backup_path)?;
        let pending_record = CandidateApplyRecord {
            source_path: source.to_path_buf(),
            backup_path: backup_path.clone(),
            before_sha256: source_sha256.clone(),
            applied_sha256: candidate_sha256.clone(),
        };
        let pending_path = pending_apply_path(&backup_path);
        let pending_bytes = serde_json::to_vec(&pending_record).map_err(|error| {
            CandidateApplyError::Io(std::io::Error::other(format!(
                "pending Apply journal serialization failed: {error}"
            )))
        })?;
        crate::memory::write_atomic_bytes(&pending_path, &pending_bytes)
            .map_err(|error| CandidateApplyError::Io(std::io::Error::other(error.to_string())))?;
        sync_file(&pending_path)?;

        if let Err(error) = atomic_replace_bytes(source, &candidate_bytes) {
            let _ = std::fs::remove_file(&pending_path);
            return Err(CandidateApplyError::Io(error));
        }
        let post_result = (|| {
            let applied_sha256 = sha256_file(source)?;
            if applied_sha256 != candidate_sha256 {
                return Err(CandidateApplyError::Verify(
                    "post-Apply source bytes differ from candidate".to_string(),
                ));
            }
            isom::chk_extract(source)
                .map_err(|error| CandidateApplyError::Verify(error.to_string()))?;
            post_verify(source)?;
            Ok(applied_sha256)
        })();
        let applied_sha256 = match post_result {
            Ok(hash) => hash,
            Err(error) => {
                let backup = std::fs::read(&backup_path).map_err(|restore| {
                    CandidateApplyError::RestoreFailed(format!(
                        "{error}; backup read failed: {restore}"
                    ))
                })?;
                atomic_replace_bytes(source, &backup).map_err(|restore| {
                    CandidateApplyError::RestoreFailed(format!(
                        "{error}; atomic backup restore failed: {restore}"
                    ))
                })?;
                if sha256_file(source).map_err(|restore| {
                    CandidateApplyError::RestoreFailed(format!(
                        "{error}; restored source hash failed: {restore}"
                    ))
                })? != source_sha256
                {
                    return Err(CandidateApplyError::RestoreFailed(format!(
                        "{error}; restored bytes do not match original"
                    )));
                }
                let _ = std::fs::remove_file(&pending_path);
                return Err(error);
            }
        };
        Ok(CandidateApplyRecord {
            applied_sha256,
            ..pending_record
        })
    }

    pub fn complete_pending(
        &self,
        record: &CandidateApplyRecord,
    ) -> Result<(), CandidateApplyError> {
        let path = pending_apply_path(&record.backup_path);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn undo(&self, record: &CandidateApplyRecord) -> Result<(), CandidateApplyError> {
        self.guard(&record.source_path)?;
        if sha256_file(&record.source_path)? != record.applied_sha256 {
            return Err(CandidateApplyError::UndoSourceChanged);
        }
        let backup = std::fs::read(&record.backup_path)?;
        if sha256_bytes(&backup) != record.before_sha256 {
            return Err(CandidateApplyError::BackupChanged);
        }
        atomic_replace_bytes(&record.source_path, &backup)?;
        if sha256_file(&record.source_path)? != record.before_sha256 {
            return Err(CandidateApplyError::RestoreFailed(
                "undo source bytes do not match the exact backup".to_string(),
            ));
        }
        isom::chk_extract(&record.source_path)
            .map_err(|error| CandidateApplyError::Verify(error.to_string()))?;
        self.complete_pending(record)?;
        Ok(())
    }

    fn guard(&self, source: &Path) -> Result<(), CandidateApplyError> {
        if self.status.is_compiling() {
            return Err(CandidateApplyError::Compiling);
        }
        if self.lock_probe.is_locked(source) {
            return Err(CandidateApplyError::MapLocked(source.to_path_buf()));
        }
        Ok(())
    }
}

fn pending_apply_path(backup_path: &Path) -> PathBuf {
    let mut name = backup_path.file_name().unwrap_or_default().to_os_string();
    name.push(".pending.json");
    backup_path.with_file_name(name)
}

fn candidate_state_committed(candidate_root: &Path, backup_path: &Path) -> bool {
    let Ok(projects) = std::fs::read_dir(candidate_root) else {
        return false;
    };
    for project in projects.flatten() {
        let Ok(sessions) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for session in sessions.flatten() {
            let state_path = session.path().join("state.json");
            let Ok(bytes) = std::fs::read(state_path) else {
                continue;
            };
            let Ok(state) = serde_json::from_slice::<crate::map_model::CandidateSession>(&bytes)
            else {
                continue;
            };
            if state.last_apply_backup.as_deref() == Some(backup_path) {
                return true;
            }
        }
    }
    false
}

pub fn recover_pending_candidate_applies(
    backup_dir: &Path,
    candidate_root: &Path,
) -> Result<usize, String> {
    if !backup_dir.is_dir() {
        return Ok(0);
    }
    let entries = std::fs::read_dir(backup_dir)
        .map_err(|error| format!("pending Apply journals could not be inspected: {error}"))?;
    let mut recovered = 0;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".apply.bak.pending.json"))
        {
            continue;
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("pending Apply journal could not be read: {error}"))?;
        let record: CandidateApplyRecord = serde_json::from_slice(&bytes)
            .map_err(|error| format!("pending Apply journal is invalid: {error}"))?;
        if candidate_state_committed(candidate_root, &record.backup_path) {
            std::fs::remove_file(&path).map_err(|error| {
                format!("committed Apply journal could not be cleared: {error}")
            })?;
            continue;
        }
        let current_hash = sha256_file(&record.source_path)
            .map_err(|error| format!("pending Apply source could not be read: {error}"))?;
        if current_hash == record.applied_sha256 {
            if WindowsLockProbe.is_locked(&record.source_path) {
                return Err(format!(
                    "pending Apply recovery is blocked because the source map is open: {}",
                    record.source_path.display()
                ));
            }
            let backup = std::fs::read(&record.backup_path)
                .map_err(|error| format!("pending Apply backup could not be read: {error}"))?;
            if sha256_bytes(&backup) != record.before_sha256 {
                return Err("pending Apply backup hash does not match its journal".to_string());
            }
            atomic_replace_bytes(&record.source_path, &backup)
                .map_err(|error| format!("pending Apply backup restore failed: {error}"))?;
            if sha256_file(&record.source_path)
                .map_err(|error| format!("restored source could not be hashed: {error}"))?
                != record.before_sha256
            {
                return Err("pending Apply recovery did not restore exact source bytes".to_string());
            }
            recovered += 1;
        } else if current_hash != record.before_sha256 {
            return Err(format!(
                "pending Apply source changed independently: {}",
                record.source_path.display()
            ));
        }
        std::fs::remove_file(&path)
            .map_err(|error| format!("pending Apply journal could not be cleared: {error}"))?;
    }
    Ok(recovered)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn sync_file(path: &Path) -> Result<(), std::io::Error> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()
}

fn ensure_sound_disk_space(
    map_path: &Path,
    map_bytes: u64,
    ogg_bytes: u64,
) -> Result<(), MapSafeError> {
    let required = map_bytes
        .checked_mul(3)
        .and_then(|value| {
            ogg_bytes
                .checked_mul(2)
                .and_then(|ogg| value.checked_add(ogg))
        })
        .and_then(|value| value.checked_add(16 * 1024 * 1024))
        .ok_or_else(|| {
            MapSafeError::Apply("sound import disk-space calculation overflow".to_string())
        })?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let parent = map_path.parent().ok_or_else(|| {
            MapSafeError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "map path has no parent directory",
            ))
        })?;
        let directory = parent
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut available = 0_u64;
        // SAFETY: `directory` is NUL-terminated and `available` is a valid out
        // pointer. The other documented output pointers are optional.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                directory.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(MapSafeError::Io(std::io::Error::last_os_error()));
        }
        if available < required {
            return Err(MapSafeError::InsufficientDisk {
                required,
                available,
            });
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (map_path, required);
    }
    Ok(())
}

fn atomic_replace_file(destination: &Path, temporary: &Path) -> Result<(), std::io::Error> {
    sync_file(temporary)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        const REPLACEFILE_WRITE_THROUGH: u32 = 0x1;
        let temporary = temporary
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let destination = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        // SAFETY: both paths are valid NUL-terminated UTF-16 for the
        // synchronous same-volume ReplaceFileW operation.
        let replaced = unsafe {
            ReplaceFileW(
                destination.as_ptr(),
                temporary.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_WRITE_THROUGH,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if replaced == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(temporary, destination)?;
    }
    Ok(())
}

fn atomic_replace_bytes(destination: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let parent = destination.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "map destination has no parent directory",
        )
    })?;
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "map.scx".to_string());
    let temporary = parent.join(format!(".{name}.{}.apply.tmp", backup_timestamp()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        const REPLACEFILE_WRITE_THROUGH: u32 = 0x1;
        let existing = temporary
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let destination = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        // SAFETY: both vectors are valid NUL-terminated UTF-16 paths for the
        // synchronous ReplaceFileW call. The backup is already app-managed.
        let moved = unsafe {
            ReplaceFileW(
                destination.as_ptr(),
                existing.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_WRITE_THROUGH,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if moved == 0 {
            let error = std::io::Error::last_os_error();
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(&temporary, destination)?;
    }
    Ok(())
}

/// A filesystem-safe, monotonic-ish timestamp for backup filenames. Nanoseconds
/// since the epoch keeps two backups of the same map within the same second
/// distinct (the Python original used `%Y%m%d-%H%M%S-%f`; this is the same intent
/// without a date-formatting dependency).
fn backup_timestamp() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs;

    // ---------------------------------------------------------------- fakes

    /// Fake compiling-status source: returns a fixed flag.
    struct FakeStatus(bool);
    impl CompilingStatus for FakeStatus {
        fn is_compiling(&self) -> bool {
            self.0
        }
    }

    /// Fake lock probe: returns a fixed flag for every path.
    struct FakeLock(bool);
    impl LockProbe for FakeLock {
        fn is_locked(&self, _path: &Path) -> bool {
            self.0
        }
    }

    /// Fake engine: records whether `apply` ran and how the map should change, so
    /// tests can assert apply happened (or did NOT) and inspect verify behavior.
    struct FakeEngine {
        /// `Ok` ⇒ apply succeeds (and writes `applied_bytes` to the map, like an
        /// in-place save); `Err(msg)` ⇒ apply aborts before save (map untouched).
        apply_result: Result<(), String>,
        /// Bytes the successful apply writes over the map (the "edited" map).
        applied_bytes: Vec<u8>,
        /// `Ok` ⇒ verify passes; `Err(msg)` ⇒ verify (re-digest) fails.
        digest_result: Result<Vec<u8>, String>,
        /// Set true the moment `apply` is invoked (rail-ordering assertions).
        apply_called: Cell<bool>,
        last_kind: Cell<Option<OpKind>>,
    }

    impl FakeEngine {
        fn ok(applied_bytes: &[u8]) -> Self {
            Self {
                apply_result: Ok(()),
                applied_bytes: applied_bytes.to_vec(),
                digest_result: Ok(vec![0xDE, 0xAD]),
                apply_called: Cell::new(false),
                last_kind: Cell::new(None),
            }
        }

        /// Apply aborts before save (bad op) — the map is left untouched.
        fn apply_fails() -> Self {
            Self {
                apply_result: Err("bad op #3".into()),
                applied_bytes: Vec::new(),
                digest_result: Ok(vec![0xDE, 0xAD]),
                apply_called: Cell::new(false),
                last_kind: Cell::new(None),
            }
        }

        /// Apply succeeds but the post-write re-digest fails (corruption signal).
        fn verify_fails(applied_bytes: &[u8]) -> Self {
            Self {
                apply_result: Ok(()),
                applied_bytes: applied_bytes.to_vec(),
                digest_result: Err("unreadable CHK".into()),
                apply_called: Cell::new(false),
                last_kind: Cell::new(None),
            }
        }
    }

    impl MapEngine for FakeEngine {
        fn apply(&self, map: &Path, kind: OpKind, _ops: &[u8]) -> Result<(), String> {
            self.apply_called.set(true);
            self.last_kind.set(Some(kind));
            self.apply_result.clone()?;
            // A successful in-place save replaces the map bytes.
            fs::write(map, &self.applied_bytes).unwrap();
            Ok(())
        }

        fn digest(&self, _map: &Path) -> Result<Vec<u8>, String> {
            self.digest_result.clone()
        }
    }

    struct FakeSoundEngine {
        add_result: Result<(), String>,
        verify_result: Result<(), String>,
        output_bytes: Vec<u8>,
        reused: bool,
        add_called: Cell<bool>,
        backup_seen: Cell<bool>,
    }

    impl FakeSoundEngine {
        fn ok(output_bytes: &[u8]) -> Self {
            Self {
                add_result: Ok(()),
                verify_result: Ok(()),
                output_bytes: output_bytes.to_vec(),
                reused: false,
                add_called: Cell::new(false),
                backup_seen: Cell::new(false),
            }
        }

        fn add_fails() -> Self {
            Self {
                add_result: Err("native sound failure".to_string()),
                ..Self::ok(EDITED)
            }
        }

        fn verify_fails(output_bytes: &[u8]) -> Self {
            Self {
                verify_result: Err("post verify failure".to_string()),
                ..Self::ok(output_bytes)
            }
        }

        fn reused() -> Self {
            Self {
                reused: true,
                ..Self::ok(ORIGINAL)
            }
        }
    }

    impl MapEngine for FakeSoundEngine {
        fn apply(&self, _map: &Path, _kind: OpKind, _ops: &[u8]) -> Result<(), String> {
            unreachable!("sound tests do not use in-place map ops")
        }

        fn digest(&self, _map: &Path) -> Result<Vec<u8>, String> {
            unreachable!("sound tests use verify_sound")
        }
    }

    impl SoundMapEngine for FakeSoundEngine {
        fn add_sound(
            &self,
            input: &Path,
            output: &Path,
            expected_input_sha256: &str,
            destination_mpq_path: &str,
            ogg_bytes: &[u8],
        ) -> Result<isom::MapSoundAddReport, String> {
            self.add_called.set(true);
            self.backup_seen.set(
                input
                    .parent()
                    .unwrap()
                    .join("map_backups")
                    .read_dir()
                    .is_ok_and(|mut entries| entries.next().is_some()),
            );
            self.add_result.clone()?;
            let output_bytes = if self.reused {
                fs::read(input).unwrap()
            } else {
                self.output_bytes.clone()
            };
            fs::write(output, &output_bytes).unwrap();
            let digest = sha256_bytes(&output_bytes);
            let stable = "a".repeat(64);
            Ok(isom::MapSoundAddReport {
                schema: "eud-map-sound-add-report/1".to_string(),
                ok: true,
                reused: self.reused,
                sound_index: 7,
                sound_string_id: 42,
                mpq_path: destination_mpq_path.to_string(),
                asset_sha256: sha256_bytes(ogg_bytes),
                asset_bytes: ogg_bytes.len() as u64,
                input_sha256: expected_input_sha256.to_string(),
                output_sha256: digest,
                unrelated_chk_digest_before: stable.clone(),
                unrelated_chk_digest_after: stable.clone(),
                unrelated_asset_digest_before: stable.clone(),
                unrelated_asset_digest_after: stable,
            })
        }

        fn replace_sound(
            &self,
            input: &Path,
            output: &Path,
            expected_input_sha256: &str,
            old_mpq_path: &str,
            destination_mpq_path: &str,
            ogg_bytes: &[u8],
        ) -> Result<isom::MapSoundReplaceReport, String> {
            self.add_called.set(true);
            self.backup_seen.set(
                input
                    .parent()
                    .unwrap()
                    .join("map_backups")
                    .read_dir()
                    .is_ok_and(|mut entries| entries.next().is_some()),
            );
            self.add_result.clone()?;
            fs::write(output, &self.output_bytes).unwrap();
            let digest = sha256_bytes(&self.output_bytes);
            let stable = "a".repeat(64);
            Ok(isom::MapSoundReplaceReport {
                schema: "eud-map-sound-replace-report/1".to_string(),
                ok: true,
                sound_index: 7,
                sound_string_id: 42,
                old_mpq_path: old_mpq_path.to_string(),
                mpq_path: destination_mpq_path.to_string(),
                asset_sha256: sha256_bytes(ogg_bytes),
                asset_bytes: ogg_bytes.len() as u64,
                input_sha256: expected_input_sha256.to_string(),
                output_sha256: digest,
                unrelated_chk_digest_before: stable.clone(),
                unrelated_chk_digest_after: stable.clone(),
                unrelated_asset_digest_before: stable.clone(),
                unrelated_asset_digest_after: stable,
            })
        }

        fn verify_sound(
            &self,
            _map: &Path,
            _destination_mpq_path: &str,
            _normalized_sha256: &str,
            _sound_index: u64,
            _sound_string_id: u64,
        ) -> Result<(), String> {
            self.verify_result.clone()
        }

        fn verify_sound_replacement(
            &self,
            _map: &Path,
            _old_mpq_path: &str,
            _destination_mpq_path: &str,
            _normalized_sha256: &str,
            _sound_index: u64,
            _sound_string_id: u64,
        ) -> Result<(), String> {
            self.verify_result.clone()
        }
    }

    struct SequenceLock {
        calls: Cell<usize>,
        lock_on_call: Option<usize>,
    }

    impl SequenceLock {
        fn never() -> Self {
            Self {
                calls: Cell::new(0),
                lock_on_call: None,
            }
        }

        fn on(call: usize) -> Self {
            Self {
                calls: Cell::new(0),
                lock_on_call: Some(call),
            }
        }
    }

    impl LockProbe for SequenceLock {
        fn is_locked(&self, _path: &Path) -> bool {
            let call = self.calls.get() + 1;
            self.calls.set(call);
            self.lock_on_call == Some(call)
        }
    }

    // ------------------------------------------------------------- helpers

    /// Unique temp base dir for a test, avoiding a `tempfile` dev-dependency
    /// (Cargo.toml is out of scope for this task — same precedent as config.rs).
    fn unique_temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("eud-agent-mapsafe-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Create a fake map file with the given bytes, return its path.
    fn make_map(base: &Path, contents: &[u8]) -> PathBuf {
        let map = base.join("demo.scx");
        fs::write(&map, contents).unwrap();
        map
    }

    const ORIGINAL: &[u8] = b"ORIGINAL-MAP-BYTES";
    const EDITED: &[u8] = b"EDITED-MAP-BYTES-after-apply";
    const OPS: &[u8] = b"add|0|0|10|10|spot";

    // ------------------------------------------------------------- rail 1

    #[test]
    fn compiling_guard_refuses_and_skips_backup_and_apply() {
        let base = unique_temp_dir("compiling");
        let map = make_map(&base, ORIGINAL);

        let engine = FakeEngine::ok(EDITED);
        let svc = MapSafe::new(base.clone(), FakeStatus(true), FakeLock(false), engine);

        let err = svc
            .write(&map, OpKind::Locedit, OPS)
            .expect_err("must refuse while compiling");
        assert!(matches!(err, MapSafeError::Compiling));

        // No backup directory / file should have been created, the engine never ran,
        // and the map is byte-for-byte unchanged.
        assert!(
            !svc.map_backups_dir().exists()
                || fs::read_dir(svc.map_backups_dir())
                    .unwrap()
                    .next()
                    .is_none(),
            "no backup must be taken when the compiling guard refuses"
        );
        assert_eq!(fs::read(&map).unwrap(), ORIGINAL);

        fs::remove_dir_all(&base).ok();
    }

    // ------------------------------------------------------------- rail 2

    #[test]
    fn lock_probe_refuses_and_skips_backup_and_apply() {
        let base = unique_temp_dir("locked");
        let map = make_map(&base, ORIGINAL);

        let svc = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            FakeLock(true),
            FakeEngine::ok(EDITED),
        );

        let err = svc
            .write(&map, OpKind::Locedit, OPS)
            .expect_err("must refuse while locked");
        assert!(matches!(err, MapSafeError::MapLocked(p) if p == map));

        assert!(
            !svc.map_backups_dir().exists()
                || fs::read_dir(svc.map_backups_dir())
                    .unwrap()
                    .next()
                    .is_none(),
            "no backup must be taken when the lock probe refuses"
        );
        assert_eq!(fs::read(&map).unwrap(), ORIGINAL);

        fs::remove_dir_all(&base).ok();
    }

    // ------------------------------------- rail 3 + 4 + 5 + 6 (happy path)

    #[test]
    fn happy_path_backs_up_applies_verifies_and_journals() {
        let base = unique_temp_dir("happy");
        let map = make_map(&base, ORIGINAL);

        let svc = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            FakeLock(false),
            FakeEngine::ok(EDITED),
        );

        let entry = svc
            .write(&map, OpKind::Locedit, OPS)
            .expect("happy path must succeed");

        // Rail 6: journal entry points at the map + its backup.
        assert_eq!(entry.map_path, map);
        assert_eq!(entry.backup_path.parent().unwrap(), svc.map_backups_dir());

        // Rail 3: the backup exists, under <data_dir>/map_backups, named <map>.*.bak,
        // and holds the ORIGINAL (pre-edit) bytes.
        assert!(entry.backup_path.is_file(), "backup file must exist");
        let bak_name = entry.backup_path.file_name().unwrap().to_string_lossy();
        assert!(
            bak_name.starts_with("demo.scx."),
            "backup keeps the map name"
        );
        assert!(bak_name.ends_with(".bak"), "backup uses the .bak suffix");
        assert_eq!(
            fs::read(&entry.backup_path).unwrap(),
            ORIGINAL,
            "backup must snapshot the pre-edit bytes"
        );

        // Rail 4: the apply ran and saved the edited bytes in place.
        assert_eq!(fs::read(&map).unwrap(), EDITED);

        fs::remove_dir_all(&base).ok();
    }

    // ------------------------------------------------------------- rail 4 fail

    #[test]
    fn apply_failure_leaves_map_untouched_and_no_restore_needed() {
        let base = unique_temp_dir("applyfail");
        let map = make_map(&base, ORIGINAL);

        let svc = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            FakeLock(false),
            FakeEngine::apply_fails(),
        );

        let err = svc
            .write(&map, OpKind::Locedit, OPS)
            .expect_err("apply failure must surface");
        assert!(matches!(err, MapSafeError::Apply(_)));

        // The engine aborts before save, so the on-disk map is the ORIGINAL —
        // no restore is needed (and the backup, if taken, is harmless).
        assert_eq!(fs::read(&map).unwrap(), ORIGINAL);

        fs::remove_dir_all(&base).ok();
    }

    // ------------------------------------------------------------- rail 5 fail

    #[test]
    fn verify_failure_surfaces_after_apply() {
        let base = unique_temp_dir("verifyfail");
        let map = make_map(&base, ORIGINAL);

        let svc = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            FakeLock(false),
            FakeEngine::verify_fails(EDITED),
        );

        let err = svc
            .write(&map, OpKind::Locedit, OPS)
            .expect_err("verify failure must surface");

        // The verify error must surface the backup path so recovery is possible:
        // the caller can reconstruct a JournalEntry and restore, or inspect it.
        let backup = match err {
            MapSafeError::Verify { backup, .. } => backup,
            other => panic!("expected Verify, got {other:?}"),
        };
        assert!(
            backup.is_file(),
            "verify failure must surface an existing backup"
        );
        assert_eq!(
            fs::read(&backup).unwrap(),
            ORIGINAL,
            "the surfaced backup must hold the pre-edit bytes (recovery is possible)"
        );

        fs::remove_dir_all(&base).ok();
    }

    // ------------------------------------------------------------- rail 7

    #[test]
    fn restore_brings_back_exact_original_bytes() {
        let base = unique_temp_dir("restore");
        let map = make_map(&base, ORIGINAL);

        let svc = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            FakeLock(false),
            FakeEngine::ok(EDITED),
        );

        // Apply: map now holds EDITED, journal points at the ORIGINAL backup.
        let entry = svc
            .write(&map, OpKind::Locedit, OPS)
            .expect("write must succeed");
        assert_eq!(fs::read(&map).unwrap(), EDITED);

        // Roll back: the map is restored byte-for-byte to ORIGINAL.
        svc.restore(&entry).expect("restore must succeed");
        assert_eq!(
            fs::read(&map).unwrap(),
            ORIGINAL,
            "rollback must restore the exact original bytes"
        );

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn write_routes_opkind_to_engine() {
        let base = unique_temp_dir("opkind-playeredit");
        let map = make_map(&base, ORIGINAL);
        let svc = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            FakeLock(false),
            FakeEngine::ok(EDITED),
        );
        svc.write(&map, OpKind::PlayerEdit, OPS)
            .expect("write must succeed");
        assert_eq!(svc.engine.last_kind.get(), Some(OpKind::PlayerEdit));
        fs::remove_dir_all(&base).ok();

        let base = unique_temp_dir("opkind-locedit");
        let map = make_map(&base, ORIGINAL);
        let svc = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            FakeLock(false),
            FakeEngine::ok(EDITED),
        );
        svc.write(&map, OpKind::Locedit, OPS)
            .expect("write must succeed");
        assert_eq!(svc.engine.last_kind.get(), Some(OpKind::Locedit));
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn restore_refuses_while_map_locked() {
        let base = unique_temp_dir("restore-locked");
        let map = make_map(&base, EDITED);

        // Take a backup-like file holding the original bytes.
        let backup = base.join("demo.scx.20260608-000000-000000.bak");
        fs::write(&backup, ORIGINAL).unwrap();
        let entry = JournalEntry {
            map_path: map.clone(),
            backup_path: backup,
        };

        // Lock probe reports the map is open elsewhere → restore must refuse and
        // leave the (edited) map untouched.
        let svc = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            FakeLock(true),
            FakeEngine::ok(EDITED),
        );

        let err = svc
            .restore(&entry)
            .expect_err("restore must refuse while locked");
        assert!(matches!(err, MapSafeError::MapLocked(p) if p == map));
        assert_eq!(
            fs::read(&map).unwrap(),
            EDITED,
            "a refused restore must not touch the map"
        );

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn restore_errors_when_backup_missing() {
        let base = unique_temp_dir("restore-nobak");
        let map = make_map(&base, EDITED);

        let entry = JournalEntry {
            map_path: map.clone(),
            backup_path: base.join("does-not-exist.bak"),
        };
        let svc = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            FakeLock(false),
            FakeEngine::ok(EDITED),
        );

        let err = svc.restore(&entry).expect_err("missing backup must error");
        assert!(matches!(err, MapSafeError::BackupNotFound(_)));

        fs::remove_dir_all(&base).ok();
    }
    #[test]
    fn candidate_apply_and_undo_restore_exact_source_bytes() {
        let base = unique_temp_dir("candidate-apply-undo");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        let source = base.join("source.scx");
        let candidate = base.join("candidate.scx");
        fs::copy(&fixture, &source).unwrap();
        fs::copy(&fixture, &candidate).unwrap();
        let original = fs::read(&source).unwrap();
        isom::switchedit(&candidate, b"rename|1|Candidate Apply Test").unwrap();
        let candidate_bytes = fs::read(&candidate).unwrap();
        assert_ne!(candidate_bytes, original);
        let safe = CandidateMapSafe::new(base.join("backups"), FakeStatus(false), FakeLock(false));
        let record = safe
            .apply(
                &source,
                &candidate,
                &sha256_bytes(&original),
                &sha256_bytes(&candidate_bytes),
            )
            .unwrap();
        assert_eq!(fs::read(&source).unwrap(), candidate_bytes);
        assert_eq!(fs::read(&record.backup_path).unwrap(), original);
        safe.undo(&record).unwrap();
        assert_eq!(fs::read(&source).unwrap(), original);
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn interrupted_candidate_apply_restores_backup_from_pending_journal() {
        let base = unique_temp_dir("candidate-pending-recovery");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        let source = base.join("source.scx");
        let candidate = base.join("candidate.scx");
        fs::copy(&fixture, &source).unwrap();
        fs::copy(&fixture, &candidate).unwrap();
        let original = fs::read(&source).unwrap();
        isom::switchedit(&candidate, b"rename|1|Interrupted Apply").unwrap();
        let candidate_bytes = fs::read(&candidate).unwrap();
        let backup_dir = base.join("backups");
        let candidate_root = base.join("candidates");
        fs::create_dir_all(&candidate_root).unwrap();
        let safe = CandidateMapSafe::new(backup_dir.clone(), FakeStatus(false), FakeLock(false));
        let record = safe
            .apply(
                &source,
                &candidate,
                &sha256_bytes(&original),
                &sha256_bytes(&candidate_bytes),
            )
            .unwrap();
        assert!(pending_apply_path(&record.backup_path).is_file());
        assert_eq!(fs::read(&source).unwrap(), candidate_bytes);
        assert_eq!(
            recover_pending_candidate_applies(&backup_dir, &candidate_root).unwrap(),
            1
        );
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(!pending_apply_path(&record.backup_path).exists());
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn post_apply_verification_failure_restores_backup_immediately() {
        let base = unique_temp_dir("candidate-post-verify-rollback");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        let source = base.join("source.scx");
        let candidate = base.join("candidate.scx");
        fs::copy(&fixture, &source).unwrap();
        fs::copy(&fixture, &candidate).unwrap();
        isom::switchedit(&candidate, b"rename|1|Forced Verify Failure").unwrap();
        let original = fs::read(&source).unwrap();
        let candidate_bytes = fs::read(&candidate).unwrap();
        let safe = CandidateMapSafe::new(base.join("backups"), FakeStatus(false), FakeLock(false));
        let error = safe
            .apply_with_post_verify(
                &source,
                &candidate,
                &sha256_bytes(&original),
                &sha256_bytes(&candidate_bytes),
                |_| Err(CandidateApplyError::Verify("forced post-check".to_string())),
            )
            .unwrap_err();
        assert!(matches!(error, CandidateApplyError::Verify(_)));
        assert_eq!(fs::read(&source).unwrap(), original);
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn candidate_apply_refuses_compiling_lock_stale_and_candidate_mismatch() {
        let base = unique_temp_dir("candidate-apply-guards");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        let source = base.join("source.scx");
        let candidate = base.join("candidate.scx");
        fs::copy(&fixture, &source).unwrap();
        fs::copy(&fixture, &candidate).unwrap();
        let source_hash = sha256_file(&source).unwrap();
        let candidate_hash = sha256_file(&candidate).unwrap();
        let compiling = CandidateMapSafe::new(
            base.join("compiling-backups"),
            FakeStatus(true),
            FakeLock(false),
        );
        assert!(matches!(
            compiling.apply(&source, &candidate, &source_hash, &candidate_hash),
            Err(CandidateApplyError::Compiling)
        ));
        let locked = CandidateMapSafe::new(
            base.join("locked-backups"),
            FakeStatus(false),
            FakeLock(true),
        );
        assert!(matches!(
            locked.apply(&source, &candidate, &source_hash, &candidate_hash),
            Err(CandidateApplyError::MapLocked(_))
        ));
        let normal = CandidateMapSafe::new(
            base.join("normal-backups"),
            FakeStatus(false),
            FakeLock(false),
        );
        assert!(matches!(
            normal.apply(&source, &candidate, "stale", &candidate_hash),
            Err(CandidateApplyError::StaleSource)
        ));
        assert!(matches!(
            normal.apply(&source, &candidate, &source_hash, "changed"),
            Err(CandidateApplyError::CandidateChanged)
        ));
        assert_eq!(sha256_file(&source).unwrap(), source_hash);
        assert!(!base.join("normal-backups").exists());
        fs::remove_dir_all(&base).ok();
    }
    #[cfg(windows)]
    #[test]
    fn real_windows_no_share_handle_blocks_candidate_apply() {
        use std::os::windows::ffi::OsStrExt;
        let base = unique_temp_dir("candidate-real-lock");
        let source = make_map(&base, ORIGINAL);
        let wide = source
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        // SAFETY: valid NUL-terminated path and documented read-only exclusive-open constants.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0x8000_0000,
                0,
                std::ptr::null_mut(),
                3,
                0,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(handle, usize::MAX as *mut core::ffi::c_void);
        assert!(WindowsLockProbe.is_locked(&source));
        // SAFETY: handle was returned by CreateFileW and is closed exactly once.
        unsafe { CloseHandle(handle) };
        assert!(!WindowsLockProbe.is_locked(&source));
        fs::remove_dir_all(&base).ok();
    }
    #[test]
    fn sound_write_runs_guards_backup_native_atomic_replace_and_post_verify_in_order() {
        let base = unique_temp_dir("sound-success");
        let map = make_map(&base, ORIGINAL);
        let expected = sha256_file(&map).unwrap();
        let engine = FakeSoundEngine::ok(EDITED);
        let service = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            SequenceLock::never(),
            engine,
        );
        let result = service
            .write_sound(
                &map,
                &expected,
                "staredit\\wav\\ea_0123456789abcdef.ogg",
                b"OggStest",
            )
            .unwrap();
        assert!(service.engine.add_called.get());
        assert!(service.engine.backup_seen.get());
        assert_eq!(fs::read(&map).unwrap(), EDITED);
        assert_eq!(fs::read(&result.backup_path).unwrap(), ORIGINAL);
        assert_eq!(result.report.output_sha256, sha256_bytes(EDITED));
        assert_eq!(result.map_bytes_before, ORIGINAL.len() as u64);
        assert_eq!(result.map_bytes_after, EDITED.len() as u64);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn sound_replace_runs_backup_atomic_replace_and_old_asset_post_verify() {
        let base = unique_temp_dir("sound-replace-success");
        let map = make_map(&base, ORIGINAL);
        let expected = sha256_file(&map).unwrap();
        let engine = FakeSoundEngine::ok(EDITED);
        let service = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            SequenceLock::never(),
            engine,
        );
        let result = service
            .replace_sound(
                &map,
                &expected,
                "staredit\\wav\\ea_0123456789abcdef.ogg",
                "staredit\\wav\\ea_fedcba9876543210.ogg",
                b"OggSedited",
            )
            .unwrap();
        assert!(service.engine.add_called.get());
        assert!(service.engine.backup_seen.get());
        assert_eq!(fs::read(&map).unwrap(), EDITED);
        assert_eq!(fs::read(&result.backup_path).unwrap(), ORIGINAL);
        assert_eq!(
            result.report.old_mpq_path,
            "staredit\\wav\\ea_0123456789abcdef.ogg"
        );
        assert_eq!(
            result.report.mpq_path,
            "staredit\\wav\\ea_fedcba9876543210.ogg"
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn sound_write_refuses_compiling_lock_and_stale_before_backup_or_native() {
        for (tag, status, lock, expected_error) in [
            ("compiling", true, false, "Compiling"),
            ("locked", false, true, "MapLocked"),
        ] {
            let base = unique_temp_dir(&format!("sound-{tag}"));
            let map = make_map(&base, ORIGINAL);
            let engine = FakeSoundEngine::ok(EDITED);
            let service = MapSafe::new(base.clone(), FakeStatus(status), FakeLock(lock), engine);
            let error = service
                .write_sound(
                    &map,
                    &sha256_file(&map).unwrap(),
                    "staredit\\wav\\ea_0123456789abcdef.ogg",
                    b"OggStest",
                )
                .unwrap_err();
            assert_eq!(
                match error {
                    MapSafeError::Compiling => "Compiling",
                    MapSafeError::MapLocked(_) => "MapLocked",
                    other => panic!("unexpected guard error: {other}"),
                },
                expected_error
            );
            assert!(!service.engine.add_called.get());
            assert!(!base.join("map_backups").exists());
            assert_eq!(fs::read(&map).unwrap(), ORIGINAL);
            fs::remove_dir_all(base).ok();
        }

        let base = unique_temp_dir("sound-stale");
        let map = make_map(&base, ORIGINAL);
        let service = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            FakeLock(false),
            FakeSoundEngine::ok(EDITED),
        );
        assert!(matches!(
            service.write_sound(
                &map,
                &"0".repeat(64),
                "staredit\\wav\\ea_0123456789abcdef.ogg",
                b"OggStest",
            ),
            Err(MapSafeError::StaleSource { .. })
        ));
        assert!(!service.engine.add_called.get());
        assert!(!base.join("map_backups").exists());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn sound_native_and_post_verify_failures_preserve_or_restore_exact_input() {
        let native_base = unique_temp_dir("sound-native-failure");
        let native_map = make_map(&native_base, ORIGINAL);
        let native = MapSafe::new(
            native_base.clone(),
            FakeStatus(false),
            SequenceLock::never(),
            FakeSoundEngine::add_fails(),
        );
        assert!(matches!(
            native.write_sound(
                &native_map,
                &sha256_file(&native_map).unwrap(),
                "staredit\\wav\\ea_0123456789abcdef.ogg",
                b"OggStest",
            ),
            Err(MapSafeError::Apply(_))
        ));
        assert_eq!(fs::read(&native_map).unwrap(), ORIGINAL);

        let verify_base = unique_temp_dir("sound-verify-failure");
        let verify_map = make_map(&verify_base, ORIGINAL);
        let verify = MapSafe::new(
            verify_base.clone(),
            FakeStatus(false),
            SequenceLock::never(),
            FakeSoundEngine::verify_fails(EDITED),
        );
        assert!(matches!(
            verify.write_sound(
                &verify_map,
                &sha256_file(&verify_map).unwrap(),
                "staredit\\wav\\ea_0123456789abcdef.ogg",
                b"OggStest",
            ),
            Err(MapSafeError::PostVerifyRestored { .. })
        ));
        assert_eq!(fs::read(&verify_map).unwrap(), ORIGINAL);

        fs::remove_dir_all(native_base).ok();
        fs::remove_dir_all(verify_base).ok();
    }

    #[test]
    fn sound_post_verify_rollback_lock_surfaces_hazard_and_keeps_backup() {
        let base = unique_temp_dir("sound-rollback-lock");
        let map = make_map(&base, ORIGINAL);
        let service = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            SequenceLock::on(2),
            FakeSoundEngine::verify_fails(EDITED),
        );
        let error = service
            .write_sound(
                &map,
                &sha256_file(&map).unwrap(),
                "staredit\\wav\\ea_0123456789abcdef.ogg",
                b"OggStest",
            )
            .unwrap_err();
        let backup = match error {
            MapSafeError::Rollback { backup, .. } => backup,
            other => panic!("unexpected rollback error: {other}"),
        };
        assert_eq!(fs::read(&map).unwrap(), EDITED);
        assert_eq!(fs::read(backup).unwrap(), ORIGINAL);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn exact_sound_reuse_does_not_replace_input_or_consume_another_slot() {
        let base = unique_temp_dir("sound-reuse");
        let map = make_map(&base, ORIGINAL);
        let service = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            SequenceLock::never(),
            FakeSoundEngine::reused(),
        );
        let result = service
            .write_sound(
                &map,
                &sha256_file(&map).unwrap(),
                "staredit\\wav\\ea_0123456789abcdef.ogg",
                b"OggStest",
            )
            .unwrap();
        assert!(result.report.reused);
        assert_eq!(result.map_bytes_before, result.map_bytes_after);
        assert_eq!(fs::read(&map).unwrap(), ORIGINAL);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn atomic_replace_failure_keeps_destination_bytes() {
        let base = unique_temp_dir("sound-atomic-failure");
        let map = make_map(&base, ORIGINAL);
        let missing = base.join("missing.scx");
        assert!(atomic_replace_file(&map, &missing).is_err());
        assert_eq!(fs::read(&map).unwrap(), ORIGINAL);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn real_scx_sound_mapsafe_backup_native_replace_verify_and_restore_roundtrip() {
        let base = unique_temp_dir("sound-real-roundtrip");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        let ogg = fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("crates")
                .join("isom")
                .join("tests")
                .join("fixtures")
                .join("tone.ogg"),
        )
        .unwrap();
        let source = base.join("source.scx");
        fs::copy(fixture, &source).unwrap();
        let before = sha256_file(&source).unwrap();
        let normalized = sha256_bytes(&ogg);
        let destination = format!("staredit\\wav\\ea_{}.ogg", &normalized[..16]);
        let service = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            WindowsLockProbe,
            IsomEngine,
        );
        let result = service
            .write_sound(&source, &before, &destination, &ogg)
            .unwrap();
        assert!(!result.report.reused);
        assert_eq!(sha256_file(&result.backup_path).unwrap(), before);
        assert_eq!(sha256_file(&source).unwrap(), result.report.output_sha256);
        service
            .restore(&JournalEntry {
                map_path: source.clone(),
                backup_path: result.backup_path,
            })
            .unwrap();
        assert_eq!(sha256_file(&source).unwrap(), before);
        fs::remove_dir_all(base).ok();
    }
    #[cfg(windows)]
    #[test]
    fn sound_real_windows_share_lock_refuses_before_backup_and_native_write() {
        use std::os::windows::ffi::OsStrExt;
        let base = unique_temp_dir("sound-real-lock");
        let map = make_map(&base, ORIGINAL);
        let wide = map
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        // SAFETY: valid NUL-terminated path and documented exclusive read-open constants.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0x8000_0000,
                0,
                std::ptr::null_mut(),
                3,
                0,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(handle, usize::MAX as *mut core::ffi::c_void);
        let service = MapSafe::new(
            base.clone(),
            FakeStatus(false),
            WindowsLockProbe,
            FakeSoundEngine::ok(EDITED),
        );
        assert!(matches!(
            service.write_sound(
                &map,
                &sha256_bytes(ORIGINAL),
                "staredit\\wav\\ea_0123456789abcdef.ogg",
                b"OggStest",
            ),
            Err(MapSafeError::MapLocked(_))
        ));
        assert!(!service.engine.add_called.get());
        assert!(!base.join("map_backups").exists());
        // SAFETY: handle was returned by CreateFileW and is closed exactly once.
        unsafe { CloseHandle(handle) };
        fs::remove_dir_all(base).ok();
    }
}
