//! Managed FFprobe/FFmpeg audio validation and canonical OGG normalization.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::attachment::{AttachmentDescriptor, ResolvedAudioAttachment, MAX_AUDIO_BYTES};
use crate::bootstrap;
use crate::config::DataDirs;
use crate::memory::write_atomic_bytes;

pub const MAX_AUDIO_DURATION_MS: u64 = 3_600_000;
pub const MAX_PROBE_STDOUT: usize = 256 * 1024;
pub const MAX_NORMALIZED_AUDIO_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_AUDIO_CHANNELS_IN: u32 = 8;
const MAX_PROCESS_STDERR: usize = 256 * 1024;
const MAX_SAMPLE_RATE: u32 = 384_000;
const PROBE_DEADLINE: Duration = Duration::from_secs(30);
const TRANSCODE_DEADLINE: Duration = Duration::from_secs(5 * 60);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(25);
const FORMAT_ALLOWLIST: &[&str] = &[
    "wav", "ogg", "flac", "mp3", "aac", "mov", "mp4", "m4a", "aiff", "asf",
];

const AUDIO_SOURCE_RECORD_SCHEMA: &str = "eud-project-audio-source/1";
pub const MAX_VOLUME_PERCENT: u16 = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioEffects {
    pub volume_percent: u16,
    pub fade_in_ms: u64,
    pub fade_out_ms: u64,
}

