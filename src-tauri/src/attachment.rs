//! Session-owned chat attachment storage and validation.
//!
//! Files cross the WebView boundary as a raw Tauri IPC body, are copied into the app's
//! LocalAppData, and are thereafter addressed only by an opaque UUID. Images become
//! Codex `localImage` inputs; UTF-8 text/code is folded into the user turn text.

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub const MAX_ATTACHMENTS_PER_TURN: usize = 5;
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 512 * 1024;
pub const MAX_AUDIO_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_AUDIO_BYTES_PER_TURN: usize = 128 * 1024 * 1024;
const DRAFT_MAX_AGE_SECS: u64 = 24 * 60 * 60;
const META_FILE: &str = "meta.json";
const CONTENT_STEM: &str = "content";
pub const FILE_NAME_HEX_HEADER: &str = "x-eud-file-name-hex";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    Image,
    Text,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentDescriptor {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub kind: AttachmentKind,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct ResolvedImageAttachment {
    pub descriptor: AttachmentDescriptor,
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedAudioAttachment {
    pub descriptor: AttachmentDescriptor,
    pub path: PathBuf,
    pub sha256: String,
}
#[derive(Debug, Clone)]
pub struct AttachmentContext {
    pub image_paths: Vec<PathBuf>,
    pub images: Vec<ResolvedImageAttachment>,
    pub text_files: Vec<(String, String)>,
    pub audio_files: Vec<ResolvedAudioAttachment>,
}

impl AttachmentContext {
    pub fn is_empty(&self) -> bool {
        self.image_paths.is_empty() && self.text_files.is_empty() && self.audio_files.is_empty()
    }

    pub fn append_text_files(&self, user_text: &str) -> String {
        if self.text_files.is_empty() {
            return user_text.to_string();
        }

        let mut rendered = String::with_capacity(
            user_text.len()
                + self
                    .text_files
                    .iter()
                    .map(|(name, content)| name.len() + content.len() + 96)
                    .sum::<usize>(),
        );
        rendered.push_str(user_text);
        for (name, content) in &self.text_files {
            rendered.push_str("\n\n[attached file: ");
            rendered.push_str(name);
            rendered.push_str("]\n----- BEGIN ATTACHED FILE -----\n");
            rendered.push_str(content);
            rendered.push_str("\n----- END ATTACHED FILE -----");
        }
        rendered
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentMeta {
    #[serde(flatten)]
    descriptor: AttachmentDescriptor,
    stored_name: String,
    created_at: u64,
    session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AttachmentStore {
    root: PathBuf,
}

impl AttachmentStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn stage(
        &self,
        requested_name: &str,
        requested_mime: &str,
        bytes: &[u8],
    ) -> Result<AttachmentDescriptor, String> {
        let name = clean_display_name(requested_name)?;
        if bytes.is_empty() {
            return Err(format!("빈 첨부 파일은 사용할 수 없습니다: {name}"));
        }
        let (kind, mime, extension) = match detect_image(bytes) {
            Some((mime, extension)) => {
                validate_attachment_size(AttachmentKind::Image, bytes.len(), &name)?;
                (AttachmentKind::Image, mime.to_string(), extension)
            }
            None => match detect_audio(bytes) {
                Some((mime, extension)) => {
                    validate_attachment_size(AttachmentKind::Audio, bytes.len(), &name)?;
                    (AttachmentKind::Audio, mime.to_string(), extension)
                }
                None => {
                    validate_attachment_size(AttachmentKind::Text, bytes.len(), &name)?;
                    let text = std::str::from_utf8(bytes)
                        .map_err(|_| format!("지원하지 않는 바이너리 파일입니다: {name}"))?;
                    if text.contains('\0') {
                        return Err(format!("지원하지 않는 바이너리 파일입니다: {name}"));
                    }
                    (
                        AttachmentKind::Text,
                        normalize_text_mime(requested_mime),
                        "txt",
                    )
                }
            },
        };

        let id = uuid::Uuid::new_v4().to_string();
        let object_dir = self.object_dir(&id);
        fs::create_dir_all(&object_dir)
            .map_err(|error| format!("첨부 파일 저장 폴더를 만들 수 없습니다: {error}"))?;
        let stored_name = format!("{CONTENT_STEM}.{extension}");
        if let Err(error) = fs::write(object_dir.join(&stored_name), bytes) {
            let _ = fs::remove_dir_all(&object_dir);
            return Err(format!("첨부 파일을 저장할 수 없습니다: {error}"));
        }

        let meta = AttachmentMeta {
            descriptor: AttachmentDescriptor {
                id,
                name,
                mime,
                kind,
                size: bytes.len() as u64,
            },
            stored_name,
            created_at: now_unix_seconds(),
            session_id: None,
        };
        if let Err(error) = self.write_meta(&meta) {
            let _ = fs::remove_dir_all(&object_dir);
            return Err(error);
        }
        Ok(meta.descriptor)
    }

    pub fn bind_and_resolve(
        &self,
        ids: &[String],
        session_id: &str,
    ) -> Result<AttachmentContext, String> {
        if ids.len() > MAX_ATTACHMENTS_PER_TURN {
            return Err(format!(
                "한 번에 첨부할 수 있는 파일은 최대 {MAX_ATTACHMENTS_PER_TURN}개입니다."
            ));
        }
        let mut unique = HashSet::with_capacity(ids.len());
        let mut loaded = Vec::with_capacity(ids.len());
        let mut total_text_bytes = 0usize;
        let mut total_audio_bytes = 0usize;

        for id in ids {
            validate_id(id)?;
            if !unique.insert(id.as_str()) {
                return Err("같은 첨부 파일을 두 번 보낼 수 없습니다.".to_string());
            }
            let meta = self.read_meta(id)?;
            if meta
                .session_id
                .as_deref()
                .is_some_and(|bound| bound != session_id)
            {
                return Err(format!(
                    "다른 대화에 속한 첨부 파일입니다: {}",
                    meta.descriptor.name
                ));
            }
            let content_path = self.object_dir(id).join(&meta.stored_name);
            if !content_path.is_file() {
                return Err(format!(
                    "첨부 파일을 찾을 수 없습니다: {}",
                    meta.descriptor.name
                ));
            }
            if meta.descriptor.kind == AttachmentKind::Text {
                total_text_bytes = total_text_bytes
                    .checked_add(meta.descriptor.size as usize)
                    .ok_or_else(|| "첨부 텍스트 크기가 너무 큽니다.".to_string())?;
                if total_text_bytes > MAX_TEXT_BYTES {
                    return Err(
                        "한 번에 첨부하는 텍스트/코드는 합계 512KB 이하여야 합니다.".to_string()
                    );
                }
            }
            if meta.descriptor.kind == AttachmentKind::Audio {
                total_audio_bytes = checked_audio_total(total_audio_bytes, meta.descriptor.size)?;
            }
            loaded.push((meta, content_path));
        }

        let mut image_paths = Vec::new();
        let mut images = Vec::new();
        let mut text_files = Vec::new();
        let mut audio_files = Vec::new();
        for (mut meta, content_path) in loaded {
            meta.session_id = Some(session_id.to_string());
            self.write_meta(&meta)?;
            match meta.descriptor.kind {
                AttachmentKind::Image => {
                    let sha256 = file_sha256(&content_path)?;
                    image_paths.push(content_path.clone());
                    images.push(ResolvedImageAttachment {
                        descriptor: meta.descriptor,
                        path: content_path,
                        sha256,
                    });
                }
                AttachmentKind::Text => {
                    let content = fs::read_to_string(&content_path).map_err(|error| {
                        format!(
                            "첨부 텍스트를 읽을 수 없습니다: {} ({error})",
                            meta.descriptor.name
                        )
                    })?;
                    text_files.push((meta.descriptor.name, content));
                }
                AttachmentKind::Audio => {
                    let sha256 = file_sha256(&content_path)?;
                    audio_files.push(ResolvedAudioAttachment {
                        descriptor: meta.descriptor,
                        path: content_path,
                        sha256,
                    });
                }
            }
        }

        Ok(AttachmentContext {
            image_paths,
            images,
            text_files,
            audio_files,
        })
    }

    pub fn bind_and_resolve_image(
        &self,
        id: &str,
        session_id: &str,
    ) -> Result<ResolvedImageAttachment, String> {
        let mut context = self.bind_and_resolve(&[id.to_string()], session_id)?;
        context
            .images
            .pop()
            .ok_or_else(|| "선택한 첨부 파일은 지원 이미지가 아닙니다.".to_string())
    }

    pub fn discard_draft(&self, id: &str) -> Result<(), String> {
        validate_id(id)?;
        let meta = self.read_meta(id)?;
        if meta.session_id.is_some() {
            return Err("이미 전송한 첨부 파일은 삭제할 수 없습니다.".to_string());
        }
        remove_object_dir(&self.object_dir(id))
    }

    pub fn delete_session(&self, session_id: &str) -> Result<(), String> {
        let objects = self.objects_dir();
        let entries = match fs::read_dir(&objects) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("첨부 파일 목록을 읽을 수 없습니다: {error}")),
        };
        for entry in entries.flatten() {
            let object_dir = entry.path();
            let Ok(meta) = read_meta_path(&object_dir.join(META_FILE)) else {
                continue;
            };
            if meta.session_id.as_deref() == Some(session_id) {
                remove_object_dir(&object_dir)?;
            }
        }
        Ok(())
    }

    pub fn cleanup_stale_drafts(&self) {
        let objects = self.objects_dir();
        let Ok(entries) = fs::read_dir(&objects) else {
            return;
        };
        let now = now_unix_seconds();
        for entry in entries.flatten() {
            let object_dir = entry.path();
            let Ok(meta) = read_meta_path(&object_dir.join(META_FILE)) else {
                continue;
            };
            if meta.session_id.is_none()
                && now.saturating_sub(meta.created_at) >= DRAFT_MAX_AGE_SECS
            {
                let _ = fs::remove_dir_all(object_dir);
            }
        }
    }

    fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }

    fn object_dir(&self, id: &str) -> PathBuf {
        self.objects_dir().join(id)
    }

    fn read_meta(&self, id: &str) -> Result<AttachmentMeta, String> {
        read_meta_path(&self.object_dir(id).join(META_FILE))
    }

    fn write_meta(&self, meta: &AttachmentMeta) -> Result<(), String> {
        let object_dir = self.object_dir(&meta.descriptor.id);
        fs::create_dir_all(&object_dir)
            .map_err(|error| format!("첨부 메타데이터 폴더를 만들 수 없습니다: {error}"))?;
        let bytes = serde_json::to_vec(meta)
            .map_err(|error| format!("첨부 메타데이터를 직렬화할 수 없습니다: {error}"))?;
        let tmp = object_dir.join("meta.tmp");
        fs::write(&tmp, bytes)
            .map_err(|error| format!("첨부 메타데이터를 저장할 수 없습니다: {error}"))?;
        fs::rename(&tmp, object_dir.join(META_FILE)).map_err(|error| {
            let _ = fs::remove_file(&tmp);
            format!("첨부 메타데이터를 확정할 수 없습니다: {error}")
        })
    }
}

#[derive(Debug, Clone)]
pub struct AttachmentManaged {
    store: AttachmentStore,
}

impl AttachmentManaged {
    pub fn new(store: AttachmentStore) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &AttachmentStore {
        &self.store
    }
}

#[tauri::command(rename = "attachment_stage")]
pub fn stage_attachment(
    state: tauri::State<'_, AttachmentManaged>,
    request: tauri::ipc::Request<'_>,
) -> Result<AttachmentDescriptor, String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("첨부 파일 본문 형식이 올바르지 않습니다.".to_string());
    };
    let encoded_name = request
        .headers()
        .get(FILE_NAME_HEX_HEADER)
        .ok_or_else(|| "첨부 파일 이름이 없습니다.".to_string())?
        .to_str()
        .map_err(|_| "첨부 파일 이름 헤더가 올바르지 않습니다.".to_string())?;
    let name = decode_hex_utf8(encoded_name)?;
    let mime = request
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream");
    state.store.stage(&name, mime, bytes)
}

