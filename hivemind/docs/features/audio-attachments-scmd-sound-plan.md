# Audio Attachments → SCMDraft Sound Import — Authoritative Implementation Plan

Status: planned. This document is the single implementation authority until the feature is accepted and folded into `architecture.md`, `rules.md`, `tech-stack.md`, and `verify.md`.

## 1. 목표

사용자가 메인 EPS 대화에 일반 오디오 파일을 첨부하고 자연어로 배경음악 또는 효과음 사용을 요청하면 `eud-agent`가 다음 작업을 하나의 검토 가능한 프로젝트 변경으로 수행한다.

1. 첨부 파일에서 실제 오디오 스트림을 검사한다.
2. 입력 포맷과 무관하게 StarCraft: Remastered 호환 OGG Vorbis로 정규화한다.
3. 현재 저장된 원본 SCX의 MPQ, `STR`/`STRx`, `WAV `에 사운드를 등록한다.
4. 등록한 맵 내부 경로를 사용하는 epScript 재생 로직을 기존 책임 파일에 추가한다.
5. 전체 EUD Editor 빌드를 실행한다.
6. 코드와 맵 변경을 한 changeset으로 검토, 승인, 거부, 복구한다.

최종 사용자 계약은 다음과 같다.

```text
첨부: battle_theme.flac
요청: 게임 시작 후 모든 플레이어에게 반복 재생해 줘
```

```text
첨부: boss-death.wav
요청: 보스가 죽을 때 이 효과음을 현재 플레이어에게 재생해 줘
```

사용자는 SCMDraft Sound Editor에서 수동으로 `Batch Import`하지 않는다. 승인된 결과 SCX를 SCMDraft에서 다시 열면 정규화된 OGG가 `Map Sound Entries`에 존재해야 한다.

## 2. 확정 결정

다음 결정은 구현 중 재해석하지 않는다.

1. **SCMDraft GUI 자동화는 사용하지 않는다.** 저장된 SCX를 직접 수정한다.
2. **입력 포맷과 맵 저장 포맷을 분리한다.** 입력은 FFmpeg가 안전하게 인식하는 일반 오디오이고 맵 내부 출력은 OGG Vorbis 하나다.
3. **StarCraft: Remastered만 지원한다.** OGG를 재생하지 못하는 1.16.1용 WAV fallback은 포함하지 않는다.
4. **배경음악과 효과음은 같은 사운드 등록 경로를 사용한다.** 차이는 생성되는 epScript 조건, 플레이어 범위, 반복 정책뿐이다.
5. **첫 구현의 재생 권한은 `PlayWAV`/`PlayWAVAll`이다.** 즉시 정지, 일시정지, 이어 재생, 페이드, 독립 다중 BGM은 제외한다.
6. **원본 첨부 이름은 MPQ 경로에 사용하지 않는다.** 출력 SHA-256에서 만든 ASCII 이름만 사용한다.
7. **모델은 첨부 ID, 로컬 경로, FFmpeg 경로, 임의 MPQ 목적지를 제공하지 않는다.** 요청 전용 `audio-N`만 도구 입력으로 사용한다.
8. **정규화는 프로젝트 변경 잠금 밖에서 수행한다.** 공유 프로젝트 임계구역에는 검증된 OGG를 SCX에 반영하고 검증하는 시간만 포함한다.
9. **승인된 SCX가 오디오 원본 권위다.** 등록 후 세션 첨부나 외부 원본 파일이 삭제되어도 빌드와 SCMDraft 표시가 유지되어야 한다.
10. **SCMDraft 또는 다른 프로그램이 원본 맵을 열어 두면 Apply하지 않는다.** 저장·닫기 후 재시도한다.
11. **기존 `ProjectWriteCoordinator`, journal, full backup, no-share lock, atomic replace, rollback을 우회하지 않는다.**
12. **별도 범용 asset framework를 만들지 않는다.** 이번 범위는 오디오 첨부, 정규화, SCX 사운드 등록에 한정한다.

## 3. 근거와 현재 기반

### 3.1 EUD Editor 3 일반 사운드 계약

EUD Editor 3은 연결된 맵의 CHK `WAV ` 섹션을 읽어 사운드 이름을 제공한다.

- `../EUD-Editor-3/EUD Editor 3/Class/Data/MapData.vb`
  - `WAV ` 섹션의 string id를 읽는다.
- `../EUD-Editor-3/EUD Editor 3/UserContorl/TriggerEditor/GUIEditorPage/Scripter/ScriptEditer/GUI_ActionValueSelect/GUI_WavSelecter.xaml.vb`
  - 맵 사운드와 StarCraft virtual sound를 선택지로 표시한다.
- `../EUD-Editor-3/EUD Editor 3/Data/TriggerEditor/epsFunctions_safe.txt`
  - `PlayWAV(WAVName)`와 `PlayWAVAll(WAVName)`을 공개한다.

따라서 SCMDraft Sound Editor와 호환되는 등록은 MPQ 파일 추가만으로 끝나지 않는다. MPQ asset, 게임 문자열, `WAV ` 슬롯이 함께 존재해야 한다.

### 3.2 vendored MappingCore 사운드 계약

`native/isom/MappingCoreLib/MapFile.cpp`의 `MapFile::addSound`는 이미 필요한 세 작업을 수행한다.

```cpp
addMpqAsset(srcFilePath, destMpqPath, wavQuality);
const size_t soundStringId = Scenario::addString(
    RawString(destMpqPath),
    Chk::StrScope::Game
);
Scenario::addSound(soundStringId);
```

표준 MPQ 경로는 다음과 같다.

```text
staredit\wav\
```

`native/isom/MappingCoreLib/MapFile.cpp`의 사운드 필터도 WAV와 OGG를 명시한다. `Chk::TotalSounds`는 512다.

현재 `MapFile::addSound`는 `Scenario::addSound`가 512를 반환하는 슬롯 고갈 결과를 검사하지 않는다. 새 C ABI는 호출 전에 빈 슬롯을 검사하고 저장 후 정확한 슬롯을 검증해야 한다. 기존 메서드를 검증 없이 직접 노출하지 않는다.