impl Default for AudioEffects {
    fn default() -> Self {
        Self {
            volume_percent: 100,
            fade_in_ms: 0,
            fade_out_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioEditPatch {
    pub volume_percent: Option<u16>,
    pub fade_in_ms: Option<u64>,
    pub fade_out_ms: Option<u64>,
}

impl AudioEditPatch {
    fn apply(self, current: AudioEffects, duration_ms: u64) -> Result<AudioEffects, String> {
        let edited = AudioEffects {
            volume_percent: self.volume_percent.unwrap_or(current.volume_percent),
            fade_in_ms: self.fade_in_ms.unwrap_or(current.fade_in_ms),
            fade_out_ms: self.fade_out_ms.unwrap_or(current.fade_out_ms),
        };
        validate_effects(edited, duration_ms)?;
        if edited == current {
            return Err("요청한 오디오 편집값이 현재 값과 같습니다.".to_string());
        }
        Ok(edited)
    }

    pub fn is_empty(self) -> bool {
        self.volume_percent.is_none() && self.fade_in_ms.is_none() && self.fade_out_ms.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectAudioSource {
    schema: String,
    pub mpq_path: String,
    pub source_sha256: String,
    pub source_display_name: String,
    pub source_codec: String,
    pub source_duration_ms: u64,
    pub source_channels: u32,
    pub source_sample_rate: u32,
    pub source_bytes: u64,
    pub effects: AudioEffects,
}

#[derive(Debug, Clone)]
pub struct EditedAudio {
    pub source: ProjectAudioSource,
    pub previous_effects: AudioEffects,
    pub effects: AudioEffects,
    pub normalized: NormalizedAudio,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioProbe {
    pub codec: String,
    pub duration_ms: u64,
    pub channels: u32,
    pub sample_rate: u32,
    pub format: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedAudioRef {
    pub audio_ref: String,
    pub name: String,
    pub duration_ms: u64,
    pub codec: String,
    pub channels: u32,
    pub sample_rate: u32,
}

#[derive(Debug, Clone)]
pub struct NormalizedAudio {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    pub duration_ms: u64,
    pub source_codec: String,
    pub profile_version: String,
}

#[derive(Debug)]
pub struct RequestAudioTemp {
    path: PathBuf,
}

impl RequestAudioTemp {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RequestAudioTemp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug, Clone)]
pub struct AudioBinding {
    pub descriptor: AttachmentDescriptor,
    pub source_path: PathBuf,
    pub source_sha256: String,
    pub probe: AudioProbe,
    pub audio_ref: String,
    request_temp: Arc<RequestAudioTemp>,
    normalized: Arc<Mutex<Option<NormalizedAudio>>>,
}

impl AudioBinding {
    pub fn trusted_ref(&self) -> TrustedAudioRef {
        TrustedAudioRef {
            audio_ref: self.audio_ref.clone(),
            name: self.descriptor.name.clone(),
            duration_ms: self.probe.duration_ms,
            codec: self.probe.codec.clone(),
            channels: self.probe.channels,
            sample_rate: self.probe.sample_rate,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AudioService {
    dirs: DataDirs,
}

impl AudioService {
    pub fn new(dirs: DataDirs) -> Self {
        Self { dirs }
    }

    pub fn request_temp(&self) -> Result<Arc<RequestAudioTemp>, String> {
        let path = self
            .dirs
            .audio_temp_dir()
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&path)
            .map_err(|error| format!("오디오 임시 폴더를 만들 수 없습니다: {error}"))?;
        Ok(Arc::new(RequestAudioTemp { path }))
    }

    pub fn bind(
        &self,
        attachment: ResolvedAudioAttachment,
        audio_ref: String,
        request_temp: Arc<RequestAudioTemp>,
        cancellation: Option<&tokio::sync::watch::Receiver<u64>>,
    ) -> Result<AudioBinding, String> {
        validate_source_file(&attachment)?;
        let tools = bootstrap::resolve_managed_ffmpeg(&self.dirs)
            .map_err(|_| "관리되는 오디오 변환기 자산이 없거나 손상되었습니다.".to_string())?;
        let probe = probe_file(&tools.ffprobe, &attachment.path, cancellation)?;
        validate_source_file(&attachment)?;
        Ok(AudioBinding {
            descriptor: attachment.descriptor,
            source_path: attachment.path,
            source_sha256: attachment.sha256,
            probe,
            audio_ref,
            request_temp,
            normalized: Arc::new(Mutex::new(None)),
        })
    }

    pub fn normalize(
        &self,
        binding: &AudioBinding,
        cancellation: Option<&tokio::sync::watch::Receiver<u64>>,
    ) -> Result<NormalizedAudio, String> {
        if let Some(cached) = binding
            .normalized
            .lock()
            .map_err(|_| "오디오 변환 cache lock이 손상되었습니다.".to_string())?
            .clone()
        {
            validate_cached_output(&cached)?;
            return Ok(cached);
        }

        validate_bound_source(binding)?;
        let output = binding
            .request_temp
            .path()
            .join(format!("{}.ogg", binding.audio_ref));
        let normalized = transcode_canonical(
            &self.dirs,
            &binding.source_path,
            &binding.probe,
            &output,
            AudioEffects::default(),
            cancellation,
        )?;
        *binding
            .normalized
            .lock()
            .map_err(|_| "오디오 변환 cache lock이 손상되었습니다.".to_string())? =
            Some(normalized.clone());
        Ok(normalized)
    }

    pub fn remember_import(
        &self,
        project_id: &str,
        mpq_path: &str,
        binding: &AudioBinding,
    ) -> Result<ProjectAudioSource, String> {
        if let Some(existing) = self.source_record(project_id, mpq_path)? {
            return Ok(existing);
        }
        validate_bound_source(binding)?;
        let source_bytes =
            self.persist_source_blob(project_id, &binding.source_path, &binding.source_sha256)?;
        let record = ProjectAudioSource {
            schema: AUDIO_SOURCE_RECORD_SCHEMA.to_string(),
            mpq_path: mpq_path.to_string(),
            source_sha256: binding.source_sha256.clone(),
            source_display_name: binding.descriptor.name.clone(),
            source_codec: binding.probe.codec.clone(),
            source_duration_ms: binding.probe.duration_ms,
            source_channels: binding.probe.channels,
            source_sample_rate: binding.probe.sample_rate,
            source_bytes,
            effects: AudioEffects::default(),
        };
        self.write_source_record(project_id, &record)?;
        Ok(record)
    }

    pub fn source_record(
        &self,
        project_id: &str,
        mpq_path: &str,
    ) -> Result<Option<ProjectAudioSource>, String> {
        let path = self.source_record_path(project_id, mpq_path)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "프로젝트 오디오 원본 기록을 읽을 수 없습니다: {error}"
                ))
            }
        };
        let record: ProjectAudioSource = serde_json::from_slice(&bytes)
            .map_err(|_| "프로젝트 오디오 원본 기록이 손상되었습니다.".to_string())?;
        validate_source_record(&record, mpq_path)?;
        let blob = self.source_blob_path(project_id, &record.source_sha256)?;
        let metadata = stable_file_metadata(&blob, MAX_AUDIO_BYTES)?;
        if metadata.0 != record.source_bytes {
            return Err("프로젝트 오디오 원본 크기가 변경되었습니다.".to_string());
        }
        Ok(Some(record))
    }

    pub fn render_edit(
        &self,
        project_id: &str,
        mpq_path: &str,
        patch: AudioEditPatch,
        request_temp: &RequestAudioTemp,
        cancellation: Option<&tokio::sync::watch::Receiver<u64>>,
    ) -> Result<EditedAudio, String> {
        if patch.is_empty() {
            return Err("오디오 편집값을 하나 이상 지정해야 합니다.".to_string());
        }
        let source = self
            .source_record(project_id, mpq_path)?
            .ok_or_else(|| "이 사운드의 프로젝트 원본이 없습니다.".to_string())?;
        let source_path = self.source_blob_path(project_id, &source.source_sha256)?;
        if sha256_file(&source_path)? != source.source_sha256 {
            return Err("프로젝트 오디오 원본 checksum이 변경되었습니다.".to_string());
        }
        let tools = bootstrap::resolve_managed_ffmpeg(&self.dirs)
            .map_err(|_| "관리되는 오디오 변환기 자산이 없거나 손상되었습니다.".to_string())?;
        let probe = probe_file(&tools.ffprobe, &source_path, cancellation)?;
        let tolerance = 250_u64.max(source.source_duration_ms / 200);
        if probe.duration_ms.abs_diff(source.source_duration_ms) > tolerance {
            return Err("프로젝트 오디오 원본 길이가 변경되었습니다.".to_string());
        }
        let effects = patch.apply(source.effects, source.source_duration_ms)?;
        let output = request_temp
            .path()
            .join(format!("edit-{}.ogg", uuid::Uuid::new_v4()));
        let normalized = transcode_canonical(
            &self.dirs,
            &source_path,
            &probe,
            &output,
            effects,
            cancellation,
        )?;
        Ok(EditedAudio {
            previous_effects: source.effects,
            source,
            effects,
            normalized,
        })
    }

    pub fn remember_edit(
        &self,
        project_id: &str,
        mpq_path: &str,
        edited: &EditedAudio,
    ) -> Result<ProjectAudioSource, String> {
        let mut record = edited.source.clone();
        record.mpq_path = mpq_path.to_string();
        record.effects = edited.effects;
        self.write_source_record(project_id, &record)?;
        Ok(record)
    }

    fn persist_source_blob(
        &self,
        project_id: &str,
        source: &Path,
        expected_sha256: &str,
    ) -> Result<u64, String> {
        validate_project_id(project_id)?;
        validate_sha256(expected_sha256)?;
        let before = stable_file_metadata(source, MAX_AUDIO_BYTES)?;
        let blob = self.source_blob_path(project_id, expected_sha256)?;
        if blob.is_file() {
            let metadata = stable_file_metadata(&blob, MAX_AUDIO_BYTES)?;
            if metadata.0 != before.0 || sha256_file(&blob)? != expected_sha256 {
                return Err("프로젝트 오디오 원본 저장소가 손상되었습니다.".to_string());
            }
            return Ok(metadata.0);
        }
        let parent = blob
            .parent()
            .ok_or_else(|| "프로젝트 오디오 원본 경로가 올바르지 않습니다.".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("프로젝트 오디오 원본 폴더를 만들 수 없습니다: {error}"))?;
        let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let copied = fs::copy(source, &temporary)
            .map_err(|error| format!("프로젝트 오디오 원본을 복사할 수 없습니다: {error}"))?;
        let after = stable_file_metadata(source, MAX_AUDIO_BYTES)?;
        let staged = stable_file_metadata(&temporary, MAX_AUDIO_BYTES)?;
        if before != after
            || copied != before.0
            || staged.0 != before.0
            || sha256_file(&temporary)? != expected_sha256
        {
            let _ = fs::remove_file(&temporary);
            return Err("프로젝트 오디오 원본이 복사 중 변경되었습니다.".to_string());
        }
        if let Err(error) = fs::rename(&temporary, &blob) {
            let _ = fs::remove_file(&temporary);
            if !blob.is_file()
                || stable_file_metadata(&blob, MAX_AUDIO_BYTES)?.0 != before.0
                || sha256_file(&blob)? != expected_sha256
            {
                return Err(format!(
                    "프로젝트 오디오 원본을 확정할 수 없습니다: {error}"
                ));
            }
        }
        Ok(before.0)
    }

    fn write_source_record(
        &self,
        project_id: &str,
        record: &ProjectAudioSource,
    ) -> Result<(), String> {
        validate_source_record(record, &record.mpq_path)?;
        if let Some(existing) = self.source_record(project_id, &record.mpq_path)? {
            if existing == *record {
                return Ok(());
            }
            return Err(
                "같은 MPQ 사운드 경로가 다른 프로젝트 원본 기록에 연결되어 있습니다.".to_string(),
            );
        }
        let path = self.source_record_path(project_id, &record.mpq_path)?;
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|_| "프로젝트 오디오 원본 기록을 만들 수 없습니다.".to_string())?;
        write_atomic_bytes(&path, &bytes)
            .map_err(|error| format!("프로젝트 오디오 원본 기록을 저장할 수 없습니다: {error}"))
    }

    fn source_record_path(&self, project_id: &str, mpq_path: &str) -> Result<PathBuf, String> {
        validate_project_id(project_id)?;
        validate_managed_sound_path(mpq_path)?;
        let key = sha256_bytes(mpq_path.to_ascii_lowercase().as_bytes());
        Ok(self
            .dirs
            .audio_sources_dir()
            .join(project_id)
            .join("sounds")
            .join(format!("{key}.json")))
    }

    fn source_blob_path(&self, project_id: &str, sha256: &str) -> Result<PathBuf, String> {
        validate_project_id(project_id)?;
        validate_sha256(sha256)?;
        Ok(self
            .dirs
            .audio_sources_dir()
            .join(project_id)
            .join("blobs")
            .join(sha256))
    }
}

fn transcode_canonical(
    dirs: &DataDirs,
    source: &Path,
    source_probe: &AudioProbe,
    output: &Path,
    effects: AudioEffects,
    cancellation: Option<&tokio::sync::watch::Receiver<u64>>,
) -> Result<NormalizedAudio, String> {
    validate_effects(effects, source_probe.duration_ms)?;
    let tools = bootstrap::resolve_managed_ffmpeg(dirs)
        .map_err(|_| "관리되는 오디오 변환기 자산이 없거나 손상되었습니다.".to_string())?;
    let _ = fs::remove_file(output);
    let args = canonical_transcode_args(source, output, effects, source_probe.duration_ms);
    let process = run_managed_process(
        &tools.ffmpeg,
        &args,
        TRANSCODE_DEADLINE,
        1,
        MAX_PROCESS_STDERR,
        cancellation,
    );
    let process = match process {
        Ok(process) => process,
        Err(error) => {
            let _ = fs::remove_file(output);
            return Err(error);
        }
    };
    if !process.success {
        let _ = fs::remove_file(output);
        return Err("오디오 파일이 손상되었거나 디코딩할 수 없습니다.".to_string());
    }

    let normalized = (|| {
        let before = stable_file_metadata(output, MAX_NORMALIZED_AUDIO_BYTES)?;
        let bytes = fs::read(output).map_err(|_| "정규화된 OGG를 읽을 수 없습니다.".to_string())?;
        let after = stable_file_metadata(output, MAX_NORMALIZED_AUDIO_BYTES)?;
        if before != after {
            return Err("정규화된 OGG가 검증 중 변경되었습니다.".to_string());
        }
        if !bytes.starts_with(b"OggS") {
            return Err("정규화된 OGG magic 검증에 실패했습니다.".to_string());
        }
        let output_probe = probe_file(&tools.ffprobe, output, cancellation)?;
        if output_probe.codec != "vorbis"
            || output_probe.sample_rate != 44_100
            || output_probe.channels != 2
        {
            return Err("정규화된 OGG codec profile 검증에 실패했습니다.".to_string());
        }
        let tolerance = 250_u64.max(source_probe.duration_ms / 200);
        if output_probe.duration_ms.abs_diff(source_probe.duration_ms) > tolerance {
            return Err("정규화된 OGG 길이 검증에 실패했습니다.".to_string());
        }
        Ok(NormalizedAudio {
            path: output.to_path_buf(),
            sha256: sha256_bytes(&bytes),
            bytes: bytes.len() as u64,
            duration_ms: output_probe.duration_ms,
            source_codec: source_probe.codec.clone(),
            profile_version: tools.version,
        })
    })();
    if normalized.is_err() {
        let _ = fs::remove_file(output);
    }
    normalized
}

fn canonical_transcode_args(
    source: &Path,
    output: &Path,
    effects: AudioEffects,
    duration_ms: u64,
) -> Vec<String> {
    let mut args = vec![
        "-nostdin".to_string(),
        "-v".to_string(),
        "error".to_string(),
        "-protocol_whitelist".to_string(),
        "file".to_string(),
        "-i".to_string(),
        source.to_string_lossy().into_owned(),
        "-map".to_string(),
        "0:a:0".to_string(),
        "-vn".to_string(),
        "-sn".to_string(),
        "-dn".to_string(),
        "-map_metadata".to_string(),
        "-1".to_string(),
        "-map_chapters".to_string(),
        "-1".to_string(),
    ];
    if let Some(filter) = audio_filter(effects, duration_ms) {
        args.extend(["-af".to_string(), filter]);
    }
    args.extend([
        "-ar".to_string(),
        "44100".to_string(),
        "-ac".to_string(),
        "2".to_string(),
        "-c:a".to_string(),
        "libvorbis".to_string(),
        "-q:a".to_string(),
        "4".to_string(),
        "-f".to_string(),
        "ogg".to_string(),
        "-y".to_string(),
        output.to_string_lossy().into_owned(),
    ]);
    args
}

fn audio_filter(effects: AudioEffects, duration_ms: u64) -> Option<String> {
    let mut filters = Vec::with_capacity(3);
    if effects.volume_percent != 100 {
        filters.push(format!("volume={}/100", effects.volume_percent));
    }
    if effects.fade_in_ms != 0 {
        filters.push(format!("afade=t=in:st=0:d={}", seconds(effects.fade_in_ms)));
    }
    if effects.fade_out_ms != 0 {
        filters.push(format!(
            "afade=t=out:st={}:d={}",
            seconds(duration_ms - effects.fade_out_ms),
            seconds(effects.fade_out_ms)
        ));
    }
    (!filters.is_empty()).then(|| filters.join(","))
}

fn seconds(milliseconds: u64) -> String {
    format!("{}.{:03}", milliseconds / 1_000, milliseconds % 1_000)
}

fn validate_effects(effects: AudioEffects, duration_ms: u64) -> Result<(), String> {
    if effects.volume_percent > MAX_VOLUME_PERCENT {
        return Err(format!(
            "오디오 볼륨은 0~{MAX_VOLUME_PERCENT}% 범위여야 합니다."
        ));
    }
    if duration_ms == 0
        || effects.fade_in_ms > duration_ms
        || effects.fade_out_ms > duration_ms
        || effects.fade_in_ms > duration_ms.saturating_sub(effects.fade_out_ms)
    {
        return Err("페이드인과 페이드아웃 합계가 오디오 길이를 초과합니다.".to_string());
    }
    Ok(())
}

fn validate_source_record(
    record: &ProjectAudioSource,
    expected_mpq_path: &str,
) -> Result<(), String> {
    validate_managed_sound_path(expected_mpq_path)?;
    validate_sha256(&record.source_sha256)?;
    validate_effects(record.effects, record.source_duration_ms)?;
    if record.schema != AUDIO_SOURCE_RECORD_SCHEMA
        || record.mpq_path != expected_mpq_path
        || record.source_display_name.trim().is_empty()
        || record.source_codec.trim().is_empty()
        || record.source_duration_ms == 0
        || record.source_duration_ms > MAX_AUDIO_DURATION_MS
        || record.source_channels == 0
        || record.source_channels > MAX_AUDIO_CHANNELS_IN
        || record.source_sample_rate == 0
        || record.source_sample_rate > MAX_SAMPLE_RATE
        || record.source_bytes == 0
        || record.source_bytes > MAX_AUDIO_BYTES as u64
    {
        return Err("프로젝트 오디오 원본 기록이 올바르지 않습니다.".to_string());
    }
    Ok(())
}

fn validate_project_id(project_id: &str) -> Result<(), String> {
    if project_id.len() == 64
        && project_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err("프로젝트 오디오 저장소 ID가 올바르지 않습니다.".to_string())
    }
}

fn validate_sha256(sha256: &str) -> Result<(), String> {
    if sha256.len() == 64
        && sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err("오디오 checksum이 올바르지 않습니다.".to_string())
    }
}

fn validate_managed_sound_path(path: &str) -> Result<(), String> {
    let Some(hash) = path
        .strip_prefix("staredit\\wav\\ea_")
        .and_then(|path| path.strip_suffix(".ogg"))
    else {
        return Err("관리형 MPQ sound path가 올바르지 않습니다.".to_string());
    };
    if matches!(hash.len(), 16 | 24 | 32 | 64)
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err("관리형 MPQ sound path가 올바르지 않습니다.".to_string())
    }
}

