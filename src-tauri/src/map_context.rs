use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::bridge_io::{SendOpts, HEARTBEAT_STALE_AFTER};
use crate::config::DataDirs;
use crate::map_model::{hex_sha256, MapRevision, Tileset};

#[derive(Clone)]
pub struct MapContextService {
    dirs: DataDirs,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapContextSnapshot {
    pub revision: MapRevision,
    pub saved_source_notice: String,
    pub source_file_size: u64,
    pub starcraft_path: PathBuf,
    pub digest: crate::chk::Digest,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapSourceProbe {
    pub project_id: String,
    pub source_path: PathBuf,
    #[serde(with = "crate::map_model::u128_string")]
    pub mtime_ns: u128,
    pub file_size: u64,
}

impl MapContextService {
    pub fn new(dirs: DataDirs) -> Self {
        Self { dirs }
    }

    pub fn current(&self) -> Result<MapContextSnapshot, String> {
        let (project_id, source_path) = self.current_source()?;
        let probe = Self::probe_path(project_id, source_path)?;
        self.snapshot_for_probe(probe)
    }

    pub(crate) fn snapshot_for_probe(
        &self,
        probe: MapSourceProbe,
    ) -> Result<MapContextSnapshot, String> {
        let file = std::fs::read(&probe.source_path)
            .map_err(|error| format!("source map bytes could not be read: {error}"))?;
        let chk = isom::chk_extract(&probe.source_path)
            .map_err(|error| format!("saved source map CHK could not be extracted: {error}"))?;
        let revision = revision_from_parts(
            probe.project_id,
            &probe.source_path,
            probe.mtime_ns,
            &file,
            &chk,
        )?;
        Ok(MapContextSnapshot {
            revision,
            saved_source_notice: "저장된 SCX 기준 · SCMDraft 미저장 상태는 포함되지 않음"
                .to_string(),
            source_file_size: probe.file_size,
            starcraft_path: resolve_starcraft_path(&self.dirs)?,
            digest: crate::chk::digest_chk(&chk),
        })
    }

    pub fn probe_current(&self) -> Result<MapSourceProbe, String> {
        let (project_id, source_path) = self.status_source()?;
        Self::probe_path(project_id, source_path)
    }

    fn probe_path(project_id: String, source_path: PathBuf) -> Result<MapSourceProbe, String> {
        let metadata = std::fs::metadata(&source_path)
            .map_err(|error| format!("saved source map metadata could not be read: {error}"))?;
        if !metadata.is_file() {
            return Err("saved OpenMapName is not a file".to_string());
        }
        let mtime_ns = metadata
            .modified()
            .map_err(|error| format!("saved source map mtime could not be read: {error}"))?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Ok(MapSourceProbe {
            project_id,
            source_path,
            mtime_ns,
            file_size: metadata.len(),
        })
    }

    #[cfg(not(test))]
    pub(crate) fn snapshot_binding_is_current(
        &self,
        snapshot: &MapContextSnapshot,
    ) -> Result<bool, String> {
        if self.current_project_id()? != snapshot.revision.project_id {
            return Ok(false);
        }
        let metadata = std::fs::metadata(&snapshot.revision.source_path)
            .map_err(|error| format!("saved source map metadata could not be read: {error}"))?;
        if !metadata.is_file() {
            return Ok(false);
        }
        let mtime_ns = metadata
            .modified()
            .map_err(|error| format!("saved source map mtime could not be read: {error}"))?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Ok(mtime_ns == snapshot.revision.mtime_ns && metadata.len() == snapshot.source_file_size)
    }

    fn current_source(&self) -> Result<(String, PathBuf), String> {
        let status = self.current_status()?;
        if status.compiling {
            return Err(
                "OpenMapName cannot be confirmed while the editor is compiling".to_string(),
            );
        }
        let project_path = Self::project_path_from_status(&status)?;
        let bridge = crate::ipc::bridge_from_config(&self.dirs)?;
        let reply = bridge
            .send(
                "GETSET project|OpenMapName",
                &SendOpts::for_map_source_confirmation(),
                None,
            )
            .map_err(|error| format!("OpenMapName could not be read: {error}"))?;
        Self::source_from_project(project_path, parse_open_map_reply(&reply))
    }

    fn status_source(&self) -> Result<(String, PathBuf), String> {
        let status = self.current_status()?;
        let project_path = Self::project_path_from_status(&status)?;
        let open_map_name = status.open_map_name.as_deref().ok_or_else(|| {
            "editor bridge status does not expose OpenMapName; restart EUD Editor after updating the bridge"
                .to_string()
        })?;
        Self::source_from_project(project_path, unquote_status_value(open_map_name))
    }

    fn source_from_project(
        project_path: PathBuf,
        open_map_name: &str,
    ) -> Result<(String, PathBuf), String> {
        if open_map_name.trim().is_empty() {
            return Err("the current project has no saved OpenMapName".to_string());
        }
        let project_root = project_path
            .parent()
            .ok_or_else(|| "the current project path has no parent directory".to_string())?;
        let requested = PathBuf::from(open_map_name.trim());
        let requested = if requested.is_absolute() {
            requested
        } else {
            project_root.join(requested)
        };
        let source = requested
            .canonicalize()
            .map_err(|error| format!("saved source map is missing or unreadable: {error}"))?;
        let canonical_root = project_root
            .canonicalize()
            .map_err(|error| format!("project directory is unreadable: {error}"))?;
        if !source.starts_with(&canonical_root) {
            return Err(
                "OpenMapName resolves outside the current EUD project directory".to_string(),
            );
        }
        Ok((project_id_for_path(project_path), source))
    }

    fn current_status(&self) -> Result<crate::bridge_io::StatusSnapshot, String> {
        let bridge = crate::ipc::bridge_from_config(&self.dirs)?;
        bridge
            .read_status_snapshot(HEARTBEAT_STALE_AFTER)
            .map_err(|error| format!("editor bridge is unavailable: {error}"))
    }

    fn project_path_from_status(
        status: &crate::bridge_io::StatusSnapshot,
    ) -> Result<PathBuf, String> {
        let project = unquote_status_value(&status.project);
        if project.is_empty() {
            return Err("no EUD project is currently open".to_string());
        }
        Ok(PathBuf::from(project))
    }

    #[cfg(not(test))]
    fn current_project_id(&self) -> Result<String, String> {
        self.current_project_path().map(project_id_for_path)
    }

    #[cfg(not(test))]
    fn current_project_path(&self) -> Result<PathBuf, String> {
        let status = self.current_status()?;
        Self::project_path_from_status(&status)
    }

    pub fn revision_for_path(
        &self,
        project_id: String,
        source_path: &Path,
    ) -> Result<MapRevision, String> {
        let metadata = std::fs::metadata(source_path)
            .map_err(|error| format!("source map metadata could not be read: {error}"))?;
        if !metadata.is_file() {
            return Err("source map path is not a file".to_string());
        }
        let file = std::fs::read(source_path)
            .map_err(|error| format!("source map bytes could not be read: {error}"))?;
        let chk = isom::chk_extract(source_path)
            .map_err(|error| format!("source map CHK could not be extracted: {error}"))?;
        let modified = metadata
            .modified()
            .map_err(|error| format!("source map mtime could not be read: {error}"))?;
        let mtime_ns = modified
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        revision_from_parts(project_id, source_path, mtime_ns, &file, &chk)
    }

    pub fn starcraft_path(&self) -> Result<PathBuf, String> {
        resolve_starcraft_path(&self.dirs)
    }
}

fn project_id_for_path(project_path: PathBuf) -> String {
    let project_identity = project_path
        .canonicalize()
        .unwrap_or(project_path)
        .to_string_lossy()
        .to_lowercase();
    hex_sha256(project_identity.as_bytes())
}

fn revision_from_parts(
    project_id: String,
    source_path: &Path,
    mtime_ns: u128,
    file: &[u8],
    chk: &[u8],
) -> Result<MapRevision, String> {
    let sections = crate::chk::assemble_sections(&crate::chk::walk_sections(chk));
    let dim = sections
        .get("DIM ")
        .ok_or_else(|| "source map has no DIM section".to_string())?;
    let era = sections
        .get("ERA ")
        .ok_or_else(|| "source map has no ERA section".to_string())?;
    if dim.len() < 4 || era.len() < 2 {
        return Err("source map DIM/ERA section is truncated".to_string());
    }
    let width = u16::from_le_bytes([dim[0], dim[1]]);
    let height = u16::from_le_bytes([dim[2], dim[3]]);
    if width == 0 || height == 0 {
        return Err("source map dimensions are empty".to_string());
    }
    let era = u16::from_le_bytes([era[0], era[1]]);
    Ok(MapRevision {
        project_id,
        source_path: source_path.to_path_buf(),
        file_sha256: hex_sha256(file),
        chk_sha256: hex_sha256(chk),
        mtime_ns,
        tileset: Tileset::from_era(era)?,
        width,
        height,
    })
}

fn parse_open_map_reply(reply: &str) -> &str {
    let trimmed = reply.trim();
    let Some((prefix, value)) = trimmed.split_once(" = ") else {
        return trimmed;
    };
    if prefix.trim() == "OK: project|OpenMapName" {
        value.trim()
    } else {
        trimmed
    }
}

fn unquote_status_value(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == 0x27 && bytes[value.len() - 1] == 0x27)
            || (bytes[0] == 0x22 && bytes[value.len() - 1] == 0x22)
        {
            return &value[1..value.len() - 1];
        }
    }
    value
}