### 3.3 현재 첨부 기반

`src-tauri/src/attachment.rs`는 다음을 이미 제공한다.

- raw Tauri IPC body
- magic-byte 기반 이미지 검사
- opaque UUID
- session binding
- LocalAppData 저장
- SHA-256 계산
- 세션 삭제 및 stale draft cleanup

현재 종류는 `Image | Text`뿐이고 비이미지 바이너리를 UTF-8 텍스트로 해석하므로 오디오는 거부된다. 이 저장소를 확장하되 이미지와 텍스트의 기존 계약은 변경하지 않는다.

### 3.4 현재 맵 안전 레일

`src-tauri/src/mapsafe.rs`는 다음 순서를 소유한다.

1. EUD Editor compiling guard
2. Windows no-share lock probe
3. 전체 SCX 백업
4. 입력 권위 검사
5. 네이티브 temp output
6. 결과 재검증과 atomic replace
7. 실패·거부 rollback

사운드 등록은 새 저장 경로를 만들지 않고 이 순서를 사용한다.

### 3.5 현재 네이티브 자산 불변식

`native/isom/IsomTerrain/MapAgentCore.cpp::mapEdit`는 저장 전후 MPQ asset inventory가 다르면 실패한다. 기존 terrain/object 연산에는 올바른 불변식이다.

사운드 등록은 이 검사를 약화하지 않는다. 전용 네이티브 진입점에서 다음 exact delta를 요구한다.

```text
before assets + exactly one requested OGG asset == after assets
```

다른 MPQ 항목 추가, 삭제, 이름 변경, 내용 변경은 실패다.

## 4. 범위

### 4.1 포함

- 메인 EPS 대화의 오디오 파일 선택, drag/drop
- `AttachmentKind::Audio`
- 요청 전용 `audio-1`, `audio-2`, ... binding
- FFprobe 기반 스트림·코덱·길이 검사
- FFmpeg 기반 OGG Vorbis 정규화
- WAV, OGG, MP3, FLAC, M4A/AAC, WMA, AIFF, Opus 등 일반 입력
- 정규화된 OGG magic/decode/크기 재검증
- 현재 저장된 `OpenMapName` SCX에 등록
- `staredit\wav\ea_<sha-prefix>.ogg` 내부 이름
- MPQ + game string + `WAV ` 슬롯 등록
- 중복 파일 재사용
- `PlayWAV`/`PlayWAVAll` 기반 1회 재생
- 측정된 길이를 사용하는 반복 재생 코드
- 코드와 맵의 한 write lease, build, changeset, rollback
- SCMDraft 재열기 후 Sound Editor 표시 검증
- 인게임 현재 플레이어/전체 플레이어/반복 재생 검증

### 4.2 제외

- SCMDraft 메뉴, 창, 파일 대화상자 UI 자동화
- 열린 SCMDraft 프로세스의 메모리나 문서 모델 주입
- SCMDraft plugin 개발
- StarCraft 1.16.1 WAV fallback
- MIDI soundfont 합성
- DRM 해제
- 네트워크 URL, playlist, 외부 segment 참조
- 동영상 파일에서 오디오 추출을 제품 기능으로 노출
- 음량 정규화, 노이즈 제거, 무음 자르기
- waveform 편집기와 panel 오디오 플레이어
- 즉시 stop, pause/resume, seek, crossfade, fade-in/out
- 여러 BGM의 독립 동시 제어
- EUD Editor의 `BGMData`/분할형 `BGMPlayer` 자동 등록
- 기존 사운드 삭제·교체·이름 변경
- 맵에 등록된 사운드 추출
- Map Agent의 공간 target/protect 권한에 사운드를 억지로 포함

## 5. 사용자 흐름

### 5.1 첨부

파일 선택기는 이미지·텍스트와 함께 오디오를 허용한다.

```text
audio/*
.wav .ogg .mp3 .flac .m4a .aac .wma .aiff .aif .opus
```

확장자는 선택기 UX용 힌트일 뿐이다. backend는 파일명과 MIME을 신뢰하지 않고 probe 결과로 종류를 결정한다.

오디오 chip은 다음만 표시한다.

```text
원본 표시 이름
원본 크기
kind = audio
```

길이와 codec은 전송 후 backend probe 결과이며 panel draft가 권위로 만들지 않는다. 별도 waveform이나 재생 버튼은 추가하지 않는다.

### 5.2 요청 해석

backend는 요청에 포함된 오디오를 순서대로 `audio-N`에 바인딩하고 모델에게 다음과 같은 bounded metadata만 제공한다.

```json
{
  "audioRef": "audio-1",
  "name": "battle_theme.flac",
  "durationMs": 183420,
  "codec": "flac",
  "channels": 2,
  "sampleRate": 44100
}
```

노출하지 않는 값:

- attachment UUID
- source SHA-256
- LocalAppData 경로
- FFmpeg/FFprobe 경로
- normalized temp 경로
- 최종 MPQ 경로

모델은 요청 의미에 따라 기존 코드를 조사하고 다음을 계획한다.

- 재생 조건
- 현재 플레이어 또는 모든 플레이어
- 한 번 또는 반복
- 기존 책임 파일과 lifecycle 함수

사운드 등록 자체는 `map_sound_import` 도구로만 수행한다.

### 5.3 SCMDraft 잠금

맵이 SCMDraft에 열려 있어 no-share probe가 실패하면 변경하지 않고 다음 행동을 반환한다.

```text
SCMDraft에서 현재 맵을 저장하고 닫은 뒤 다시 시도해 주세요.
```

자동으로 SCMDraft를 종료하거나 사용자 미저장 상태를 덮어쓰지 않는다.

### 5.4 검토

사운드 import 결과는 changeset에 코드 변경과 별도 map asset 항목으로 표시한다.

```text
오디오 추가
- 원본: battle_theme.flac
- 맵 경로: staredit\wav\ea_8f3c91a2d4019a77.ogg
- 길이: 03:03.420
- 출력 크기: 2.9 MB
- 맵 크기 변화: +2.9 MB
- 재생: 모든 플레이어, 반복
```