fn validate_source_file(attachment: &ResolvedAudioAttachment) -> Result<(), String> {
    let metadata = stable_file_metadata(&attachment.path, MAX_AUDIO_BYTES)?;
    if metadata.0 != attachment.descriptor.size {
        return Err("첨부 오디오 크기가 staging 이후 변경되었습니다.".to_string());
    }
    let actual = sha256_file(&attachment.path)?;
    if actual != attachment.sha256 {
        return Err("첨부 오디오 checksum이 staging 이후 변경되었습니다.".to_string());
    }
    Ok(())
}

fn validate_bound_source(binding: &AudioBinding) -> Result<(), String> {
    let attachment = ResolvedAudioAttachment {
        descriptor: binding.descriptor.clone(),
        path: binding.source_path.clone(),
        sha256: binding.source_sha256.clone(),
    };
    validate_source_file(&attachment)
}

fn validate_cached_output(output: &NormalizedAudio) -> Result<(), String> {
    let metadata = stable_file_metadata(&output.path, MAX_NORMALIZED_AUDIO_BYTES)?;
    if metadata.0 != output.bytes || sha256_file(&output.path)? != output.sha256 {
        return Err("정규화된 OGG cache가 변경되었습니다.".to_string());
    }
    Ok(())
}

fn stable_file_metadata(path: &Path, cap: usize) -> Result<(u64, Option<SystemTime>), String> {
    let metadata = fs::metadata(path).map_err(|_| "오디오 파일을 찾을 수 없습니다.".to_string())?;
    let size = metadata.len();
    if size == 0 || size > cap as u64 {
        return Err("오디오 파일 크기 제한을 초과했습니다.".to_string());
    }
    Ok((size, metadata.modified().ok()))
}