#[tauri::command(rename = "attachment_discard")]
pub fn discard_attachment(
    state: tauri::State<'_, AttachmentManaged>,
    id: String,
) -> Result<(), String> {
    state.store.discard_draft(&id)
}

fn clean_display_name(requested: &str) -> Result<String, String> {
    let name = Path::new(requested)
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or(requested)
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(255)
        .collect::<String>();
    if name.is_empty() {
        return Err("첨부 파일 이름이 비어 있습니다.".to_string());
    }
    Ok(name)
}

fn validate_attachment_size(kind: AttachmentKind, size: usize, name: &str) -> Result<(), String> {
    match kind {
        AttachmentKind::Image if size > MAX_IMAGE_BYTES => {
            Err(format!("이미지 파일은 10MB 이하여야 합니다: {name}"))
        }
        AttachmentKind::Text if size > MAX_TEXT_BYTES => {
            Err(format!("텍스트/코드 파일은 512KB 이하여야 합니다: {name}"))
        }
        AttachmentKind::Audio if size > MAX_AUDIO_BYTES => {
            Err(format!("오디오 파일은 64MB 이하여야 합니다: {name}"))
        }
        _ => Ok(()),
    }
}

fn checked_audio_total(total: usize, size: u64) -> Result<usize, String> {
    let size = usize::try_from(size).map_err(|_| "첨부 오디오 크기가 너무 큽니다.".to_string())?;
    let total = total
        .checked_add(size)
        .ok_or_else(|| "첨부 오디오 크기 합계가 너무 큽니다.".to_string())?;
    if total > MAX_AUDIO_BYTES_PER_TURN {
        return Err("한 번에 첨부하는 오디오는 합계 128MB 이하여야 합니다.".to_string());
    }
    Ok(total)
}