승인 시 코드와 map journal이 함께 확정된다. 거부 시 코드와 SCX를 모두 복원한다.

### 5.5 SCMDraft 확인

승인 후 SCMDraft에서 다시 열면 `Scenario → Sound Editor → Map Sound Entries`에 내부 ASCII 경로가 보여야 한다. 원본 한글 표시 이름을 SCMDraft path로 복원하지 않는다.

## 6. 포맷 정규화 계약

### 6.1 변환기 소유권

EUD Editor 3에 포함된 `Data\ffmpeg.exe`는 2020년 GPL build다. 폭넓은 입력을 지원하지만 앱의 영구 신뢰 경계로 사용하지 않는다.

`eud-agent` bootstrap이 다음 version-matched 자산을 관리한다.

```text
%localappdata%\eud-agent\bin\ffmpeg.exe
%localappdata%\eud-agent\bin\ffprobe.exe
```

각 자산은 다음 메타데이터를 release manifest와 번들 resource에 가진다.

- 정확한 upstream build/version
- download URL
- SHA-256
- license text
- source/provenance
- enabled codec/configuration record

runtime download는 기존 bootstrap checksum/atomic placement 패턴을 재사용한다. 시스템 PATH나 EUD Editor의 오래된 binary로 조용히 fallback하지 않는다. 자산이 없거나 checksum이 다르면 오디오 기능만 명시적으로 unavailable이며 기존 채팅·이미지·텍스트 기능은 유지한다.

### 6.2 입력 제한

backend와 panel 양쪽 상수를 일치시킨다.

```text
MAX_AUDIO_BYTES = 64 MiB per file
MAX_AUDIO_BYTES_PER_TURN = 128 MiB
MAX_ATTACHMENTS_PER_TURN = 5 (existing total count)
MAX_AUDIO_DURATION_MS = 3,600,000 (60 minutes)
MAX_PROBE_STDOUT = 256 KiB
MAX_NORMALIZED_AUDIO_BYTES = 64 MiB
MAX_AUDIO_CHANNELS_IN = 8
```

서버 검사가 권위다. 0바이트, cap 초과, 산술 overflow, 누락 파일은 변환 전에 실패한다.

### 6.3 probe

FFprobe를 shell 없이 직접 실행한다.

필수 process 계약:

- `CreateNoWindow`
- stdin closed
- stdout/stderr bounded
- 30초 probe deadline
- process tree Job Object containment
- network protocol 비허용
- exact staged local file 하나만 입력
- JSON output

검증:

1. 하나 이상의 audio stream 존재
2. 첫 번째 audio stream 선택
3. duration 유한, 양수, cap 이하
4. channels 1..8
5. sample rate 양수, bounded
6. codec name bounded ASCII
7. 외부 playlist/segment 의존 없음

앨범아트가 있는 M4A/MP3는 audio stream만 사용하고 attached picture는 무시한다. 비디오 컨테이너는 첫 구현의 file picker에서 노출하지 않으며 backend도 `format`이 제품 allowlist 밖이면 거부한다.

### 6.4 canonical output

FFmpeg 출력 profile은 고정한다.

```text
container: Ogg
codec: Vorbis (`libvorbis`)
sample rate: 44,100 Hz
channels: 2
quality: q=4
video/subtitle/data streams: omitted
metadata and chapters: stripped
output extension: .ogg
```

원본이 mono여도 첫 구현은 호환성과 단순한 재생 계약을 위해 stereo로 만든다. loudness normalization, gain, trim을 적용하지 않는다.

변환은 app-owned request temp directory에 쓴다. direct process args를 사용하고 shell 문자열을 만들지 않는다.

```text
-nostdin
-v error
-i <staged-content>
-map 0:a:0
-vn -sn -dn
-map_metadata -1
-map_chapters -1
-ar 44100
-ac 2
-c:a libvorbis
-q:a 4
-y <request-temp-output.ogg>
```

hard deadline은 5분이다. cancellation generation이 바뀌면 process tree를 종료하고 temp output을 삭제한다.

### 6.5 출력 검증

FFmpeg exit code 0만으로 성공 처리하지 않는다.

1. output file 존재
2. 크기 1..64 MiB
3. `OggS` magic
4. FFprobe 재실행
5. codec이 Vorbis
6. sample rate 44,100
7. channels 2
8. duration이 원본과 허용 오차 안에서 일치
9. output SHA-256 계산
10. temp file을 읽는 동안 size/mtime 변경 없음

허용 길이 오차:

```text
max(250 ms, source duration의 0.5%)
```

normalized output은 request가 끝날 때까지 session-bound cache에 보존한다. 승인 후 SCX가 bytes를 소유하므로 별도 project asset으로 승격하지 않는다.

## 7. 첨부 저장 계약

### 7.1 Rust 타입

`AttachmentKind`를 additive하게 확장한다.

```rust
pub enum AttachmentKind {
    Image,
    Text,
    Audio,
}
```

`AttachmentContext`는 모델에 경로를 노출하지 않는 audio descriptor를 추가한다.

```rust
pub struct ResolvedAudioAttachment {
    pub descriptor: AttachmentDescriptor,
    pub path: PathBuf,
    pub sha256: String,
}

pub struct AttachmentContext {
    pub image_paths: Vec<PathBuf>,
    pub images: Vec<ResolvedImageAttachment>,
    pub text_files: Vec<(String, String)>,
    pub audio_files: Vec<ResolvedAudioAttachment>,
}
```

### 7.2 판별 순서

stage 판별 순서는 다음과 같다.

1. supported image magic
2. supported audio container magic
3. UTF-8 text without NUL
4. reject binary

Audio magic은 stage 시 빠른 분류용이다. 최종 포맷 권위는 FFprobe다. 최소 분류:

- RIFF/WAVE
- OggS
- FLAC
- ID3 또는 valid MP3 frame sync
- ISO BMFF `ftyp`의 bounded audio-compatible brands
- FORM/AIFF
- ASF/WMA