fn probe_file(
    ffprobe: &Path,
    path: &Path,
    cancellation: Option<&tokio::sync::watch::Receiver<u64>>,
) -> Result<AudioProbe, String> {
    let args = vec![
        "-v".to_string(),
        "error".to_string(),
        "-protocol_whitelist".to_string(),
        "file".to_string(),
        "-show_entries".to_string(),
        "stream=codec_type,codec_name,channels,sample_rate,duration:format=format_name,duration"
            .to_string(),
        "-of".to_string(),
        "json".to_string(),
        "--".to_string(),
        path.to_string_lossy().into_owned(),
    ];
    let output = run_managed_process(
        ffprobe,
        &args,
        PROBE_DEADLINE,
        MAX_PROBE_STDOUT,
        MAX_PROCESS_STDERR,
        cancellation,
    )?;
    if !output.success {
        return Err("오디오 파일이 손상되었거나 probe할 수 없습니다.".to_string());
    }
    parse_probe_json(&output.stdout)
}

#[derive(Debug, Deserialize)]
struct ProbeDocument {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    channels: Option<u32>,
    sample_rate: Option<String>,
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    format_name: Option<String>,
    duration: Option<String>,
}

fn parse_probe_json(bytes: &[u8]) -> Result<AudioProbe, String> {
    let document: ProbeDocument = serde_json::from_slice(bytes)
        .map_err(|_| "FFprobe 응답 형식이 올바르지 않습니다.".to_string())?;
    let format = document
        .format
        .as_ref()
        .and_then(|format| format.format_name.as_deref())
        .ok_or_else(|| "지원하지 않는 오디오 컨테이너입니다.".to_string())?;
    if format.len() > 128
        || !format.is_ascii()
        || !format
            .split(',')
            .any(|name| FORMAT_ALLOWLIST.contains(&name))
    {
        return Err("지원하지 않는 오디오 컨테이너입니다.".to_string());
    }
    let stream = document
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("audio"))
        .ok_or_else(|| "오디오 스트림이 없습니다.".to_string())?;
    let codec = stream
        .codec_name
        .as_deref()
        .filter(|codec| !codec.is_empty() && codec.len() <= 64 && codec.is_ascii())
        .ok_or_else(|| "오디오 codec 정보가 올바르지 않습니다.".to_string())?;
    let channels = stream
        .channels
        .filter(|channels| (1..=MAX_AUDIO_CHANNELS_IN).contains(channels))
        .ok_or_else(|| "오디오 채널 수가 1..8 범위를 벗어났습니다.".to_string())?;
    let sample_rate = stream
        .sample_rate
        .as_deref()
        .and_then(|rate| rate.parse::<u32>().ok())
        .filter(|rate| (1..=MAX_SAMPLE_RATE).contains(rate))
        .ok_or_else(|| "오디오 sample rate가 올바르지 않습니다.".to_string())?;
    let duration = stream
        .duration
        .as_deref()
        .or_else(|| {
            document
                .format
                .as_ref()
                .and_then(|format| format.duration.as_deref())
        })
        .and_then(|duration| duration.parse::<f64>().ok())
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or_else(|| "오디오 길이가 올바르지 않습니다.".to_string())?;
    let duration_ms = (duration * 1000.0).round();
    if duration_ms > MAX_AUDIO_DURATION_MS as f64 {
        return Err("오디오 길이는 60분 이하여야 합니다.".to_string());
    }
    Ok(AudioProbe {
        codec: codec.to_string(),
        duration_ms: duration_ms as u64,
        channels,
        sample_rate,
        format: format.to_string(),
    })
}