fn normalize_text_mime(requested: &str) -> String {
    let mime = requested.trim();
    if mime.is_empty() || mime.len() > 127 || !mime.is_ascii() || mime.chars().any(char::is_control)
    {
        "text/plain".to_string()
    } else {
        mime.to_string()
    }
}

fn detect_image(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "png"))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("image/jpeg", "jpg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("image/gif", "gif"))
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

fn detect_audio(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        return Some(("audio/wav", "wav"));
    }
    if bytes.starts_with(b"OggS") {
        return Some(("audio/ogg", "ogg"));
    }
    if bytes.starts_with(b"fLaC") {
        return Some(("audio/flac", "flac"));
    }
    if bytes.starts_with(b"ID3") || has_valid_mp3_frame_sync(bytes) {
        return Some(("audio/mpeg", "mp3"));
    }
    if has_valid_adts_header(bytes) {
        return Some(("audio/aac", "aac"));
    }
    if is_audio_iso_bmff(bytes) {
        return Some(("audio/mp4", "m4a"));
    }
    if bytes.len() >= 12 && bytes.starts_with(b"FORM") && matches!(&bytes[8..12], b"AIFF" | b"AIFC")
    {
        return Some(("audio/aiff", "aiff"));
    }
    const ASF_HEADER: [u8; 16] = [
        0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62, 0xce,
        0x6c,
    ];
    bytes
        .starts_with(&ASF_HEADER)
        .then_some(("audio/x-ms-wma", "wma"))
}

