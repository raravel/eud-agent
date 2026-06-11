//! Keep the editor's installed Lua bridge in sync with the one bundled in the app.
//!
//! The slim file-IPC bridge (`bridge/ZZZ_10_agent_bridge.lua`) is a copy that lives inside
//! the EUD Editor 3 install at `Data\Lua\TriggerEditor\` (rules.md: integration is file
//! copies only — the editor is third-party and never modified). The app bundles the bridge
//! as a Tauri resource and, on every start, copies it over the editor's copy when they
//! differ — so a self-update that ships a newer bridge re-installs it on the next launch
//! without the user re-running `scripts/install_bridge.ps1`.
//!
//! This is the Rust port of that script's copy step. Bytes are compared and copied verbatim
//! (NEVER re-encoded): KopiLua reads the `.lua` as Latin1, so any byte rewrite would corrupt
//! non-ASCII content. The sync is best-effort at the call site (a downed/edited editor must
//! not block app startup).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

/// Basename of the bridge Lua under the editor's TriggerEditor folder. Matches
/// `scripts/install_bridge.ps1` (`$LuaName`).
pub const BRIDGE_LUA_NAME: &str = "ZZZ_10_agent_bridge.lua";

/// The editor-relative install path of the bridge: `<editor>\Data\Lua\TriggerEditor`.
/// Mirrors `install_bridge.ps1` (`$TriggerEditorRel`) and `config::validate_editor_path`,
/// which uses the same folder as the marker that a path is a real editor install.
pub fn trigger_editor_dir(editor_path: &Path) -> PathBuf {
    editor_path
        .join("Data")
        .join("Lua")
        .join("TriggerEditor")
}

/// Copy `bundled_lua` over the editor's bridge copy when their bytes differ.
///
/// Returns `Ok(true)` when a copy was performed (target missing or stale), `Ok(false)` when
/// the target was already byte-identical (no write). Idempotent: re-runs are a no-op once
/// in sync. Errors if the bundled source is unreadable or the TriggerEditor folder is
/// absent (i.e. `editor_path` is not a real editor install) — callers treat that as
/// non-fatal.
pub fn sync_bridge(bundled_lua: &Path, editor_path: &Path) -> anyhow::Result<bool> {
    let src_bytes = fs::read(bundled_lua)
        .with_context(|| format!("cannot read bundled bridge {}", bundled_lua.display()))?;

    let dst_dir = trigger_editor_dir(editor_path);
    if !dst_dir.is_dir() {
        anyhow::bail!(
            "editor TriggerEditor folder not found: {} (is editor_path a valid EUD Editor 3 install?)",
            dst_dir.display()
        );
    }
    let dst = dst_dir.join(BRIDGE_LUA_NAME);

    // Skip the write when the editor already has the identical bytes — the bridge file is
    // small, so a full read-compare is cheaper than a needless overwrite each launch.
    if let Ok(existing) = fs::read(&dst) {
        if existing == src_bytes {
            return Ok(false);
        }
    }

    fs::write(&dst, &src_bytes)
        .with_context(|| format!("cannot write bridge to {}", dst.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-bridge-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A fake editor root with the `Data\Lua\TriggerEditor` marker folder present.
    fn make_editor_root(base: &Path) -> PathBuf {
        let editor = base.join("EUDEditor3");
        fs::create_dir_all(trigger_editor_dir(&editor)).unwrap();
        editor
    }

    fn write_bundled(base: &Path, contents: &[u8]) -> PathBuf {
        let src = base.join(BRIDGE_LUA_NAME);
        fs::write(&src, contents).unwrap();
        src
    }

    #[test]
    fn copies_when_target_missing() {
        let base = unique_temp_dir("missing");
        let editor = make_editor_root(&base);
        let src = write_bundled(&base, b"-- bridge v2\n");

        let copied = sync_bridge(&src, &editor).unwrap();

        assert!(copied, "a missing target must be installed");
        let dst = trigger_editor_dir(&editor).join(BRIDGE_LUA_NAME);
        assert_eq!(fs::read(&dst).unwrap(), b"-- bridge v2\n");
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn noop_when_identical() {
        let base = unique_temp_dir("identical");
        let editor = make_editor_root(&base);
        let src = write_bundled(&base, b"-- bridge v2\n");
        // Pre-place the identical bytes.
        let dst = trigger_editor_dir(&editor).join(BRIDGE_LUA_NAME);
        fs::write(&dst, b"-- bridge v2\n").unwrap();

        let copied = sync_bridge(&src, &editor).unwrap();

        assert!(!copied, "identical bytes must not be rewritten");
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn overwrites_when_stale() {
        let base = unique_temp_dir("stale");
        let editor = make_editor_root(&base);
        let src = write_bundled(&base, b"-- bridge v3 (new)\n");
        let dst = trigger_editor_dir(&editor).join(BRIDGE_LUA_NAME);
        fs::write(&dst, b"-- bridge v2 (old)\n").unwrap();

        let copied = sync_bridge(&src, &editor).unwrap();

        assert!(copied, "stale target must be overwritten");
        assert_eq!(fs::read(&dst).unwrap(), b"-- bridge v3 (new)\n");
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn preserves_non_ascii_bytes_verbatim() {
        // KopiLua reads the .lua as Latin1; the sync must copy bytes without re-encoding.
        let base = unique_temp_dir("bytes");
        let editor = make_editor_root(&base);
        let raw = b"-- \xEC\x95\x88\xEB\x85\x95\n"; // UTF-8 bytes, must survive untouched
        let src = write_bundled(&base, raw);

        sync_bridge(&src, &editor).unwrap();

        let dst = trigger_editor_dir(&editor).join(BRIDGE_LUA_NAME);
        assert_eq!(fs::read(&dst).unwrap(), raw);
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn errors_when_editor_path_invalid() {
        let base = unique_temp_dir("invalid");
        let src = write_bundled(&base, b"-- bridge\n");
        // No Data\Lua\TriggerEditor under this path.
        let not_editor = base.join("not-an-editor");
        fs::create_dir_all(&not_editor).unwrap();

        assert!(
            sync_bridge(&src, &not_editor).is_err(),
            "a non-editor path must error so the caller can skip it"
        );
        fs::remove_dir_all(&base).ok();
    }
}