magic이 모호하거나 너무 짧으면 확장자/MIME으로 audio를 승격하지 않는다.

### 7.3 session/request binding

기존 session binding 뒤 현재 request id에 audio binding을 만든다.

```text
session_id
request_id
audio_ref
audio_attachment_id
source_sha256
```

`audio_ref`는 현재 request에서만 유효하다. 다른 세션, 이전 요청, plan feedback 재전송, 임의 문자열은 거부한다.

재생 코드를 수정하는 follow-up에는 이미 맵에 등록된 sound path를 `map_sound_list`로 조회한다. 이전 `audio-ref`를 재사용하지 않는다.

## 8. 사운드 import 도구

### 8.1 model-visible schema

메인 EPS 세션에만 다음 도구를 등록한다.

```json
{
  "name": "map_sound_import",
  "input": {
    "audioRef": "audio-1"
  }
}
```

모델이 지정할 수 없는 값:

- source path
- destination MPQ path
- codec/profile
- quality
- output name
- map path
- overwrite mode
- sound slot

Map session registry에는 이 도구를 등록하지 않는다. Map Agent의 기존 여섯 공간 레이어와 target/protect 모델을 변경하지 않는다.

### 8.2 backend 처리

`map_sound_import`는 다음 순서를 지킨다.

1. current request/session/kind 확인
2. `OpenMapName` authority와 source hash 확인
3. write intent 등록
4. audio attachment binding 확인
5. FFprobe와 정규화 수행
6. output SHA-256으로 기존 맵 사운드 재사용 검색
7. 새 asset이면 MPQ path 계산
8. project operation mutex 진입
9. compiling guard와 no-share lock
10. full-file backup
11. 전용 native sound-add로 temp SCX 생성
12. exact CHK/MPQ 검증
13. atomic replace
14. journal 기록
15. mutex 해제
16. exact result 반환

MPQ path:

```text
staredit\wav\ea_<normalized-sha256-first-16-lowerhex>.ogg
```

16 hex prefix가 기존 다른 전체 hash와 충돌하면 24, 32, 64 hex 순으로 확장한다. overwrite하지 않는다.

### 8.3 중복과 idempotency

다음 경우 새 WAV 슬롯을 소비하지 않는다.

- 같은 MPQ path의 asset bytes SHA-256이 normalized output과 같음
- 해당 path string이 game string에 존재
- 해당 string id가 `WAV `에 존재

이때 `reused: true`로 기존 sound를 반환한다.

불완전 상태는 자동 수리하지 않는다.

- MPQ asset만 존재
- string만 존재
- WAV slot만 존재
- 같은 path에 다른 bytes 존재

이런 상태는 명시적 conflict로 실패하고 원본을 변경하지 않는다. 별도 수리 범위를 몰래 추가하지 않는다.

### 8.4 result

```json
{
  "soundRef": "sound-1",
  "mpqPath": "staredit\\wav\\ea_8f3c91a2d4019a77.ogg",
  "durationMs": 183420,
  "normalizedBytes": 3018201,
  "sourceCodec": "flac",
  "outputCodec": "vorbis",
  "reused": false,
  "mapSha256Before": "...",
  "mapSha256After": "..."
}
```

`mpqPath`는 코드 생성용 데이터이지 write authority가 아니다. 모델이 임의 다른 path를 코드에 쓰더라도 build/review에서 그대로 보이며 asset import 권한은 생기지 않는다.

### 8.5 read tool

기존 등록 사운드 재사용을 위해 read-only `map_sound_list`를 메인 EPS 세션에 추가한다.

반환 상한은 512개다.

```json
{
  "sounds": [
    {
      "soundIndex": 0,
      "mpqPath": "staredit\\wav\\ea_8f3c91a2d4019a77.ogg",
      "assetPresent": true,
      "managed": true
    }
  ]
}
```

`managed`는 path가 exact `staredit\wav\ea_<hex>.ogg` 형식일 때만 true다. 기존 사용자 사운드를 숨기거나 관리 대상으로 간주하지 않는다.

## 9. Native C ABI

### 9.1 전용 진입점

범용 `mapEdit`의 asset 불변식을 약화하지 않는다. `isom_capi.h/.cpp`에 별도 함수를 추가한다.

개념 서명:

```cpp
int isom_map_sound_add(
    const char* input_map_path,
    const char* output_map_path,
    const char* expected_input_sha256,
    const char* destination_mpq_path_ascii,
    const uint8_t* ogg_bytes,
    size_t ogg_length,
    char** report_json,
    char** error_message
);
```

FFI는 app-owned Rust만 호출한다. model-visible tool은 raw path/bytes에 접근하지 않는다.

### 9.2 입력 검증

C++ defense-in-depth:

- input/output path non-empty and distinct
- expected SHA-256 exact lowercase hex
- destination ASCII only
- normalized slash form exact `staredit\wav\ea_<16..64 lowercase hex>.ogg`
- `..`, absolute path, colon, forward slash, duplicate separator 거부
- OGG bytes non-null, non-empty, cap 이하
- `OggS` magic
- input map hash 일치
- SCX/SCM container만 허용; bare CHK 거부
- free WAV slot 존재 또는 exact idempotent existing sound

### 9.3 mutation

새 asset 경로:

1. `MapFile input`
2. before CHK section digests와 MPQ inventory 계산
3. free slot 확인
4. `MapFile::addSound(destMpqPath, oggBytes, WavQuality::Uncompressed)`
5. 반환 후 string id와 WAV index 확인
6. temp output에 save
7. temp re-open

OGG에는 StormLib WAV lossy compression을 적용하지 않는다. `WavQuality::Uncompressed`는 OGG bytes를 일반 MPQ file로 저장한다.

### 9.4 검증

저장 후 다음이 모두 성립해야 한다.

