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
        let tools = bootstrap::resolve_managed_ffmpeg(&self.dirs)
            .map_err(|_| "관리되는 오디오 변환기 자산이 없거나 손상되었습니다.".to_string())?;
        let output = binding
            .request_temp
            .path()
            .join(format!("{}.ogg", binding.audio_ref));
        let _ = fs::remove_file(&output);
        let args = vec![
            "-nostdin".to_string(),
            "-v".to_string(),
            "error".to_string(),
            "-protocol_whitelist".to_string(),
            "file".to_string(),
            "-i".to_string(),
            binding.source_path.to_string_lossy().into_owned(),
            "-map".to_string(),
            "0:a:0".to_string(),
            "-vn".to_string(),
            "-sn".to_string(),
            "-dn".to_string(),
            "-map_metadata".to_string(),
            "-1".to_string(),
            "-map_chapters".to_string(),
            "-1".to_string(),
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
        ];
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
                let _ = fs::remove_file(&output);
                return Err(error);
            }
        };
        if !process.success {
            let _ = fs::remove_file(&output);
            return Err("오디오 파일이 손상되었거나 디코딩할 수 없습니다.".to_string());
        }

        let normalized = (|| {
            let before = stable_file_metadata(&output, MAX_NORMALIZED_AUDIO_BYTES)?;
            let bytes =
                fs::read(&output).map_err(|_| "정규화된 OGG를 읽을 수 없습니다.".to_string())?;
            let after = stable_file_metadata(&output, MAX_NORMALIZED_AUDIO_BYTES)?;
            if before != after {
                return Err("정규화된 OGG가 검증 중 변경되었습니다.".to_string());
            }
            if !bytes.starts_with(b"OggS") {
                return Err("정규화된 OGG magic 검증에 실패했습니다.".to_string());
            }
            let output_probe = probe_file(&tools.ffprobe, &output, cancellation)?;
            if output_probe.codec != "vorbis"
                || output_probe.sample_rate != 44_100
                || output_probe.channels != 2
            {
                return Err("정규화된 OGG codec profile 검증에 실패했습니다.".to_string());
            }
            let tolerance = 250_u64.max(binding.probe.duration_ms / 200);
            if output_probe.duration_ms.abs_diff(binding.probe.duration_ms) > tolerance {
                return Err("정규화된 OGG 길이 검증에 실패했습니다.".to_string());
            }
            Ok(NormalizedAudio {
                path: output.clone(),
                sha256: sha256_bytes(&bytes),
                bytes: bytes.len() as u64,
                duration_ms: output_probe.duration_ms,
                source_codec: binding.probe.codec.clone(),
                profile_version: tools.version,
            })
        })();
        let normalized = match normalized {
            Ok(normalized) => normalized,
            Err(error) => {
                let _ = fs::remove_file(&output);
                return Err(error);
            }
        };
        *binding
            .normalized
            .lock()
            .map_err(|_| "오디오 변환 cache lock이 손상되었습니다.".to_string())? =
            Some(normalized.clone());
        Ok(normalized)
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