fn resolve_starcraft_path(dirs: &DataDirs) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("STARCRAFT_PATH").map(PathBuf::from) {
        if path.is_dir() {
            return Ok(path);
        }
        return Err("STARCRAFT_PATH does not name an installed StarCraft directory".to_string());
    }
    let standard = PathBuf::from(r"C:\Program Files (x86)\StarCraft");
    if standard.is_dir() {
        return Ok(standard);
    }
    let editor = dirs
        .load_config()
        .map_err(|error| format!("app config could not be read: {error}"))?
        .editor_path;
    let editor = PathBuf::from(editor);
    if editor.is_dir() {
        return Ok(editor);
    }
    Err("StarCraft data directory could not be resolved".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dirs(root: &Path) -> DataDirs {
        DataDirs::from_bases(&root.join("roaming"), &root.join("local"))
    }

    fn status_probe_service(
        root: &Path,
        compiling: bool,
        include_open_map: bool,
    ) -> (MapContextService, PathBuf, PathBuf) {
        let dirs = test_dirs(root);
        let editor = root.join("editor");
        let agent_dir = editor.join("Data").join("agent");
        let inbox = agent_dir.join("inbox");
        let project_dir = root.join("project");
        let project = project_dir.join("demo.e3s");
        let map = project_dir.join("demo.scx");
        std::fs::create_dir_all(&inbox).unwrap();
        std::fs::create_dir_all(agent_dir.join("outbox")).unwrap();
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(&project, b"project").unwrap();
        std::fs::write(&map, b"map").unwrap();
        dirs.save_config(&crate::config::Config {
            editor_path: editor.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();
        let open_map_line = include_open_map
            .then(|| format!("\nopenMapName='{}'", map.display()))
            .unwrap_or_default();
        std::fs::write(
            agent_dir.join("status.txt"),
            format!(
                "compiling={compiling}\nproject='{}'{open_map_line}\n",
                project.display()
            ),
        )
        .unwrap();
        std::fs::write(agent_dir.join("heartbeat.txt"), b"alive\n").unwrap();
        (MapContextService::new(dirs), map, inbox)
    }

    #[test]
    fn rich_fixture_revision_is_stable_and_complete() {
        let root = std::env::temp_dir().join(format!("map-context-{}", uuid::Uuid::new_v4()));
        let service = MapContextService::new(test_dirs(&root));
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        let first = service
            .revision_for_path("project".to_string(), &fixture)
            .unwrap();
        let second = service
            .revision_for_path("project".to_string(), &fixture)
            .unwrap();
        assert_eq!(first, second);
        assert!(!first.file_sha256.is_empty());
        assert!(!first.chk_sha256.is_empty());
        assert!(first.width > 0 && first.height > 0);
    }

    #[test]
    fn source_probe_serializes_mtime_losslessly() {
        let mtime_ns = 1_700_000_000_000_000_123_u128;
        let value = serde_json::to_value(MapSourceProbe {
            project_id: "project".to_string(),
            source_path: PathBuf::from(r"C:\maps\demo.scx"),
            mtime_ns,
            file_size: 1024,
        })
        .unwrap();
        assert_eq!(value["mtimeNs"], mtime_ns.to_string());
    }

    #[test]
    fn passive_source_probe_uses_status_without_bridge_command() {
        let root = std::env::temp_dir().join(format!("map-context-probe-{}", uuid::Uuid::new_v4()));
        let (service, map, inbox) = status_probe_service(&root, false, true);

        let probe = service.probe_current().unwrap();

        assert_eq!(probe.source_path, map.canonicalize().unwrap());
        assert_eq!(probe.file_size, 3);
        assert!(std::fs::read_dir(inbox).unwrap().next().is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn passive_source_probe_requires_updated_bridge_status() {
        let root =
            std::env::temp_dir().join(format!("map-context-old-bridge-{}", uuid::Uuid::new_v4()));
        let (service, _, inbox) = status_probe_service(&root, false, false);

        let error = service.probe_current().unwrap_err();

        assert!(error.contains("restart EUD Editor after updating the bridge"));
        assert!(std::fs::read_dir(inbox).unwrap().next().is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn authoritative_source_confirmation_skips_compiling_editor() {
        let root =
            std::env::temp_dir().join(format!("map-context-compiling-{}", uuid::Uuid::new_v4()));
        let (service, _, inbox) = status_probe_service(&root, true, true);

        let error = service.current_source().unwrap_err();

        assert_eq!(
            error,
            "OpenMapName cannot be confirmed while the editor is compiling"
        );
        assert!(std::fs::read_dir(inbox).unwrap().next().is_none());
        std::fs::remove_dir_all(root).ok();
    }
    #[test]
    fn open_map_reply_accepts_only_the_protocol_prefix() {
        assert_eq!(
            parse_open_map_reply("OK: project|OpenMapName = C:\\map.scx"),
            "C:\\map.scx"
        );
        assert_eq!(parse_open_map_reply("C:\\map.scx"), "C:\\map.scx");
    }
    #[test]
    #[ignore = "requires the live EUD Editor bridge and current OpenMapName"]
    fn live_saved_open_map_loads_and_renders() {
        let roaming = PathBuf::from(std::env::var_os("APPDATA").unwrap());
        let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
        let service = MapContextService::new(DataDirs::from_bases(&roaming, &local));
        let context = service.current().unwrap();
        eprintln!(
            "live map: {} {}x{} {}",
            context.revision.source_path.display(),
            context.revision.width,
            context.revision.height,
            context.revision.tileset.era()
        );
        assert!(context.revision.source_path.is_file());
        assert!(!context.revision.file_sha256.is_empty());
        assert!(context.revision.width > 0 && context.revision.height > 0);
        let request = serde_json::json!({
            "schema": "eud-map-render/1",
            "mode": "region",
            "x": 0,
            "y": 0,
            "width": context.revision.width.min(16),
            "height": context.revision.height.min(16),
            "scale": 4,
            "layers": ["terrain", "doodads", "sprites", "units", "buildings"]
        });
        let image = isom::render_region(
            &context.revision.source_path,
            &context.starcraft_path,
            request.to_string().as_bytes(),
        )
        .unwrap();
        assert!(image.width > 0 && image.height > 0);
        assert!(image.rgba.chunks_exact(4).any(|pixel| pixel[3] == 255));
    }
}