- MPQ에 destination path 존재
- extracted asset SHA-256 == normalized output SHA-256
- game string이 exact destination path
- 정확히 한 WAV slot이 해당 string id를 참조
- 다른 기존 WAV slots 순서와 값 불변
- 다른 game strings의 bytes/ids 불변
- 허용한 `STR`/`STRx`, `WAV ` 변화 외 CHK sections 불변
- MPQ inventory는 exact destination asset 하나만 추가
- 기존 MPQ asset names와 bytes digest 불변
- save 후 reopen 성공
- `autoDefragmentLocations=false`
- `lockAnywhere=true`

실패 시 temp/output을 삭제하고 input을 변경하지 않는다.

### 9.5 report

```json
{
  "schema": "eud-map-sound-add-report/1",
  "ok": true,
  "reused": false,
  "soundIndex": 12,
  "soundStringId": 418,
  "mpqPath": "staredit\\wav\\ea_8f3c91a2d4019a77.ogg",
  "assetSha256": "...",
  "assetBytes": 3018201,
  "inputSha256": "...",
  "outputSha256": "...",
  "unrelatedChkDigestBefore": "...",
  "unrelatedChkDigestAfter": "...",
  "unrelatedAssetDigestBefore": "...",
  "unrelatedAssetDigestAfter": "..."
}
```

Rust는 schema와 모든 invariant를 다시 확인한다.

## 10. epScript 생성 계약

### 10.1 1회 재생

현재 플레이어에게 재생:

```javascript
PlayWAV("staredit\\wav\\ea_8f3c91a2d4019a77.ogg");
```

모든 플레이어와 observer에게 재생:

```javascript
PlayWAVAll("staredit\\wav\\ea_8f3c91a2d4019a77.ogg");
```

모델은 요청 의미와 기존 코드의 CurrentPlayer 계약을 조사한다. `PlayWAVAll`을 모든 human player loop 안에서 호출해 중복 재생하지 않는다.

### 10.2 반복 재생

반복 요청은 probe로 확인된 normalized duration을 도구 결과에서 받는다. 반복 상태는 기존 feature owner가 가진 lifecycle 함수와 상태에 넣는다.

규칙:

- 기존 타이머/게임 lifecycle 패턴 재사용
- 별도 generic audio manager 생성 금지
- 한 요청에 한 BGM이고 기존 안정된 owner가 없을 때만 cohesive module 고려
- 실제 길이보다 먼저 재호출해 overlap하지 않음
- trigger cadence를 고려한 bounded guard margin 사용
- 배속·pause/resume·정밀 gapless를 약속하지 않음
- 사용자의 기본 StarCraft 음악과 겹칠 수 있음을 changeset 설명에 표시

반복 정확도는 인게임 runtime verification 대상이다. 정적 build 성공만으로 완료 처리하지 않는다.

### 10.3 기존 등록 사운드

후속 요청은 `map_sound_list`로 path를 읽고 코드만 변경한다. 이미 등록된 사운드에 원본 첨부를 다시 요구하지 않는다.

### 10.4 코드 구조

- configured MainFile을 composition root로 유지
- 현재 mutable state와 이벤트를 소유한 기존 파일에 호출 추가
- import cycle 생성 금지
- 파일 topology가 바뀌면 structure memory 완전 갱신
- 모든 modified/created EPS를 한 `eps_check` batch로 preflight
- 최종 권위는 complete-project `build_run`

## 11. Transaction, journal, review

### 11.1 write lease

`map_sound_import`는 project mutation이다. 첫 mutation 전에 FIFO write intent를 등록하고 다음이 settle될 때까지 동일 lease를 유지한다.

```text
sound import
→ EPS changes
→ build
→ changeset review
→ accept/reject
→ rollback/promote completion
```

다른 세션의 read turn은 계속 가능하다. 다른 writer는 기존 coordinator 계약을 따른다.

### 11.2 journal target

명시적인 sound map entry를 추가한다.

```rust
JournalTarget::MapSound {
    source_map: PathBuf,
    mpq_path: String,
    normalized_sha256: String,
}
```

snapshot은 full-file map backup authority와 함께 다음 감사 metadata를 가진다.

- source attachment display name
- source SHA-256
- source codec/duration/channels/rate
- normalization profile/version
- normalized SHA-256/bytes
- MPQ path
- WAV index/string id
- map before/after SHA-256
- backup path
- native report digest

원본 attachment path는 durable journal에 저장하지 않는다.

### 11.3 reject/rollback

거부 시:

1. SCMDraft lock probe 재실행
2. full backup에서 temp restore 생성
3. atomic replace
4. exact before map SHA-256 확인
5. EPS journal rollback
6. session workspace reject
7. normalized request temp/cache cleanup

맵 복구가 실패하면 lease를 유지하고 명시적 rollback failure를 표시한다. 코드만 되돌리고 성공으로 표시하지 않는다.

### 11.4 accept와 harness

사운드 import는 runtime-affecting map change다. 코드와 맵 changeset accept 후 기존 harness job은 `waiting_runtime`에 머문다.

사용자가 인게임 결과를 확인하면 문서 harness를 진행한다. `harness_skip`은 기존 계약대로 가능하며 accepted map/code는 유지한다.

## 12. Panel과 IPC

### 12.1 protocol

`panel/src/lib/protocol.ts`:

```ts
export type AttachmentKind = "image" | "text" | "audio";
```

기존 `ChatAttachment` wire shape는 additive kind만 받는다. session log, rewind, draft restore, plan feedback의 attachment metadata를 보존한다.

### 12.2 composer

`InstructionBox` 변경:

- audio file accept
- `AudioLines` 또는 기존 icon set의 음표 icon
- audio 전용 크기 오류
- image thumbnail 없음
- attachment-only message 유지
- 기존 5개 총 개수 유지
- staged audio aggregate 128 MiB 검사

plan feedback에 audio를 새로 첨부하면 plan 수정 context로는 metadata만 전달한다. 실제 `map_sound_import`는 implementation phase와 write lease owner에서만 사용할 수 있다.

### 12.3 conversation log

사용자 message에는 원본 표시 이름과 크기를 표시한다. 변경 review에는 normalized 결과와 map size delta를 표시한다. data URL 또는 audio bytes를 durable panel log에 넣지 않는다.