fn has_valid_mp3_frame_sync(bytes: &[u8]) -> bool {
    bytes.get(..4).is_some_and(|header| {
        header[0] == 0xff
            && header[1] & 0xe0 == 0xe0
            && header[1] & 0x18 != 0x08
            && header[1] & 0x06 != 0
            && header[2] & 0xf0 != 0
            && header[2] & 0xf0 != 0xf0
            && header[2] & 0x0c != 0x0c
    })
}

fn has_valid_adts_header(bytes: &[u8]) -> bool {
    bytes.get(..7).is_some_and(|header| {
        let sample_rate_index = (header[2] >> 2) & 0x0f;
        let frame_length = (usize::from(header[3] & 0x03) << 11)
            | (usize::from(header[4]) << 3)
            | usize::from(header[5] >> 5);
        header[0] == 0xff
            && header[1] & 0xf6 == 0xf0
            && sample_rate_index != 0x0f
            && frame_length >= 7
    })
}

fn is_audio_iso_bmff(bytes: &[u8]) -> bool {
    if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let box_size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if box_size < 16 {
        return false;
    }
    let end = box_size.min(bytes.len()).min(128);
    bytes[8..end].chunks_exact(4).any(|brand| {
        matches!(
            brand,
            b"M4A " | b"M4B " | b"M4P " | b"mp41" | b"mp42" | b"isom" | b"iso2" | b"iso5"
        )
    })
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("첨부 파일을 읽을 수 없습니다: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("첨부 파일을 읽을 수 없습니다: {error}"))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn validate_id(id: &str) -> Result<(), String> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| "첨부 파일 식별자가 올바르지 않습니다.".to_string())
}

fn read_meta_path(path: &Path) -> Result<AttachmentMeta, String> {
    let bytes = fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "첨부 파일을 찾을 수 없습니다.".to_string()
        } else {
            format!("첨부 메타데이터를 읽을 수 없습니다: {error}")
        }
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("첨부 메타데이터가 손상되었습니다: {error}"))
}