#[derive(Debug)]
struct ProcessOutput {
    success: bool,
    stdout: Vec<u8>,
    #[allow(dead_code)]
    stderr: Vec<u8>,
}

fn run_managed_process(
    executable: &Path,
    args: &[String],
    deadline: Duration,
    stdout_cap: usize,
    stderr_cap: usize,
    cancellation: Option<&tokio::sync::watch::Receiver<u64>>,
) -> Result<ProcessOutput, String> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "관리되는 오디오 변환기를 실행할 수 없습니다.".to_string())?;
    #[cfg(windows)]
    let mut job = match crate::eps_preflight::WindowsJob::assign(&child) {
        Ok(job) => Some(job),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("오디오 변환기 process containment를 설정할 수 없습니다.".to_string());
        }
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "오디오 변환기 stdout pipe가 없습니다.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "오디오 변환기 stderr pipe가 없습니다.".to_string())?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, stdout_cap));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, stderr_cap));
    let started = Instant::now();
    let cancel_generation = cancellation.map(|receiver| *receiver.borrow());
    let status = loop {
        if cancellation
            .zip(cancel_generation)
            .is_some_and(|(receiver, generation)| *receiver.borrow() != generation)
        {
            #[cfg(windows)]
            if let Some(job) = job.take() {
                job.terminate();
            }
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err("오디오 변환이 취소되었습니다.".to_string());
        }
        if started.elapsed() >= deadline {
            #[cfg(windows)]
            if let Some(job) = job.take() {
                job.terminate();
            }
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err("오디오 변환 시간이 초과되었습니다.".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(PROCESS_POLL_INTERVAL),
            Err(_) => {
                #[cfg(windows)]
                if let Some(job) = job.take() {
                    job.terminate();
                }
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err("오디오 변환기 상태를 확인할 수 없습니다.".to_string());
            }
        }
    };
    #[cfg(windows)]
    drop(job.take());
    let stdout = stdout_reader
        .join()
        .map_err(|_| "오디오 변환기 stdout reader가 중단되었습니다.".to_string())??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "오디오 변환기 stderr reader가 중단되었습니다.".to_string())??;
    Ok(ProcessOutput {
        success: status.success(),
        stdout,
        stderr,
    })
}