### 12.4 backend events

긴 변환 동안 bounded progress를 보낸다.

```text
audio_probe
audio_transcode
audio_validate
waiting_map_close
map_sound_write
map_sound_verify
```

FFmpeg stderr 원문이나 사용자 로컬 경로를 panel event에 내보내지 않는다.

## 13. 보안과 실패 처리

### 13.1 파일 신뢰

- MIME/extension 비신뢰
- magic은 stage 분류용
- FFprobe가 codec/stream authority
- FFmpeg는 exact staged file만 읽음
- network protocols disabled
- shell 금지
- output path app-owned
- path traversal 금지
- source/display name은 MPQ path에 영향 없음

### 13.2 process containment

FFmpeg와 FFprobe:

- exact managed executable checksum 재확인
- direct spawn
- Job Object kill-on-close
- no window
- stdin disabled
- finite timeout
- bounded stdout/stderr
- cancellation tree kill
- temp cleanup
- retry 없음

손상 파일은 다른 decoder나 EUD Editor FFmpeg로 fallback하지 않는다.

### 13.3 리소스 상한

- raw IPC body cap은 header보다 먼저 backend request size에서 검사
- `usize` 변환 전 `u64` checked bounds
- OGG bytes를 C ABI에 전달할 때 length cap 재검사
- native vector allocation 전 cap 재검사
- map output과 backup에 충분한 disk space가 없으면 mutation 전 실패
- 최대 512 WAV slots
- 한 tool call은 한 audioRef만 import

### 13.4 저작권

제품은 파일의 라이선스를 추론하거나 자동 차단하지 않는다. changeset에 원본 표시 이름과 다음 안내를 포함한다.

```text
이 오디오를 맵에 배포할 권한은 사용자에게 있어야 합니다.
```

이 안내는 구현을 막는 confirmation dialog가 아니라 검토 정보다.

### 13.5 오류 메시지

오류는 안정된 한국어 범주와 bounded detail을 제공한다.

- 지원하지 않는 오디오 컨테이너
- 오디오 스트림 없음
- 손상되었거나 디코딩 불가
- 길이/크기 제한 초과
- 변환기 자산 없음/손상
- 변환 시간 초과
- normalized OGG 검증 실패
- 원본 맵 stale
- EUD Editor 빌드 중
- SCMDraft에서 맵 사용 중
- WAV 슬롯 512개 사용됨
- 기존 MPQ path 충돌
- 디스크 공간 부족
- map save/verify/rollback 실패

경로, FFmpeg command line, raw stderr는 application log에 bounded redact 형태로만 남긴다.

## 14. 파일별 변경 지도

### 14.1 Rust/Tauri

- `src-tauri/src/attachment.rs`
  - `Audio` kind, magic classification, size caps, resolved audio
- `src-tauri/src/audio.rs` (new cohesive responsibility)
  - managed FFprobe/FFmpeg resolution, probe, transcode, output validation, request cache
- `src-tauri/src/config.rs`
  - managed binary/temp paths if existing `bin/` helpers are insufficient
- `src-tauri/src/bootstrap.rs`
  - FFmpeg/FFprobe versioned assets, checksums, provenance/license
- `src-tauri/src/tool_exec.rs`
  - request-local audio binding, `map_sound_import`, journal/write coordinator integration
- `src-tauri/src/tools.rs`
  - tool schema and gating
- `src-tauri/src/mapsafe.rs`
  - sound-add operation family through existing rails
- `src-tauri/src/journal.rs`
  - `MapSound` target and metadata
- `src-tauri/src/engine.rs`
  - trusted audio refs prompt section, main EPS registry only
- `src-tauri/src/ipc.rs`
  - additive attachment serialization only if needed
- `src-tauri/src/lib.rs`
  - module/service wiring
- `src-tauri/Cargo.toml`
  - no general decoder dependency; add only dependencies required by managed asset/process metadata

### 14.2 Native

- `native/isom/isom_capi.h`
  - ABI version bump and `isom_map_sound_add`
- `native/isom/isom_capi.cpp`
  - FFI validation/translation/error ownership
- `native/isom/IsomTerrain/MapAgentCore.h`
  - sound-add declaration
- `native/isom/IsomTerrain/MapAgentCore.cpp`
  - exact sound mutation, CHK/MPQ verification/report
- `native/isom/MappingCoreLib/MapFile.cpp/.h`
  - only narrow verified helpers if current public API cannot expose free slot/exact path checks
- `crates/isom-sys/build.rs` or generated bindings path
  - ABI binding refresh
- `crates/isom/src/lib.rs`
  - safe Rust wrapper and report types

### 14.3 Panel

- `panel/src/lib/protocol.ts`
  - audio kind
- `panel/src/lib/attachments.ts`
  - audio recognition hints and client caps
- `panel/src/components/InstructionBox.tsx`
  - accept filter, icon, errors
- `panel/src/components/ConversationLog.tsx`
  - audio attachment rendering
- review/changeset component owning map journal entries
  - normalized metadata and map delta

### 14.4 Distribution and docs

- release asset manifest and release script
  - FFmpeg/FFprobe packages, hashes, license/provenance
- installer/bootstrap UI
  - audio converter asset progress/failure scoped to feature
- `hivemind/docs/architecture.md`
  - accepted audio pipeline and data ownership
- `hivemind/docs/rules.md`
  - audio/SCX invariants, ASCII path, lock, slots, exact asset delta
- `hivemind/docs/tech-stack.md`
  - pinned FFmpeg/FFprobe distribution
- `hivemind/docs/verify.md`
  - automated and live SCMDraft/StarCraft scenarios
- feature docs and user-facing README
  - supported input, output OGG, limits, close/reopen requirement

문서 변경은 구현 acceptance 후 실제 동작을 반영한다. 이 계획 파일 외 문서는 계획 작성 단계에서 수정하지 않는다.

## 15. 테스트 계획

테스트는 observable contract와 plausible failure를 방어한다. source text나 단순 wiring만 확인하는 테스트를 만들지 않는다.

### 15.1 Attachment 단위 테스트

