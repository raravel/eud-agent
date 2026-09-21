//! Blank-map project creation for the launcher wizard.
//!
//! The wizard sends one [`BlankProjectRequest`]; this module validates the
//! project name, derives the canonical manifest (`maps/<name>.scx`,
//! `build/[EUD]<name>.scx`), asks the native engine to create the map, and then
//! independently re-reads the CHK so the manifest is only published for a map
//! that matches the requested spec. Filesystem/session orchestration stays in
//! `setup.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Ordered tilesets as the CHK `ERA` value names them. Korean labels are the
/// panel's display text; `key` is the stable identifier used in specs/tests.
pub const TILESETS: [(&str, &str); 8] = [
    ("badlands", "배드랜드"),
    ("platform", "스페이스 플랫폼"),
    ("installation", "인스톨레이션"),
    ("ashworld", "애쉬 월드"),
    ("jungle", "정글"),
    ("desert", "사막"),
    ("arctic", "얼음"),
    ("twilight", "트와일라잇"),
];

pub const MAP_SIZE_MIN: u16 = 64;
pub const MAP_SIZE_MAX: u16 = 256;
pub const PROJECT_NAME_MAX: usize = 64;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlankProjectRequest {
    /// Absolute destination folder chosen through the native picker.
    pub destination: String,
    /// Project name; also the map file stem and the default scenario title.
    pub name: String,
    pub spec: isom::MapNewSpec,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TilesetOption {
    pub id: u8,
    pub key: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StarcraftAvailability {
    pub available: bool,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapNewOptionsResponse {
    pub starcraft: StarcraftAvailability,
    pub tilesets: Vec<TilesetOption>,
    pub size_presets: Vec<u16>,
    pub size_min: u16,
    pub size_max: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrushOption {
    pub id: u16,
    pub name: String,
    pub graphics_valid: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlankProjectPreview {
    pub map_path: String,
    pub output_map: String,
    pub width: u16,
    pub height: u16,
    pub tileset: String,
    pub players: usize,
    pub start_locations: usize,
    /// PNG bytes, base64.
    pub preview_png: String,
}

pub fn tileset_options() -> Vec<TilesetOption> {
    TILESETS
        .iter()
        .enumerate()
        .map(|(id, (key, label))| TilesetOption {
            id: id as u8,
            key,
            label,
        })
        .collect()
}

pub fn size_presets() -> Vec<u16> {
    vec![64, 96, 128, 192, 256]
}

/// Validate a project/map name as a single Windows-safe file stem.
pub fn validate_project_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("프로젝트 이름을 입력해 주세요.".to_string());
    }
    if trimmed.chars().count() > PROJECT_NAME_MAX {
        return Err(format!(
            "프로젝트 이름은 {PROJECT_NAME_MAX}자 이하여야 합니다."
        ));
    }
    if trimmed.chars().any(|c| {
        matches!(
            c,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '[' | ']'
        ) || c.is_control()
    }) {
        return Err("프로젝트 이름에는 < > : \" / \\ | ? * [ ] 문자를 쓸 수 없습니다.".to_string());
    }
    if trimmed.ends_with('.') {
        return Err("프로젝트 이름은 마침표로 끝날 수 없습니다.".to_string());
    }
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = trimmed
        .split('.')
        .next()
        .unwrap_or(trimmed)
        .to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return Err("Windows 예약 이름은 프로젝트 이름으로 쓸 수 없습니다.".to_string());
    }
    Ok(trimmed.to_string())
}

/// Manifest for a blank-map project. Mirrors `create_project_from_map` naming.
pub fn blank_manifest(name: &str) -> Result<crate::native_project::ProjectManifest, String> {
    let name = validate_project_name(name)?;
    Ok(crate::native_project::ProjectManifest {
        schema_version: crate::native_project::PROJECT_SCHEMA_VERSION,
        source_map: format!("maps/{name}.scx"),
        output_map: crate::native_project::default_output_map(&name, "scx"),
        main_file: "src/main.eps".to_string(),
        name,
        settings: crate::native_project::ProjectSettings::default(),
        plugins: Vec::new(),
        python_entrypoints: Vec::new(),
        python_dependencies: Vec::new(),
        python_lock: None,
        editor_compatibility: None,
    })
}

/// Reject a spec before any native call so the wizard gets an actionable
/// message instead of an engine status code.
pub fn validate_spec(spec: &isom::MapNewSpec) -> Result<(), String> {
    if !(MAP_SIZE_MIN..=MAP_SIZE_MAX).contains(&spec.width)
        || !(MAP_SIZE_MIN..=MAP_SIZE_MAX).contains(&spec.height)
    {
        return Err(format!(
            "맵 크기는 {MAP_SIZE_MIN}~{MAP_SIZE_MAX} 타일 사이여야 합니다."
        ));
    }
    if usize::from(spec.tileset) >= TILESETS.len() {
        return Err("지원하지 않는 타일셋입니다.".to_string());
    }
    if spec.terrain_type == 0 {
        return Err("초기 지형을 선택해 주세요.".to_string());
    }
    if !matches!(spec.version.as_str(), "remastered" | "broodWar") {
        return Err("맵 버전은 remastered 또는 broodWar여야 합니다.".to_string());
    }
    if spec.title.trim().is_empty() || spec.title.len() > 1024 {
        return Err("맵 제목은 1자 이상 1024바이트 이하여야 합니다.".to_string());
    }
    if spec.description.len() > 4096 {
        return Err("맵 설명은 4096바이트 이하여야 합니다.".to_string());
    }
    if spec.forces.is_empty() || spec.forces.len() > 4 {
        return Err("포스는 1~4개여야 합니다.".to_string());
    }
    for force in &spec.forces {
        if force.name.trim().is_empty() || force.name.len() > 256 {
            return Err("포스 이름은 1자 이상 256바이트 이하여야 합니다.".to_string());
        }
    }
    if spec.players.len() > 8 {
        return Err("플레이어는 최대 8명입니다.".to_string());
    }
    let mut slots = std::collections::BTreeSet::new();
    for player in &spec.players {
        if player.slot > 7 {
            return Err("플레이어 슬롯은 1~8 사이여야 합니다.".to_string());
        }
        if !slots.insert(player.slot) {
            return Err("같은 플레이어 슬롯이 두 번 지정되었습니다.".to_string());
        }
        if usize::from(player.force) >= spec.forces.len() {
            return Err("플레이어가 존재하지 않는 포스를 가리킵니다.".to_string());
        }
        if !matches!(
            player.r#type.as_str(),
            "human" | "computer" | "rescuable" | "neutral" | "inactive" | "closed"
        ) {
            return Err("지원하지 않는 플레이어 슬롯 타입입니다.".to_string());
        }
        if !matches!(
            player.race.as_str(),
            "zerg" | "terran" | "protoss" | "userSelectable" | "random"
        ) {
            return Err("지원하지 않는 종족입니다.".to_string());
        }
        if let Some(start) = &player.start {
            if u32::from(start.x) >= u32::from(spec.width) * 32
                || u32::from(start.y) >= u32::from(spec.height) * 32
            {
                return Err("시작 위치가 맵 밖에 있습니다.".to_string());
            }
        }
    }
    Ok(())
}

/// Read the brush list for one tileset from the native catalog.
pub fn tileset_brushes(starcraft: &Path, tileset: u8) -> Result<Vec<BrushOption>, String> {
    if usize::from(tileset) >= TILESETS.len() {
        return Err("지원하지 않는 타일셋입니다.".to_string());
    }
    let request = serde_json::json!({
        "schema": "eud-map-catalog/1",
        "kind": "brushes",
        "tileset": tileset,
        "offset": 0,
        "limit": 128,
    });
    let text = isom::catalog_query(starcraft, request.to_string().as_bytes())
        .map_err(|error| format!("지형 브러시 목록을 읽지 못했습니다: {error}"))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("지형 브러시 목록 응답이 올바르지 않습니다: {error}"))?;
    let entries = value["entries"]
        .as_array()
        .ok_or_else(|| "지형 브러시 목록 응답에 entries가 없습니다.".to_string())?;
    let mut brushes = Vec::with_capacity(entries.len());
    for entry in entries {
        let id = entry["terrainType"]
            .as_u64()
            .and_then(|id| u16::try_from(id).ok())
            .ok_or_else(|| "지형 브러시 항목에 terrainType이 없습니다.".to_string())?;
        let name = entry["name"]
            .as_str()
            .ok_or_else(|| "지형 브러시 항목에 name이 없습니다.".to_string())?
            .to_string();
        brushes.push(BrushOption {
            id,
            name,
            graphics_valid: entry["graphicsValid"].as_bool().unwrap_or(false),
        });
    }
    Ok(brushes)
}

/// Independent CHK re-read of the generated map against the spec.
pub fn verify_created_map(
    map_path: &Path,
    spec: &isom::MapNewSpec,
) -> Result<crate::chk::Digest, String> {
    let chk = isom::chk_extract(map_path)
        .map_err(|error| format!("생성된 맵의 CHK를 읽지 못했습니다: {error}"))?;
    let digest = crate::chk::digest_chk(&chk);
    if digest.map.width != spec.width || digest.map.height != spec.height {
        return Err(format!(
            "생성된 맵 크기가 요청과 다릅니다: {}x{} (요청 {}x{})",
            digest.map.width, digest.map.height, spec.width, spec.height
        ));
    }
    let expected_starts = spec
        .players
        .iter()
        .filter(|player| player.start.is_some())
        .count();
    if digest.start_locations.len() != expected_starts {
        return Err(format!(
            "생성된 맵의 시작 위치 수가 요청과 다릅니다: {} (요청 {expected_starts})",
            digest.start_locations.len()
        ));
    }
    let expected_tiles = usize::from(spec.width) * usize::from(spec.height);
    if digest.tiles.len() != expected_tiles || digest.tiles.contains(&0) {
        return Err("생성된 맵의 지형이 완전히 채워지지 않았습니다.".to_string());
    }
    Ok(digest)
}

/// Render a bounded top-down preview of the whole map as PNG.
pub fn render_preview_png(
    map_path: &Path,
    starcraft: &Path,
    width: u16,
    height: u16,
) -> Result<Vec<u8>, String> {
    // 8x downscale keeps a 256x256 map at 1024px; smaller maps stay legible.
    let scale = if width.max(height) > 128 { 8 } else { 4 };
    let request = serde_json::json!({
        "schema": "eud-map-render/1",
        "mode": "region",
        "x": 0,
        "y": 0,
        "width": width,
        "height": height,
        "scale": scale,
        "layers": ["terrain", "units"],
    });
    let image = isom::render_region(map_path, starcraft, request.to_string().as_bytes())
        .map_err(|error| format!("미리보기를 그리지 못했습니다: {error}"))?;
    crate::map_agent::encode_rgba_png(&image)
}

/// Create the manifest tree and the blank map inside an empty destination.
///
/// The engine writes `maps/<name>.scx` through its own same-directory temporary
/// file and atomic promotion; the map is then re-read through the CHK parser
/// and rendered before the preview is returned. On any failure the caller
/// removes generated contents (never the folder).
pub fn create_blank_project(
    destination: &Path,
    starcraft: &Path,
    request: &BlankProjectRequest,
) -> Result<BlankProjectPreview, String> {
    validate_spec(&request.spec)?;
    let manifest = blank_manifest(&request.name)?;
    if destination.exists()
        && fs::read_dir(destination)
            .map_err(|error| error.to_string())?
            .next()
            .is_some()
    {
        return Err(format!(
            "project destination must be empty: {}",
            destination.display()
        ));
    }
    let output_map = manifest.output_map.clone();
    let map_name = format!("{}.scx", manifest.name);

    let project = crate::native_project::NativeProject::create(destination, manifest)?;
    let map_path: PathBuf = project.root().join("maps").join(&map_name);
    if map_path.exists() {
        return Err("맵 파일이 이미 존재합니다.".to_string());
    }
    isom::map_new(&map_path, starcraft, &request.spec)
        .map_err(|error| format!("맵을 생성하지 못했습니다: {error}"))?;
    let digest = verify_created_map(&map_path, &request.spec)?;
    let preview = render_preview_png(
        &map_path,
        starcraft,
        request.spec.width,
        request.spec.height,
    )?;
    use base64::Engine as _;
    Ok(BlankProjectPreview {
        map_path: map_path.display().to_string(),
        output_map,
        width: digest.map.width,
        height: digest.map.height,
        tileset: digest.map.tileset,
        players: request.spec.players.len(),
        start_locations: digest.start_locations.len(),
        preview_png: base64::engine::general_purpose::STANDARD.encode(preview),
    })
}

/// Remove everything the wizard generated inside `destination`, keeping the
/// folder itself (a picker may still hold a handle to it).
pub fn remove_generated_contents(destination: &Path) -> Result<(), String> {
    if !destination.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(destination).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() {
            fs::remove_dir_all(&path).map_err(|error| error.to_string())?;
        } else {
            fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> isom::MapNewSpec {
        isom::MapNewSpec {
            version: "remastered".to_string(),
            tileset: 4,
            width: 128,
            height: 96,
            terrain_type: 3,
            title: "테스트".to_string(),
            description: String::new(),
            players: vec![isom::MapNewPlayer {
                slot: 0,
                r#type: "human".to_string(),
                race: "userSelectable".to_string(),
                force: 0,
                start: Some(isom::MapNewStart { x: 128, y: 128 }),
            }],
            forces: vec![isom::MapNewForce {
                name: "Force 1".to_string(),
                allied: true,
                allied_victory: true,
                shared_vision: false,
                random_start: false,
            }],
        }
    }

    #[test]
    fn project_name_is_a_windows_safe_stem() {
        assert_eq!(validate_project_name("  My Map ").unwrap(), "My Map");
        assert_eq!(validate_project_name("한글 맵").unwrap(), "한글 맵");
        for invalid in [
            "",
            "   ",
            "a/b",
            "a\\b",
            "a:b",
            "a[b]",
            "trailing.",
            "CON",
            "nul.scx",
            "tab\tname",
        ] {
            assert!(validate_project_name(invalid).is_err(), "{invalid:?}");
        }
        assert!(validate_project_name(&"x".repeat(PROJECT_NAME_MAX + 1)).is_err());
    }

    #[test]
    fn blank_manifest_uses_bracketed_output_name() {
        let manifest = blank_manifest("Arena").unwrap();
        assert_eq!(manifest.name, "Arena");
        assert_eq!(manifest.source_map, "maps/Arena.scx");
        assert_eq!(manifest.output_map, "build/[EUD]Arena.scx");
        assert_eq!(manifest.main_file, "src/main.eps");
        crate::native_project::validate_manifest(&manifest).unwrap();
    }

    #[test]
    fn spec_validation_rejects_each_boundary() {
        validate_spec(&spec()).unwrap();
        let mut small = spec();
        small.width = 63;
        assert!(validate_spec(&small).unwrap_err().contains("맵 크기"));
        let mut big = spec();
        big.height = 257;
        assert!(validate_spec(&big).is_err());
        let mut tileset = spec();
        tileset.tileset = 8;
        assert!(validate_spec(&tileset).unwrap_err().contains("타일셋"));
        let mut brush = spec();
        brush.terrain_type = 0;
        assert!(validate_spec(&brush).unwrap_err().contains("초기 지형"));
        let mut version = spec();
        version.version = "hybrid".to_string();
        assert!(validate_spec(&version).unwrap_err().contains("버전"));
        let mut forces = spec();
        forces.forces.clear();
        assert!(validate_spec(&forces).unwrap_err().contains("포스"));
        let mut dup = spec();
        dup.players.push(dup.players[0].clone());
        assert!(validate_spec(&dup).unwrap_err().contains("두 번"));
        let mut force_ref = spec();
        force_ref.players[0].force = 1;
        assert!(validate_spec(&force_ref).unwrap_err().contains("포스"));
        let mut outside = spec();
        outside.players[0].start = Some(isom::MapNewStart { x: 128 * 32, y: 0 });
        assert!(validate_spec(&outside).unwrap_err().contains("맵 밖"));
        let mut race = spec();
        race.players[0].race = "xelnaga".to_string();
        assert!(validate_spec(&race).unwrap_err().contains("종족"));
        let mut nine = spec();
        for slot in 1..9 {
            let mut player = nine.players[0].clone();
            player.slot = slot;
            nine.players.push(player);
        }
        assert!(validate_spec(&nine).unwrap_err().contains("최대 8명"));
    }

    #[test]
    fn options_expose_eight_tilesets_and_presets() {
        let tilesets = tileset_options();
        assert_eq!(tilesets.len(), 8);
        assert_eq!(tilesets[0].key, "badlands");
        assert_eq!(tilesets[7].id, 7);
        assert_eq!(size_presets(), vec![64, 96, 128, 192, 256]);
    }

    #[test]
    fn remove_generated_contents_keeps_the_folder() {
        let root = std::env::temp_dir().join(format!(
            "eud-blank-cleanup-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("maps")).unwrap();
        fs::write(root.join("project.eap"), b"{}").unwrap();
        fs::write(root.join("maps").join("x.scx"), b"x").unwrap();
        remove_generated_contents(&root).unwrap();
        assert!(root.exists());
        assert!(fs::read_dir(&root).unwrap().next().is_none());
        fs::remove_dir(&root).unwrap();
    }

    #[test]
    #[ignore = "loads installed StarCraft terrain assets and writes a real blank map"]
    fn creates_a_native_project_around_a_generated_map() {
        let starcraft = std::env::var_os("STARCRAFT_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)\StarCraft"));
        let brushes = tileset_brushes(&starcraft, 4).unwrap();
        let brush = brushes
            .iter()
            .find(|brush| brush.graphics_valid)
            .expect("jungle exposes a graphics-valid brush");
        let mut request_spec = spec();
        request_spec.terrain_type = brush.id;
        let request = BlankProjectRequest {
            destination: String::new(),
            name: "새 맵".to_string(),
            spec: request_spec,
        };
        let destination = std::env::temp_dir().join("한글 폴더").join(format!(
            "eud-blank-project-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let preview = create_blank_project(&destination, &starcraft, &request).unwrap();
        assert_eq!((preview.width, preview.height), (128, 96));
        assert_eq!(preview.tileset, "jungle");
        assert_eq!(preview.players, 1);
        assert_eq!(preview.start_locations, 1);
        assert_eq!(preview.output_map, "build/[EUD]새 맵.scx");
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD
            .decode(&preview.preview_png)
            .unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");

        let project = crate::native_project::NativeProject::open(&destination).unwrap();
        assert_eq!(project.manifest().source_map, "maps/새 맵.scx");
        assert!(project.source_map_path().unwrap().is_file());
        assert!(destination.join("src").join("main.eps").is_file());

        // Real euddraft acceptance: the generated map plus the bracketed output name
        // must survive the EDS `[main]` value position and produce a fresh SCX.
        if let Some(euddraft) = std::env::var_os("EUDDRAFT_PATH") {
            let launch =
                crate::native_build::EuddraftLaunch::resolve(Path::new(&euddraft)).unwrap();
            let compat_root = std::env::var_os("NATIVE_ASSETS_DIR")
                .map(PathBuf::from)
                .expect("NATIVE_ASSETS_DIR must accompany EUDDRAFT_PATH");
            project
                .write_source("src/main.eps", "function onPluginStart() {}\n")
                .unwrap();
            let built =
                crate::native_build::run_native_build(&project, &compat_root, &launch).unwrap();
            assert!(
                built.ok,
                "stdout: {}\nstderr: {}",
                built.stdout, built.stderr
            );
            let output = project.output_map_path().unwrap();
            assert!(output.ends_with(Path::new("build").join("[EUD]새 맵.scx")));
            let bytes = fs::read(&output).unwrap();
            assert!(
                bytes
                    .windows(4)
                    .take(1024)
                    .any(|window| window == b"MPQ\x1a"),
                "built output is not an MPQ container"
            );
        }

        // A second creation into the now-populated folder is refused by the
        // manifest layer before the engine runs, and cleanup leaves the folder.
        assert!(create_blank_project(&destination, &starcraft, &request).is_err());
        remove_generated_contents(&destination).unwrap();
        assert!(fs::read_dir(&destination).unwrap().next().is_none());
        fs::remove_dir(&destination).unwrap();
        fs::remove_dir(destination.parent().unwrap()).ok();
    }

    #[test]
    fn request_rejects_unknown_fields() {
        let json = serde_json::json!({
            "destination": "C:/Work/x",
            "name": "x",
            "spec": serde_json::to_value(spec()).unwrap(),
            "extra": 1,
        });
        assert!(serde_json::from_value::<BlankProjectRequest>(json).is_err());
    }
}