fn read_bounded(mut stream: impl Read, cap: usize) -> Result<Vec<u8>, String> {
    let mut kept = Vec::with_capacity(cap.min(16 * 1024));
    let mut total = 0usize;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let count = stream
            .read(&mut buffer)
            .map_err(|_| "오디오 변환기 출력을 읽을 수 없습니다.".to_string())?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count)
            .ok_or_else(|| "오디오 변환기 출력 크기가 overflow되었습니다.".to_string())?;
        if kept.len() < cap {
            let take = count.min(cap - kept.len());
            kept.extend_from_slice(&buffer[..take]);
        }
    }
    if total > cap {
        return Err("오디오 변환기 출력 크기 제한을 초과했습니다.".to_string());
    }
    Ok(kept)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|_| "오디오 파일을 읽을 수 없습니다.".to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "오디오 파일을 읽을 수 없습니다.".to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_json_selects_first_audio_and_rejects_untrusted_formats() {
        let parsed = parse_probe_json(
            br#"{"streams":[{"codec_type":"video","codec_name":"mjpeg"},{"codec_type":"audio","codec_name":"aac","channels":2,"sample_rate":"48000","duration":"1.250"}],"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"1.250"}}"#,
        )
        .unwrap();
        assert_eq!(parsed.codec, "aac");
        assert_eq!(parsed.duration_ms, 1_250);
        assert_eq!(parsed.channels, 2);
        assert_eq!(parsed.sample_rate, 48_000);

        assert!(parse_probe_json(
            br#"{"streams":[{"codec_type":"audio","codec_name":"aac","channels":2,"sample_rate":"48000","duration":"1"}],"format":{"format_name":"hls","duration":"1"}}"#,
        )
        .unwrap_err()
        .contains("지원하지 않는"));
    }

    #[test]
    fn probe_json_enforces_audio_stream_duration_and_channel_caps() {
        assert!(parse_probe_json(
            br#"{"streams":[],"format":{"format_name":"wav","duration":"1"}}"#,
        )
        .unwrap_err()
        .contains("스트림"));
        assert!(parse_probe_json(
            br#"{"streams":[{"codec_type":"audio","codec_name":"pcm_s16le","channels":9,"sample_rate":"44100","duration":"1"}],"format":{"format_name":"wav","duration":"1"}}"#,
        )
        .unwrap_err()
        .contains("채널"));
        assert!(parse_probe_json(
            br#"{"streams":[{"codec_type":"audio","codec_name":"pcm_s16le","channels":1,"sample_rate":"44100","duration":"3600.001"}],"format":{"format_name":"wav","duration":"3600.001"}}"#,
        )
        .unwrap_err()
        .contains("60분"));
    }

    #[test]
    fn managed_converter_missing_or_checksum_mismatch_is_feature_scoped() {
        let root = integration_root("managed-checksum");
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        assert!(bootstrap::resolve_managed_ffmpeg(&dirs).is_err());
        fs::write(dirs.bin_dir().join("ffmpeg.exe"), b"corrupt").unwrap();
        fs::write(dirs.bin_dir().join("ffprobe.exe"), b"corrupt").unwrap();
        assert!(bootstrap::resolve_managed_ffmpeg(&dirs).is_err());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn bounded_reader_rejects_excess_after_draining() {
        assert_eq!(read_bounded(&b"abcd"[..], 4).unwrap(), b"abcd");
        assert!(read_bounded(&b"abcde"[..], 4).is_err());
    }

    #[test]
    fn edit_patch_builds_bounded_volume_and_fade_filters() {
        let effects = AudioEditPatch {
            volume_percent: Some(50),
            fade_in_ms: Some(1_500),
            fade_out_ms: Some(2_000),
        }
        .apply(AudioEffects::default(), 10_000)
        .unwrap();
        let args = canonical_transcode_args(
            Path::new("source.flac"),
            Path::new("output.ogg"),
            effects,
            10_000,
        );
        let filter_index = args.iter().position(|arg| arg == "-af").unwrap();
        assert_eq!(
            args[filter_index + 1],
            "volume=50/100,afade=t=in:st=0:d=1.500,afade=t=out:st=8.000:d=2.000"
        );
        assert!(AudioEditPatch {
            volume_percent: None,
            fade_in_ms: Some(6_000),
            fade_out_ms: Some(5_000),
        }
        .apply(AudioEffects::default(), 10_000)
        .is_err());
        assert!(AudioEditPatch {
            volume_percent: Some(MAX_VOLUME_PERCENT + 1),
            ..AudioEditPatch::default()
        }
        .apply(AudioEffects::default(), 10_000)
        .is_err());
    }

    #[test]
    fn project_audio_store_keeps_immutable_source_and_path_edit_history() {
        let root = integration_root("project-source");
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let service = AudioService::new(dirs.clone());
        let source_path = root.join("source.bin");
        let source_bytes = b"original-audio-master";
        fs::write(&source_path, source_bytes).unwrap();
        let project_id = "a".repeat(64);
        let old_path = format!("staredit\\wav\\ea_{}.ogg", "b".repeat(16));
        let binding = AudioBinding {
            descriptor: AttachmentDescriptor {
                id: uuid::Uuid::new_v4().to_string(),
                name: "theme.flac".to_string(),
                mime: "audio/flac".to_string(),
                kind: crate::attachment::AttachmentKind::Audio,
                size: source_bytes.len() as u64,
            },
            source_path,
            source_sha256: sha256_bytes(source_bytes),
            probe: AudioProbe {
                codec: "flac".to_string(),
                duration_ms: 10_000,
                channels: 2,
                sample_rate: 48_000,
                format: "flac".to_string(),
            },
            audio_ref: "audio-1".to_string(),
            request_temp: service.request_temp().unwrap(),
            normalized: Arc::new(Mutex::new(None)),
        };
        let original = service
            .remember_import(&project_id, &old_path, &binding)
            .unwrap();
        let blob = service
            .source_blob_path(&project_id, &original.source_sha256)
            .unwrap();
        assert_eq!(fs::read(blob).unwrap(), source_bytes);

        let new_path = format!("staredit\\wav\\ea_{}.ogg", "c".repeat(16));
        let edited = EditedAudio {
            source: original.clone(),
            previous_effects: AudioEffects::default(),
            effects: AudioEffects {
                volume_percent: 50,
                fade_in_ms: 1_000,
                fade_out_ms: 2_000,
            },
            normalized: NormalizedAudio {
                path: root.join("unused.ogg"),
                sha256: "d".repeat(64),
                bytes: 1,
                duration_ms: 10_000,
                source_codec: "flac".to_string(),
                profile_version: "test".to_string(),
            },
        };
        service
            .remember_edit(&project_id, &new_path, &edited)
            .unwrap();
        service
            .remember_edit(&project_id, &new_path, &edited)
            .unwrap();
        assert_eq!(
            service
                .source_record(&project_id, &old_path)
                .unwrap()
                .unwrap()
                .effects,
            AudioEffects::default()
        );
        assert_eq!(
            service
                .source_record(&project_id, &new_path)
                .unwrap()
                .unwrap()
                .effects,
            edited.effects
        );
        fs::remove_dir_all(root).ok();
    }

    fn installed_dirs() -> DataDirs {
        DataDirs::from_bases(
            Path::new(&std::env::var("APPDATA").unwrap()),
            Path::new(&std::env::var("LOCALAPPDATA").unwrap()),
        )
    }

    fn integration_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("eud-agent-audio-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn generate_audio(tools: &bootstrap::ManagedFfmpegPaths, path: &Path, codec: &str) {
        let mut args = vec![
            "-nostdin".to_string(),
            "-v".to_string(),
            "error".to_string(),
            "-f".to_string(),
            "lavfi".to_string(),
            "-i".to_string(),
            "sine=frequency=1000:duration=0.4".to_string(),
            "-c:a".to_string(),
            codec.to_string(),
        ];
        if path.extension().and_then(|ext| ext.to_str()) == Some("m4a") {
            args.extend(["-f".to_string(), "mp4".to_string()]);
        }
        args.extend(["-y".to_string(), path.to_string_lossy().into_owned()]);
        assert!(
            run_managed_process(
                &tools.ffmpeg,
                &args,
                Duration::from_secs(30),
                1,
                MAX_PROCESS_STDERR,
                None,
            )
            .unwrap()
            .success
        );
    }

    #[test]
    fn bundled_manifest_pins_both_tools_and_vorbis() {
        let manifest = bootstrap::managed_ffmpeg_manifest().unwrap();
        assert_eq!(manifest.schema, "eud-managed-ffmpeg/1");
        assert_eq!(manifest.members.len(), 2);
        assert!(manifest
            .configuration
            .iter()
            .any(|item| item == "--enable-libvorbis"));
    }

    #[test]
    #[ignore = "requires checksum-pinned managed FFmpeg/FFprobe in LocalAppData"]
    fn pinned_ffmpeg_transcodes_every_supported_input_to_canonical_profile() {
        let dirs = installed_dirs();
        let tools = bootstrap::resolve_managed_ffmpeg(&dirs).unwrap();
        let service = AudioService::new(dirs);
        let root = integration_root("formats");
        let formats = [
            ("wav", "pcm_s16le"),
            ("mp3", "libmp3lame"),
            ("flac", "flac"),
            ("m4a", "aac"),
            ("aac", "aac"),
            ("wma", "wmav2"),
            ("aiff", "pcm_s16be"),
            ("opus", "libopus"),
            ("ogg", "libvorbis"),
        ];
        for (index, (extension, codec)) in formats.into_iter().enumerate() {
            let source = root.join(format!("source-{index}.{extension}"));
            generate_audio(&tools, &source, codec);
            let bytes = fs::read(&source).unwrap();
            let binding = service
                .bind(
                    ResolvedAudioAttachment {
                        descriptor: AttachmentDescriptor {
                            id: uuid::Uuid::new_v4().to_string(),
                            name: format!("source-{index}.{extension}"),
                            mime: "application/octet-stream".to_string(),
                            kind: crate::attachment::AttachmentKind::Audio,
                            size: bytes.len() as u64,
                        },
                        path: source,
                        sha256: sha256_bytes(&bytes),
                    },
                    format!("audio-{}", index + 1),
                    service.request_temp().unwrap(),
                    None,
                )
                .unwrap();
            let normalized = service.normalize(&binding, None).unwrap();
            let probe = probe_file(&tools.ffprobe, &normalized.path, None).unwrap();
            assert_eq!(probe.codec, "vorbis");
            assert_eq!(probe.sample_rate, 44_100);
            assert_eq!(probe.channels, 2);
            assert_eq!(sha256_file(&normalized.path).unwrap(), normalized.sha256);
            assert!(fs::read(&normalized.path).unwrap().starts_with(b"OggS"));
        }
        fs::remove_dir_all(root).ok();
    }

    #[test]
    #[ignore = "requires checksum-pinned managed FFmpeg/FFprobe in LocalAppData"]
    fn pinned_process_rejects_corruption_timeout_cancel_and_output_overflow() {
        let dirs = installed_dirs();
        let tools = bootstrap::resolve_managed_ffmpeg(&dirs).unwrap();
        let service = AudioService::new(dirs);
        let root = integration_root("failure");
        let corrupt = root.join("corrupt.wav");
        fs::write(&corrupt, b"RIFF\0\0\0\0WAVEbroken").unwrap();
        let bytes = fs::read(&corrupt).unwrap();
        assert!(service
            .bind(
                ResolvedAudioAttachment {
                    descriptor: AttachmentDescriptor {
                        id: uuid::Uuid::new_v4().to_string(),
                        name: "corrupt.wav".to_string(),
                        mime: "audio/wav".to_string(),
                        kind: crate::attachment::AttachmentKind::Audio,
                        size: bytes.len() as u64,
                    },
                    path: corrupt,
                    sha256: sha256_bytes(&bytes),
                },
                "audio-1".to_string(),
                service.request_temp().unwrap(),
                None,
            )
            .is_err());

        let video_only = root.join("video-only.m4a");
        let video_args = vec![
            "-nostdin".to_string(),
            "-v".to_string(),
            "error".to_string(),
            "-f".to_string(),
            "lavfi".to_string(),
            "-i".to_string(),
            "color=size=16x16:duration=0.2".to_string(),
            "-an".to_string(),
            "-c:v".to_string(),
            "mpeg4".to_string(),
            "-f".to_string(),
            "mp4".to_string(),
            "-y".to_string(),
            video_only.to_string_lossy().into_owned(),
        ];
        assert!(
            run_managed_process(
                &tools.ffmpeg,
                &video_args,
                Duration::from_secs(30),
                1,
                MAX_PROCESS_STDERR,
                None,
            )
            .unwrap()
            .success
        );
        let video_bytes = fs::read(&video_only).unwrap();
        let no_audio = service
            .bind(
                ResolvedAudioAttachment {
                    descriptor: AttachmentDescriptor {
                        id: uuid::Uuid::new_v4().to_string(),
                        name: "video-only.m4a".to_string(),
                        mime: "audio/mp4".to_string(),
                        kind: crate::attachment::AttachmentKind::Audio,
                        size: video_bytes.len() as u64,
                    },
                    path: video_only,
                    sha256: sha256_bytes(&video_bytes),
                },
                "audio-2".to_string(),
                service.request_temp().unwrap(),
                None,
            )
            .unwrap_err();
        assert!(no_audio.contains("오디오 스트림"));

        let realtime = vec![
            "-nostdin".to_string(),
            "-v".to_string(),
            "error".to_string(),
            "-re".to_string(),
            "-f".to_string(),
            "lavfi".to_string(),
            "-i".to_string(),
            "anullsrc".to_string(),
            "-t".to_string(),
            "30".to_string(),
            "-f".to_string(),
            "null".to_string(),
            "-".to_string(),
        ];
        assert!(run_managed_process(
            &tools.ffmpeg,
            &realtime,
            Duration::from_millis(1),
            1,
            MAX_PROCESS_STDERR,
            None,
        )
        .unwrap_err()
        .contains("초과"));

        let (cancel, receiver) = tokio::sync::watch::channel(0_u64);
        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            cancel.send_replace(1);
        });
        assert!(run_managed_process(
            &tools.ffmpeg,
            &realtime,
            Duration::from_secs(5),
            1,
            MAX_PROCESS_STDERR,
            Some(&receiver),
        )
        .unwrap_err()
        .contains("취소"));
        cancel_thread.join().unwrap();

        assert!(run_managed_process(
            &tools.ffmpeg,
            &["-version".to_string()],
            Duration::from_secs(5),
            1,
            MAX_PROCESS_STDERR,
            None,
        )
        .unwrap_err()
        .contains("크기 제한"));
        fs::remove_dir_all(root).ok();
    }
}