- WAV/OGG/FLAC/MP3/M4A/AIFF/ASF magic이 audio로 분류됨
- image RIFF/WEBP가 audio보다 먼저 image로 분류됨
- 임의 binary 거부
- UTF-8 text 유지
- 0 byte 거부
- 64 MiB 경계
- aggregate 128 MiB 경계
- 다른 session audio binding 거부
- duplicate attachment id 거부
- session 삭제 cleanup

### 15.2 Audio service 단위/통합 테스트

고정된 작은 fixtures를 사용한다.

- WAV → canonical OGG
- MP3 → canonical OGG
- FLAC → canonical OGG
- M4A/AAC → canonical OGG
- 이미 OGG인 입력도 canonical profile로 재인코딩
- album art 무시
- no-audio container 거부
- corrupted/truncated input 거부
- duration cap 경계
- channels > 8 거부
- timeout/cancel process tree 종료
- stdout/stderr cap
- missing/checksum-failed managed binary
- output magic/codec/rate/channel 검증
- duration tolerance
- temp cleanup on every failure

FFmpeg fixture tests는 exact pinned build에서 실행한다. codec encoder의 incidental byte-for-byte output을 assertion하지 않고 verified profile과 SHA self-consistency를 확인한다.

### 15.3 Native real-map 테스트

작은 실제 SCX fixture와 OGG fixture를 사용한다.

- MPQ에 exact path/bytes 추가
- game string 추가
- WAV slot 추가
- saved map reopen
- unrelated CHK sections digest 불변
- unrelated MPQ inventory/content digest 불변
- existing WAV slots 불변
- 511 used slots에서 마지막 slot 성공
- 512 used slots에서 byte-for-byte input 불변
- duplicate exact sound reuse
- MPQ-only/string-only/WAV-only inconsistent state 거부
- same path/different bytes 거부
- invalid destination path 거부
- non-OGG bytes 거부
- stale input hash 거부
- distinct input/output requirement
- temp save failure leaves input unchanged
- report schema round-trip

### 15.4 Rust mapsafe/tool 테스트

- compiling guard precedes backup/mutation
- SCMDraft lock precedes backup/mutation
- backup precedes native write
- native failure preserves original
- post-verify failure restores backup
- atomic replace failure preserves/reports
- journal contains map/code coherent transaction
- reject restores exact before SHA
- rollback lock failure retains lease
- another session writer waits while reads continue
- model cannot pass local path/MPQ path
- stale/cross-request `audioRef` rejected
- reused sound does not create second journaled map mutation
- map size delta matches verified output

### 15.5 Engine/tool contract 테스트

- main EPS session exposes `map_sound_import`/`map_sound_list`
- Map session does not expose import tool
- read/triage turn cannot mutate
- implementation lease required
- audio metadata prompt contains `audio-N` and no paths/UUID/SHA
- plan feedback does not import audio
- tool result supplies exact code path
- required `build_run` follows map/code mutation
- build failure keeps reviewable rollback state

### 15.6 Panel 테스트

- picker accepts common audio extensions
- drop stages audio
- attachment-only audio message
- audio icon/name/size
- client file and aggregate size errors
- image/text behavior unchanged
- rewind/edit draft retains audio metadata
- conversion progress rendering
- locked-map action message
- changeset map delta/audio metadata
- no raw bytes/data URL in durable log

### 15.7 Live acceptance: SCMDraft

실제 설치된 SCMDraft에서 수행한다.

1. 저장된 테스트 SCX를 선택한다.
2. WAV, MP3, FLAC, M4A 각각 한 파일로 import 요청을 수행한다.
3. 적용 전 원본 SHA와 backup 존재를 확인한다.
4. 적용 후 SCMDraft로 SCX를 연다.
5. `Scenario → Sound Editor`의 `Map Sound Entries`에 각 managed path가 존재하는지 확인한다.
6. 각 항목의 SCMDraft Play 기능을 확인한다.
7. SCMDraft에서 맵을 저장하고 닫은 뒤 다시 열어 항목이 유지되는지 확인한다.
8. SCMDraft에 맵을 열어 둔 상태에서 새 import가 잠금 오류로 거부되는지 확인한다.
9. 기존 수동 import 사운드가 변경되지 않았는지 확인한다.
10. 거부/rollback 후 managed entry와 MPQ bytes가 제거되고 원본 SHA가 복원되는지 확인한다.

### 15.8 Live acceptance: EUD Editor와 StarCraft

1. EUD Editor 프로젝트의 `OpenMapName`에 테스트 SCX를 연결한다.
2. 첨부 효과음을 특정 이벤트의 현재 플레이어에게 1회 재생하도록 요청한다.
3. build 성공과 output SCX asset/WAV 등록을 확인한다.
4. 2인 이상 게임에서 대상 플레이어에게만 1회 들리는지 확인한다.
5. `PlayWAVAll` 요청은 observer를 포함해 각 client에서 한 번만 들리는지 확인한다.
6. 긴 BGM을 게임 시작 시 1회 재생한다.
7. 반복 요청은 최소 세 번 경계에서 overlap 없이 재호출되는지 확인한다.
8. 네트워크 지연이 있는 방에서 반복 gap과 중복 여부를 기록한다.
9. 기본 StarCraft 음악과 겹침 안내가 실제 UX에 나타나는지 확인한다.
10. 코드/map changeset 거부가 양쪽을 정확히 복구하는지 확인한다.
11. accept 후 session 첨부를 삭제하고 다시 build해도 사운드가 유지되는지 확인한다.
12. 앱·에디터 재시작 후 기존 등록 사운드로 코드-only follow-up이 가능한지 확인한다.

## 16. 구현 단계

각 단계는 중간 제품 범위를 뜻하지 않는다. 최종 acceptance까지 feature는 미완료다.

### Phase A — 첨부와 변환 자산

1. pinned FFmpeg/FFprobe distribution contract와 license/provenance를 고정한다.
2. bootstrap asset을 추가한다.
3. `AttachmentKind::Audio`와 server caps를 추가한다.
4. request-local audio binding을 추가한다.
5. panel picker/chip/error를 추가한다.
6. `audio.rs` probe/transcode/validation/cache를 구현한다.
7. attachment/audio service focused tests를 통과시킨다.

