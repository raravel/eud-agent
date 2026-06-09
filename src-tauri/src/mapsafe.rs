//! Map-write safety rails + journal (port of the Python `chk_info` write path +
//! `journal._rollback_location`).
//!
//! EVERY mutating map write (location_write / player_setup) runs these rails IN
//! ORDER (rules.md "Map file writes"; features/09 "Safety rails"):
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

use std::path::{Path, PathBuf};

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

/// Which isom op family a write routes to (rail 4). Locedit -> isom::locedit,
/// PlayerEdit -> isom::playeredit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Locedit,
    PlayerEdit,
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
        }
        .map_err(|e| e.to_string())
    }

    fn digest(&self, map: &Path) -> Result<Vec<u8>, String> {
        isom::chk_extract(map).map_err(|e| e.to_string())
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

        // Write via a temp file beside the map, then atomically rename over it so a
        // crash mid-restore can never leave a half-written map. `fs::rename` on the
        // same volume is atomic and replaces the destination on Windows & *nix.
        let mut ext = entry
            .map_path
            .extension()
            .unwrap_or_default()
            .to_os_string();
        ext.push(".restoretmp");
        let tmp = entry.map_path.with_extension(ext);

        let bytes = std::fs::read(&entry.backup_path)?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &entry.map_path)?;
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

    // ------------------------------------------------------------- helpers

    /// Unique temp base dir for a test, avoiding a `tempfile` dev-dependency
    /// (Cargo.toml is out of scope for this task — same precedent as config.rs).
    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-mapsafe-{tag}-{nanos}"));
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
}
