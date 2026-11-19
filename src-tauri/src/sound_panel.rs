//! The "사운드" tab: every WAV registration of the project's saved source map,
//! what each one holds in the map, the original the project keeps for it,
//! and the bytes of either for the panel to play. Read-only: sounds change
//! only through the agent's sound tools.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::audio::{AudioOriginRecord, AudioService};

const MAX_SOUNDS: usize = 512;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundPanelList {
    /// Changes whenever the saved map changes; the panel polls it.
    pub stamp: String,
    pub map_name: String,
    pub sounds: Vec<SoundPanelEntry>,
    pub total_bytes: u64,
    /// Registrations beyond the listed ones (more than `MAX_SOUNDS`).
    pub omitted: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundPanelEntry {
    pub sound_index: usize,
    pub mpq_path: String,
    /// The file name inside the map (`staredit\wav\` dropped).
    pub name: String,
    pub managed: bool,
    pub in_map: Option<InMapAudio>,
    /// The file the sound was imported from, when the project knows it.
    pub imported_as: Option<AudioFacts>,
    /// The originals it descends from (the imported file itself when that
    /// was the original). Empty when the project keeps none.
    pub originals: Vec<AudioFacts>,
    pub volume_percent: Option<u16>,
    pub fade_in_ms: Option<u64>,
    pub fade_out_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InMapAudio {
    pub bytes: u64,
    pub format: String,
    pub duration_ms: Option<u64>,
    pub channels: Option<u32>,
    pub sample_rate: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioFacts {
    pub sha256: String,
    pub name: String,
    pub codec: String,
    pub duration_ms: u64,
    pub channels: u32,
    pub sample_rate: u32,
    pub bytes: u64,
}

impl From<&AudioOriginRecord> for AudioFacts {
    fn from(origin: &AudioOriginRecord) -> Self {
        Self {
            sha256: origin.sha256.clone(),
            name: origin.display_name.clone(),
            codec: origin.codec.clone(),
            duration_ms: origin.duration_ms,
            channels: origin.channels,
            sample_rate: origin.sample_rate,
            bytes: origin.bytes,
        }
    }
}

/// The saved map's identity for polling: its path, size and write time.
pub fn map_stamp(map_path: &Path) -> String {
    let metadata = std::fs::metadata(map_path);
    let (len, modified) = match metadata {
        Ok(metadata) => (
            metadata.len(),
            metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |time| time.as_nanos()),
        ),
        Err(_) => (0, 0),
    };
    format!("{}|{len}|{modified}", map_path.to_string_lossy())
}

pub fn list(audio: &AudioService, store: &Path, map_path: &Path) -> Result<SoundPanelList, String> {
    let stamp = map_stamp(map_path);
    let chk = isom::chk_extract(map_path).map_err(|_| crate::tr!("sound_panel.map_unreadable"))?;
    let inventory = crate::tool_exec::map_asset_inventory(map_path)?;
    let registered = crate::chk::parse_sounds(&chk);
    let omitted = registered.len().saturating_sub(MAX_SOUNDS);
    let mut sounds = Vec::with_capacity(registered.len().min(MAX_SOUNDS));
    let mut total_bytes = 0_u64;
    for sound in registered.into_iter().take(MAX_SOUNDS) {
        let key = sound.mpq_path.to_ascii_lowercase();
        let in_map = if inventory.contains_key(&key) {
            isom::map_asset(
                map_path,
                &sound.mpq_path,
                crate::attachment::MAX_AUDIO_BYTES,
            )
            .ok()
            .map(|bytes| {
                total_bytes += bytes.len() as u64;
                inspect(&bytes)
            })
        } else {
            None
        };
        let managed = crate::tool_exec::managed_sound_hash(&sound.mpq_path).is_some();
        let record = if managed {
            audio.source_record(store, &sound.mpq_path).ok().flatten()
        } else {
            None
        };
        let imported_as = record.as_ref().map(|record| AudioFacts {
            sha256: record.source_sha256.clone(),
            name: record.source_display_name.clone(),
            codec: record.source_codec.clone(),
            duration_ms: record.source_duration_ms,
            channels: record.source_channels,
            sample_rate: record.source_sample_rate,
            bytes: record.source_bytes,
        });
        let originals = match &record {
            Some(record) if !record.origins.is_empty() => {
                record.origins.iter().map(AudioFacts::from).collect()
            }
            Some(_) => imported_as.iter().cloned().collect(),
            None => Vec::new(),
        };
        sounds.push(SoundPanelEntry {
            sound_index: sound.sound_index,
            name: sound
                .mpq_path
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or(&sound.mpq_path)
                .to_string(),
            mpq_path: sound.mpq_path,
            managed,
            in_map,
            imported_as,
            originals,
            volume_percent: record.as_ref().map(|record| record.effects.volume_percent),
            fade_in_ms: record.as_ref().map(|record| record.effects.fade_in_ms),
            fade_out_ms: record.as_ref().map(|record| record.effects.fade_out_ms),
        });
    }
    Ok(SoundPanelList {
        stamp,
        map_name: map_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        sounds,
        total_bytes,
        omitted,
    })
}

/// The bytes one registration holds in the saved map, checked against the
/// map's own inventory checksum.
pub fn map_audio(map_path: &Path, mpq_path: &str) -> Result<Vec<u8>, String> {
    let chk = isom::chk_extract(map_path).map_err(|_| crate::tr!("sound_panel.map_unreadable"))?;
    let key = mpq_path.to_ascii_lowercase();
    if !crate::chk::parse_sounds(&chk)
        .iter()
        .any(|sound| sound.mpq_path.to_ascii_lowercase() == key)
    {
        return Err(crate::tr!("sound_panel.not_registered", path = mpq_path));
    }
    let inventory = crate::tool_exec::map_asset_inventory(map_path)?;
    let expected = inventory
        .get(&key)
        .ok_or_else(|| crate::tr!("sound_panel.asset_missing", path = mpq_path))?;
    let bytes = isom::map_asset(map_path, mpq_path, crate::attachment::MAX_AUDIO_BYTES)
        .map_err(|_| crate::tr!("sound_panel.asset_missing", path = mpq_path))?;
    if format!("{:x}", Sha256::digest(&bytes)) != *expected {
        return Err(crate::tr!("sound_panel.asset_changed"));
    }
    Ok(bytes)
}

/// Format, length, channels and rate read from the file's own headers: OGG
/// (Vorbis, Opus) and RIFF WAVE. Anything else reports only its size.
pub fn inspect(bytes: &[u8]) -> InMapAudio {
    let mut audio = InMapAudio {
        bytes: bytes.len() as u64,
        format: "unknown".to_string(),
        duration_ms: None,
        channels: None,
        sample_rate: None,
    };
    if bytes.starts_with(b"OggS") {
        inspect_ogg(bytes, &mut audio);
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        inspect_wav(bytes, &mut audio);
    } else if bytes.starts_with(b"ID3") || bytes.first() == Some(&0xff) {
        audio.format = "mp3".to_string();
    }
    audio
}

fn inspect_ogg(bytes: &[u8], audio: &mut InMapAudio) {
    audio.format = "ogg".to_string();
    if bytes.len() < 28 {
        return;
    }
    let segments = bytes[26] as usize;
    let payload = 27 + segments;
    let Some(packet) = bytes.get(payload..) else {
        return;
    };
    // (sample rate, samples to skip at the start)
    let (rate, pre_skip) = if packet.len() >= 16 && packet.starts_with(b"\x01vorbis") {
        audio.format = "ogg vorbis".to_string();
        audio.channels = Some(u32::from(packet[11]));
        (
            u32::from_le_bytes([packet[12], packet[13], packet[14], packet[15]]),
            0,
        )
    } else if packet.len() >= 19 && packet.starts_with(b"OpusHead") {
        audio.format = "ogg opus".to_string();
        audio.channels = Some(u32::from(packet[9]));
        (
            48_000,
            u64::from(u16::from_le_bytes([packet[10], packet[11]])),
        )
    } else {
        return;
    };
    if rate == 0 {
        return;
    }
    audio.sample_rate = Some(rate);
    // The last page's granule position is the stream's last sample.
    let last_page = bytes
        .windows(4)
        .rposition(|window| window == b"OggS")
        .filter(|&page| page + 14 <= bytes.len());
    if let Some(page) = last_page {
        let mut granule = [0_u8; 8];
        granule.copy_from_slice(&bytes[page + 6..page + 14]);
        let samples = i64::from_le_bytes(granule);
        if samples > 0 {
            let samples = (samples as u64).saturating_sub(pre_skip);
            audio.duration_ms = Some(samples * 1000 / u64::from(rate));
        }
    }
}

fn inspect_wav(bytes: &[u8], audio: &mut InMapAudio) {
    audio.format = "wav".to_string();
    let mut offset = 12;
    let mut byte_rate = 0_u32;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let body = offset + 8;
        if id == b"fmt " && body + 12 <= bytes.len() {
            audio.channels = Some(u32::from(u16::from_le_bytes([
                bytes[body + 2],
                bytes[body + 3],
            ])));
            audio.sample_rate = Some(u32::from_le_bytes([
                bytes[body + 4],
                bytes[body + 5],
                bytes[body + 6],
                bytes[body + 7],
            ]));
            byte_rate = u32::from_le_bytes([
                bytes[body + 8],
                bytes[body + 9],
                bytes[body + 10],
                bytes[body + 11],
            ]);
        } else if id == b"data" {
            if byte_rate > 0 {
                let size = size.min(bytes.len().saturating_sub(body)) as u64;
                audio.duration_ms = Some(size * 1000 / u64::from(byte_rate));
            }
            return;
        }
        offset = body + size + (size & 1);
    }
}

fn project_paths(
    registry: &crate::project_registry::ProjectRegistry,
    label: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let handle = registry.for_window(label)?;
    let root = handle.root().to_path_buf();
    let map_path = handle.native()?.source_map_path()?;
    Ok((root, map_path))
}

fn store_for(
    audio: &AudioService,
    dirs: &crate::config::DataDirs,
    root: &Path,
) -> Result<PathBuf, String> {
    let workspace_id = crate::workspace::WorkspaceManager::for_root(dirs.clone(), root)
        .prepare_current()?
        .id;
    audio.project_store(root, &workspace_id)
}

#[tauri::command]
pub async fn sound_panel_list(
    window: tauri::WebviewWindow,
    registry: tauri::State<'_, crate::project_registry::ProjectRegistry>,
) -> Result<SoundPanelList, String> {
    let (root, map_path) = project_paths(&registry, window.label())?;
    let dirs = registry.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let audio = AudioService::new(dirs.clone());
        let store = store_for(&audio, &dirs, &root)?;
        list(&audio, &store, &map_path)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn sound_panel_stamp(
    window: tauri::WebviewWindow,
    registry: tauri::State<'_, crate::project_registry::ProjectRegistry>,
) -> Result<String, String> {
    let (_, map_path) = project_paths(&registry, window.label())?;
    Ok(map_stamp(&map_path))
}

#[tauri::command]
pub async fn sound_panel_map_audio(
    window: tauri::WebviewWindow,
    registry: tauri::State<'_, crate::project_registry::ProjectRegistry>,
    mpq_path: String,
) -> Result<tauri::ipc::Response, String> {
    let (_, map_path) = project_paths(&registry, window.label())?;
    tauri::async_runtime::spawn_blocking(move || map_audio(&map_path, &mpq_path))
        .await
        .map_err(|error| error.to_string())?
        .map(tauri::ipc::Response::new)
}

#[tauri::command]
pub async fn sound_panel_original_audio(
    window: tauri::WebviewWindow,
    registry: tauri::State<'_, crate::project_registry::ProjectRegistry>,
    sha256: String,
) -> Result<tauri::ipc::Response, String> {
    let (root, _) = project_paths(&registry, window.label())?;
    let dirs = registry.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let audio = AudioService::new(dirs.clone());
        let store = store_for(&audio, &dirs, &root)?;
        audio.stored_original(&store, &sha256)
    })
    .await
    .map_err(|error| error.to_string())?
    .map(tauri::ipc::Response::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_headers_give_length_channels_and_rate() {
        // 0.5 s of 16-bit stereo at 22050 Hz.
        let data_len = 22_050 * 2 * 2 / 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&22_050_u32.to_le_bytes());
        wav.extend_from_slice(&(22_050_u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data_len as u32).to_le_bytes());
        wav.resize(wav.len() + data_len, 0);
        let audio = inspect(&wav);
        assert_eq!(audio.format, "wav");
        assert_eq!(audio.duration_ms, Some(500));
        assert_eq!(audio.channels, Some(2));
        assert_eq!(audio.sample_rate, Some(22_050));
    }

    #[test]
    fn ogg_vorbis_length_comes_from_the_last_granule() {
        let ogg = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/isom/tests/fixtures/tone.ogg"),
        )
        .unwrap();
        let audio = inspect(&ogg);
        assert_eq!(audio.format, "ogg vorbis");
        assert!(audio.sample_rate.is_some_and(|rate| rate > 0), "{audio:?}");
        assert!(audio.duration_ms.is_some_and(|ms| ms > 0), "{audio:?}");
    }

    #[test]
    fn lists_map_sounds_and_plays_what_the_map_holds() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/isom/tests/fixtures");
        let root = std::env::temp_dir().join(format!("sound-panel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let plain = root.join("plain.scx");
        std::fs::copy(fixtures.join("map_agent_rich.scx"), &plain).unwrap();
        let ogg = std::fs::read(fixtures.join("tone.ogg")).unwrap();
        let sha = format!("{:x}", Sha256::digest(&ogg));
        let path = format!("staredit\\wav\\ea_{}.ogg", &sha[..16]);
        let map = root.join("source.scx");
        isom::map_sound_add(
            &plain,
            &map,
            &crate::bootstrap::sha256_file(&plain).unwrap(),
            &path,
            &ogg,
        )
        .unwrap();
        let dirs = crate::config::DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        let audio = AudioService::new(dirs);
        let listed = list(&audio, &root.join(crate::audio::PROJECT_AUDIO_STORE), &map).unwrap();

        let sound = listed
            .sounds
            .iter()
            .find(|sound| sound.mpq_path == path)
            .unwrap();
        assert!(sound.managed);
        assert_eq!(sound.name, format!("ea_{}.ogg", &sha[..16]));
        let in_map = sound.in_map.as_ref().unwrap();
        assert_eq!(in_map.bytes, ogg.len() as u64);
        assert!(in_map.duration_ms.is_some());
        assert!(sound.originals.is_empty());
        assert!(listed.total_bytes >= ogg.len() as u64);
        assert_eq!(listed.stamp, map_stamp(&map));

        assert_eq!(map_audio(&map, &path).unwrap(), ogg);
        assert!(map_audio(&map, "staredit\\wav\\nothing.wav").is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unknown_bytes_report_only_their_size() {
        let audio = inspect(b"not audio at all");
        assert_eq!(audio.format, "unknown");
        assert_eq!(audio.bytes, 16);
        assert_eq!(audio.duration_ms, None);
    }
}