fn remove_object_dir(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("첨부 파일을 삭제할 수 없습니다: {error}")),
    }
}

fn decode_hex_utf8(encoded: &str) -> Result<String, String> {
    if encoded.len() % 2 != 0 {
        return Err("첨부 파일 이름 인코딩이 올바르지 않습니다.".to_string());
    }
    let mut bytes = Vec::with_capacity(encoded.len() / 2);
    for pair in encoded.as_bytes().chunks_exact(2) {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        bytes.push((high << 4) | low);
    }
    String::from_utf8(bytes).map_err(|_| "첨부 파일 이름이 UTF-8이 아닙니다.".to_string())
}

fn hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("첨부 파일 이름 인코딩이 올바르지 않습니다.".to_string()),
    }
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> (PathBuf, AttachmentStore) {
        let root = std::env::temp_dir().join(format!(
            "eud-agent-attachment-test-{tag}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (root.clone(), AttachmentStore::new(root))
    }

    #[test]
    fn stages_binds_and_deletes_a_session_image() {
        let (root, store) = temp_store("image");
        let descriptor = store
            .stage(
                "스크린샷.png",
                "application/octet-stream",
                b"\x89PNG\r\n\x1a\nbody",
            )
            .unwrap();
        assert_eq!(descriptor.kind, AttachmentKind::Image);
        assert_eq!(descriptor.mime, "image/png");

        let context = store
            .bind_and_resolve(std::slice::from_ref(&descriptor.id), "session-1")
            .unwrap();
        assert_eq!(context.image_paths.len(), 1);
        assert!(context.image_paths[0].is_file());
        assert!(context.text_files.is_empty());
        assert!(store.discard_draft(&descriptor.id).is_err());

        store.delete_session("session-1").unwrap();
        assert!(!context.image_paths[0].exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn text_files_are_utf8_and_rendered_into_the_turn() {
        let (root, store) = temp_store("text");
        let descriptor = store
            .stage("main.eps", "text/plain", "유닛 설정".as_bytes())
            .unwrap();
        assert_eq!(descriptor.kind, AttachmentKind::Text);

        let context = store
            .bind_and_resolve(std::slice::from_ref(&descriptor.id), "session-1")
            .unwrap();
        let rendered = context.append_text_files("검토해 줘");
        assert!(rendered.contains("[attached file: main.eps]"));
        assert!(rendered.contains("유닛 설정"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rejects_binary_oversize_duplicate_and_cross_session_inputs() {
        let (root, store) = temp_store("reject");
        assert!(store.stage("bad.bin", "", &[0, 159, 146, 150]).is_err());
        assert!(
            validate_attachment_size(AttachmentKind::Text, MAX_TEXT_BYTES + 1, "large.txt",)
                .is_err()
        );
        assert!(
            validate_attachment_size(AttachmentKind::Image, MAX_IMAGE_BYTES + 1, "large.png",)
                .is_err()
        );

        let descriptor = store.stage("a.txt", "text/plain", b"a").unwrap();
        assert!(store
            .bind_and_resolve(&[descriptor.id.clone(), descriptor.id.clone()], "session-1")
            .is_err());
        store
            .bind_and_resolve(std::slice::from_ref(&descriptor.id), "session-1")
            .unwrap();
        assert!(store
            .bind_and_resolve(std::slice::from_ref(&descriptor.id), "session-2")
            .is_err());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn audio_magic_order_caps_and_binding_are_bounded() {
        let cases: &[(&[u8], &str)] = &[
            (b"RIFF\0\0\0\0WAVE", "wav"),
            (b"OggS\0\0\0\0", "ogg"),
            (b"fLaC\0\0\0\0", "flac"),
            (&[0xff, 0xfb, 0x90, 0x64], "mp3"),
            (&[0xff, 0xf1, 0x50, 0x80, 0x01, 0x7f, 0xfc], "aac"),
            (b"\0\0\0\x18ftypM4A \0\0\0\0M4A ", "m4a"),
            (b"FORM\0\0\0\0AIFF", "aiff"),
            (
                &[
                    0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0, 0xaa, 0, 0x62,
                    0xce, 0x6c,
                ],
                "wma",
            ),
        ];
        for (bytes, extension) in cases {
            assert_eq!(detect_audio(bytes).unwrap().1, *extension);
        }
        assert!(detect_audio(b"RIFF\0\0\0\0WEBP").is_none());
        assert!(validate_attachment_size(AttachmentKind::Audio, MAX_AUDIO_BYTES, "a.ogg").is_ok());
        assert!(
            validate_attachment_size(AttachmentKind::Audio, MAX_AUDIO_BYTES + 1, "a.ogg").is_err()
        );
        assert_eq!(
            checked_audio_total(MAX_AUDIO_BYTES, MAX_AUDIO_BYTES as u64).unwrap(),
            MAX_AUDIO_BYTES_PER_TURN
        );
        assert!(checked_audio_total(MAX_AUDIO_BYTES_PER_TURN, 1).is_err());

        let (root, store) = temp_store("audio");
        let descriptor = store.stage("effect.wav", "text/plain", cases[0].0).unwrap();
        assert_eq!(descriptor.kind, AttachmentKind::Audio);
        assert_eq!(descriptor.mime, "audio/wav");
        let context = store
            .bind_and_resolve(std::slice::from_ref(&descriptor.id), "session-a")
            .unwrap();
        assert_eq!(context.audio_files.len(), 1);
        assert!(context.image_paths.is_empty());
        assert!(context.text_files.is_empty());
        assert!(store
            .bind_and_resolve(std::slice::from_ref(&descriptor.id), "session-b")
            .is_err());
        store.delete_session("session-a").unwrap();
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn empty_attachment_is_rejected_before_text_classification() {
        let (root, store) = temp_store("empty");
        assert!(store.stage("empty.txt", "text/plain", b"").is_err());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn filename_header_hex_round_trips_korean() {
        let encoded = "ec8aa4ed81aceba6b0ec83b72e706e67";
        assert_eq!(decode_hex_utf8(encoded).unwrap(), "스크린샷.png");
        assert!(decode_hex_utf8("0").is_err());
        assert!(decode_hex_utf8("gg").is_err());
    }
}