완료 조건: 일반 입력이 canonical OGG로 bounded 변환되지만 아직 맵은 바뀌지 않는다.

### Phase B — Native sound import

1. ABI version을 올린다.
2. `isom_map_sound_add`를 추가한다.
3. free slot, duplicate, inconsistent state 검사를 구현한다.
4. exact CHK/MPQ delta 검증을 구현한다.
5. Rust safe wrapper/report parsing을 추가한다.
6. real SCX native tests를 통과시킨다.

완료 조건: 별도 input/output SCX에서 exact sound import와 검증이 가능하지만 model tool은 없다.

### Phase C — Main EPS tool transaction

1. `map_sound_list`를 추가한다.
2. `map_sound_import` schema와 request binding을 추가한다.
3. mapsafe operation과 journal target을 추가한다.
4. ProjectWriteCoordinator lease와 rollback을 연결한다.
5. engine trusted audio metadata를 추가한다.
6. map/code coherent changeset 표시를 추가한다.
7. focused Rust/tool tests를 통과시킨다.

완료 조건: 모델이 opaque `audio-N`을 import하고 정확한 MPQ path를 받아 코드 변경과 함께 review할 수 있다.

### Phase D — epScript behavior와 build

1. agent instruction에 1회/전체/반복 계약을 추가한다.
2. 기존 owner에 `PlayWAV`/`PlayWAVAll` 코드를 생성한다.
3. 반복은 duration metadata를 사용한다.
4. `eps_check`와 complete `build_run`을 강제한다.
5. build/reject/accept/harness lifecycle을 검증한다.

완료 조건: 코드와 맵이 한 lease와 changeset으로 build/review/rollback된다.

### Phase E — 실제 표면 검증과 문서 반영

1. SCMDraft live acceptance 전체를 수행한다.
2. EUD Editor/StarCraft live acceptance 전체를 수행한다.
3. 실패하는 실제 계약을 고치고 재검증한다.
4. architecture/rules/tech-stack/verify를 실제 구현으로 갱신한다.
5. user-facing 지원 포맷·제약을 갱신한다.
6. 사용하지 않는 scaffold와 임시 fixture를 정리한다.

완료 조건: 아래 전체 acceptance criteria가 증거와 함께 충족된다.

## 17. 최종 acceptance criteria

1. 사용자는 WAV, OGG, MP3, FLAC, M4A/AAC, WMA, AIFF, Opus 중 지원 codec 파일을 메인 대화에 첨부할 수 있다.
2. backend는 extension/MIME이 아니라 probe로 오디오를 검증한다.
3. 출력은 exact canonical OGG Vorbis profile이다.
4. 모델은 로컬 path, attachment UUID, 변환기 path, 임의 MPQ destination을 받거나 만들지 않는다.
5. `audio-N`은 exact session/request에서만 유효하다.
6. 등록된 asset은 `staredit\wav\ea_<hex>.ogg` ASCII path다.
7. SCX MPQ, game string, `WAV `가 함께 추가되고 저장 후 재검증된다.
8. 요청한 OGG 하나 외 모든 MPQ asset은 byte-for-byte 불변이다.
9. 허용한 sound/string sections 외 모든 CHK section은 불변이다.
10. 512 slot 고갈, duplicate conflict, partial inconsistent state가 input 불변으로 실패한다.
11. 동일 normalized sound 재요청은 asset과 WAV slot을 중복 추가하지 않는다.
12. SCMDraft가 맵을 열고 있으면 backup/write 전에 실패한다.
13. 모든 write는 compiling guard, full backup, source hash, temp output, atomic replace, post-verify를 통과한다.
14. 코드와 map import는 한 ProjectWriteCoordinator lease와 changeset에 속한다.
15. reject는 코드와 SCX의 exact before state를 복원한다.
16. build 실패는 부분 성공으로 표시되지 않고 review/rollback 상태를 유지한다.
17. 승인된 SCX는 session 첨부가 삭제되어도 SCMDraft 표시와 재빌드를 유지한다.
18. SCMDraft Sound Editor에 managed entry가 보이고 재열기 후 유지된다.
19. 현재 플레이어 재생은 다른 player에게 중복 재생되지 않는다.
20. 전체 플레이어 재생은 각 client에서 정확히 한 번 들린다.
21. 반복 BGM은 측정된 길이를 사용하고 최소 세 경계에서 overlap 없이 동작한다.
22. unsupported/DRM/MIDI/corrupt/oversized 입력은 명시적 오류로 원본 map/code를 변경하지 않는다.
23. FFmpeg/FFprobe는 pinned checksum, direct spawn, bounded output, deadline, Job Object, cancellation cleanup을 사용한다.
24. Map Agent의 기존 공간 레이어 authority와 original Apply 계약은 변경되지 않는다.
25. 기존 이미지·텍스트 첨부, map image placement, EPS write/build/review 회귀가 없다.
26. 실제 SCMDraft, EUD Editor, StarCraft에서 §15.7과 §15.8을 완료한다.

## 18. 완료 후 영구 규칙

구현이 승인되면 다음 규칙을 `rules.md`에 승격한다.

- 오디오 입력 포맷은 저장 포맷이 아니다. 맵 sound asset은 canonical OGG Vorbis만 허용한다.
- 사용자 파일명을 MPQ sound path로 사용하지 않는다.
- model-visible sound tool은 opaque request-local ref만 받는다.
- sound import는 MPQ + game string + WAV slot의 검증된 원자 변경이다.
- 512 WAV slot을 넘기지 않는다.
- requested sound 외 MPQ asset delta를 허용하지 않는다.
- SCMDraft lock과 EUD Editor compiling 중에는 map sound를 쓰지 않는다.
- accepted SCX가 sound bytes의 durable authority다.
- runtime-affecting sound change는 사용자 인게임 확인 전 harness를 완료하지 않는다.
