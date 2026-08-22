# Map Agent Image-to-Terrain Placement — Implementation Plan

Status: implemented 2026-08-22; historical implementation plan. Runtime authority is `architecture.md`, `rules.md`, `tech-stack.md`, `verify.md`, and the Map Agent system guide.

## 0. 구현 결과

- `MapRequestAuthority::calculate`가 no-target 전체 candidate 지원 레이어, current-target
  union 축소, layer-aware persistent protect 우선을 한 번 계산하며 patch/finalize/replay/
  Apply verifier가 같은 snapshot을 사용한다. Reference/anchor와 omitted stored target은
  scope를 바꾸지 않고 exact object/location fingerprint binding은 유지된다.
- `src-tauri/src/map_image.rs`가 10 MiB encoded limit, 16,777,216 decode pixels, allocation
  overflow, PNG/JPEG/WebP/GIF-first-frame, bounded normalization, Lanczos3 aspect resize, preview
  PNG, tile-grid SHA-256, changed rows, unique tile, walkability, height report를 소유한다.
- ABI v5 `isom_image_quantize`가 기존 native asset cache로 graphics-valid SD tile palette,
  stable RGB dedupe/tie, candidate alpha composite, Bayer 8x8 quantization을 수행하고 packed
  exact tiles/preview RGB/metadata를 반환한다. 8개 installed tileset real-asset 검사를
  통과했다.
- 직접 `사진 배치`는 실제 WebView2 canvas에서 drag, corner resize, tile snap, 숫자,
  Arrow/Shift+Arrow, original/result toggle, stale sequence/digest, report, protect block,
  one-revision confirm을 제공한다. 1280x800과 1920x1080에서 horizontal overflow가 없었다.
- Map request 이미지는 기존 `localImage`와 동시에 request-local `image-1..N`으로
  binding된다. `map_image_place`는 path/palette/MTXM/matrix 입력 없이 일반
  `TerrainBlit` batch를 만들며 multiple image와 normal terrain patch를 양방향으로
  interleave한다.
- Candidate manifest는 `TerrainBlit`과 비권한 conversion metadata만 저장한다. 실제
  attachment 삭제 뒤 replay, r1→r0→r1, MTXM/TILE equality, original SHA 유지,
  trusted Apply full backup/atomic verification, exact-byte undo를 실제 saved SCX에서
  확인했다.

## 1. 목표

Map Agent Workbench에서 사진을 현재 맵의 지형 타일 모자이크로 변환해 미니맵에 사진처럼 보이게 한다. 변환 결과는 별도 미니맵 이미지가 아니라 `MTXM`/`TILE` 지형 변경이다.

동일한 변환 엔진을 두 진입점에서 사용한다.

1. 사용자가 사진을 직접 선택하고 현재 맵 캔버스 위에서 이동·비율 크기 조절한 뒤 후보 맵에 반영한다.
2. 사용자가 현재 요청에 사진을 첨부하면 에이전트가 일반 지형 변환 권한 안에서 맵의 위치와 크기를 결정해 후보 맵에 반영한다.

두 흐름 모두 원본 SCX를 직접 쓰지 않는다. 사진 배치의 `후보에 반영`은 candidate revision만 생성하며, 원본 기록은 기존 Map Agent의 신뢰된 사용자 `맵에 적용` 동작만 수행한다.

완료 상태는 다음 전체 흐름이다.

```text
사용자 직접 배치
  → 사진 선택
  → 현재 맵 위에 변환 오버레이 생성
  → 타일 단위 이동·비율 크기 조절
  → 실제 양자화 결과와 게임성 변화 확인
  → 후보에 반영
  → candidate/diff 검토
  → 기존 사용자 Apply
  → 원본 hash·빌드·파일 잠금·백업 재검사
  → 원본에 원자 적용

에이전트 배치
  → 사진 첨부
  → 사진이 현재 요청의 imageRef로 바인딩
  → 에이전트가 현재 맵을 분석하고 위치·크기 결정
  → 전용 map_image_place 호출
  → 서버가 사진에서 타일 배열을 결정해 request draft에 기록
  → 검증된 candidate revision 승격
  → candidate/diff 검토
  → 기존 사용자 Apply
```

## 2. 확정 결정

다음 결정은 구현 중 재해석하지 않는다.

1. **적용 의미**: UseMapEditor와 같은 지형 타일 변환이다. 게임 지형을 유지한 채 미니맵 표시만 교체하는 기능이 아니다.
2. **확인 동작**: 사진 배치 화면의 확인은 candidate revision만 만든다. 원본 SCX는 기존 `맵에 적용`에서만 쓴다.
3. **직접 조작**: 이동, 원본 비율을 유지하는 크기 조절, 타일 단위 스냅, 숫자 좌표 입력을 지원한다.
4. **에이전트 위치 결정**: 사진이 현재 요청에 첨부되면 에이전트가 일반 지형 변환 권한 안에서 맵의 위치와 크기를 정할 수 있다.
5. **변환 우선순위**: 사진 재현을 우선한다. 보행 가능 여부와 고도 변화는 차단하지 않고 확인 전에 수치로 경고한다.
6. **공유 엔진**: 사용자 직접 배치와 에이전트 배치는 같은 서버 측 디코딩·리사이즈·양자화·검증 경로를 사용한다.
7. **모델 제한**: 모델은 타일 ID 행렬을 직접 만들지 않는다. request-local `imageRef`와 `x/y/width/height`만 전용 도구에 전달한다.
8. **기본 Map 권한**: target 영역이 없으면 현재 candidate 전체의 지원 레이어가 기본 작업 범위다. target은 권한을 새로 부여하지 않고 범위를 좁힌다.
9. **보호 우선**: protect는 기본 전체 맵 권한과 target 범위 모두에서 해당 셀·레이어의 변경을 금지한다. reference와 anchor는 읽기 맥락이며 쓰기 범위를 넓히지 않는다.
10. **사진 권한 통합**: 사진 첨부는 별도 권한을 만들지 않는다. `map_image_place`는 현재 요청의 일반 terrain 권한을 소비하는 `image → terrain.blit` 변환 도구다.
11. **원본 Apply 금지**: 모델과 사진 변환 서비스는 원본 Apply, backup restore, 임의 파일 경로를 호출할 수 없다.
12. **라이선스 경계**: UseMapEditor 소스는 동작 참고 자료로만 사용한다. 소스 코드나 라이브러리 구현을 복사하지 않는다.

### 2.1 Map Agent 기본 작업 범위

현재 target-required 권한 계산은 사용자의 의도와 맞지 않으므로 이 기능의 선행 작업으로 수정한다.

```text
current agent request에 target mention 없음
  → 현재 candidate 전체가 기본 작업 범위
  → Map Agent가 지원하는 terrain, units, buildings, doodads, sprites, locations 허용

current agent request에 target mention 1개 이상
  → 좌표 기반 변경 범위를 target 셀과 target의 허용 레이어 합집합으로 축소

protect 있음
  → 위 작업 범위에서 protect 셀과 protect 레이어를 항상 제외

reference/anchor 있음
  → 읽기·비교·배치 맥락만 제공
  → 작업 범위를 확대하지 않음
```

정확한 object/location instance mention의 기존 instance-bound 권한은 유지한다. target 영역이 있으면 일반 좌표 기반 변경만 그 영역으로 좁아지며, 명시적으로 멘션된 개체 권한을 자연어가 다른 개체로 확대하지 못한다.

자연어는 기본 전체 맵 권한을 새로 만드는 근거가 아니다. 전체 맵 권한은 Map session이 원본과 분리된 candidate만 수정하고 원본 Apply는 사용자에게만 허용한다는 제품 계약에서 온다. 자연어는 작업 의도를 전달하고, target/protect 및 지원 operation allowlist가 실제 범위를 결정한다.

이 변경으로 target은 매 요청마다 반드시 그려야 하는 허가증이 아니라, 사용자가 특정 부분에 초점을 맞추고 변경 범위를 좁힐 때 사용하는 선택적 제약이 된다.

## 3. 상류 동작 분석과 라이선스 경계

참고 저장소:

- `https://github.com/Buizz/UseMapEditor`
- `UseMapEditor/Windows/MinimapImageWindow.xaml`
- `UseMapEditor/Windows/MinimapImageWindow.xaml.cs`
- `UseMapEditor/FileData/TileSet.cs`

확인된 사용자 동작은 다음과 같다.

1. 파일 대화상자에서 이미지를 연다.
2. 원본 비율을 유지한 채 맵 최대 변에 맞는 타일 크기로 축소한다.
3. 현재 타일셋의 SD 미니맵 대표색으로 indexed palette를 구성한다.
4. Bayer 8×8 ordered dithering으로 이미지를 팔레트 색상에 양자화한다.
5. 각 팔레트 색을 유효한 MTXM 타일 ID에 매핑한다.
6. 결과를 타일 복사 브러시로 만들어 사용자가 맵 위에 배치한다.

해당 저장소는 GitHub 메타데이터의 `license`가 `null`이고 루트에 라이선스 파일이 없다. 따라서 다음만 재사용한다.

- 사용자가 관찰할 수 있는 사진→팔레트→타일 변환 개념
- 비율 유지 크기 조절
- Bayer 8×8 ordered dithering이라는 알고리즘 선택
- 현재 타일셋의 유효 타일만 사용하는 제약

다음은 재사용하지 않는다.

- C#/XAML 소스 코드
- KGySoft 호출 구조
- 클래스·메서드 설계
- 타일셋 파일 경로 처리
- 색상 dictionary 구성 구현

우리 구현은 기존 `native/isom`의 StarCraft asset loader와 Rust candidate 안전 흐름을 사용한 독립 구현이어야 한다.

## 4. 범위

### 4.1 포함

- Map Agent 툴바의 `사진 배치` 진입점
- PNG, JPEG, WebP, GIF 첫 프레임 디코딩
- session-bound 이미지 stage/resolve
- 인코딩 크기와 디코딩 픽셀 상한
- 현재 맵 위 사진 오버레이
- 타일 단위 이동
- 원본 비율을 유지하는 타일 단위 크기 조절
- `X`, `Y`, `너비`, `높이` 숫자 입력
- 키보드 이동 대안
- 원본 오버레이와 실제 변환 결과 전환
- 현재 타일셋의 결정적 사진용 타일 팔레트
- 비율 유지 리사이즈
- Bayer 8×8 ordered dithering
- 투명 픽셀의 기존 지형 보존
- 부분 투명 픽셀의 기존 지형색 합성
- 보행 가능 상태와 고도 변화 수 계산
- target이 없을 때 candidate 전체의 지원 레이어를 허용하는 기본 Map 권한
- target이 있을 때 해당 셀·레이어로 범위를 좁히는 권한 계산
- protect의 셀·레이어 우선 차단
- 현재 요청 첨부 이미지를 request-local `imageRef`로 바인딩
- 에이전트용 `map_image_place` MCP 도구
- server-generated 일반 `terrain.blit` batch
- candidate revision, diff, revert, discard, Apply/undo 연동
- stale source/revision, 맵 경계, target, protect mask 검증
- 8개 StarCraft 타일셋 지원

### 4.2 제외

- 게임 지형을 유지한 채 미니맵 이미지만 교체
- 런타임 EUD 오버레이나 커스텀 미니맵 렌더러
- 회전, 원근 변형, 자유 비율 왜곡
- Photoshop식 다중 이미지 레이어
- 범용 crop editor
- 색상 보정, 필터, 밝기/대비 편집
- 사용자의 범용 exact-tile brush 직접 배치
- 일반 palette brush 기능
- 다른 타일셋의 타일 혼합
- 사진에서 unit/doodad/sprite 생성
- 보행/고도 보존 모드
- 모델의 원본 Apply
- 원본 SCX 즉시 기록
- EUD Editor 3 소스나 바이너리 수정

이 기능은 기존 `map-agent-workbench-plan.md`의 “palette brush 직접 배치 제외”를 뒤집지 않는다. 사용자가 임의 팔레트 타일을 칠하는 기능이 아니라, 서버가 검증된 사진 변환 결과 하나를 candidate에 기록하는 제한된 경로다.

## 5. 현재 기반과 격차

### 5.1 재사용할 현재 기능

- `panel/src/map/MapCanvas.tsx`
  - pan/zoom/grid
  - map↔screen transform
  - native crop bitmap 렌더
  - selection/diff/object overlay
  - pointer capture와 requestAnimationFrame 조절
- `panel/src/map/canvasTransform.ts`
  - map/screen 좌표 변환
  - visible crop 계산
- `panel/src/map/MapToolbar.tsx`
  - 원본/후보/diff와 Apply/undo 상태
- `panel/src/map/MapPromptInput.tsx`
  - 파일 선택, drag/drop, clipboard 이미지 첨부
- `src-tauri/src/attachment.rs`
  - opaque attachment ID
  - 이미지 magic-byte 검사
  - session binding
  - `%localappdata%` 저장과 stale cleanup
- `src-tauri/src/map_model.rs`
  - `TerrainBlit`
  - `MapMentionSnapshot`
  - candidate revision/baseline 모델
- `src-tauri/src/map_candidate.rs`
  - request draft
  - operation batch
  - verified candidate finalize
  - replay/revert/discard
- `src-tauri/src/map_verify.rs`
  - target/protect/layer 검증
  - MTXM/TILE 일치 검사
- `src-tauri/src/map_agent.rs`
  - Map Agent 창 신뢰 경계
  - source probe와 stale 처리
  - user-only Apply/undo
- `native/isom/IsomTerrain/MapAgentCore.cpp`
  - StarCraft asset loading/cache
  - CV5/VX4/VR4/WPE terrain pixels
  - exact tile graphics validation
  - `terrain.blit`
  - tile metadata와 실제 지형 렌더
- `crates/isom`, `crates/isom-sys`
  - safe Rust/C ABI 경계
  - native buffer ownership과 오류 변환

### 5.2 새로 필요한 기능

- 이미지 디코딩과 bounded normalization
- 사진용 결정적 타일 팔레트 추출
- ordered dither quantizer
- 현재 지형색을 이용한 알파 합성
- 변환 결과 PNG preview
- 보행/고도 변화 보고서
- 임시 image placement UI state
- image placement 전용 pointer mode
- direct preview/confirm IPC
- 기본 전체 맵/선택적 target/protect 권한 계산
- 현재 요청 attachment의 request-local image reference
- `map_image_place` 전용 MCP tool
- 변환 metadata를 포함한 일반 `terrain.blit` candidate batch

## 6. 사용자 직접 배치 UX

### 6.1 진입과 초기 배치

Map Agent 툴바에 `사진 배치` 버튼을 추가한다. 버튼은 다음 상태에서 비활성화한다.

- candidate/bootstrap이 없음
- source가 stale
- agent turn 또는 Apply가 진행 중
- 현재 맵을 다시 읽는 중

버튼을 누르면 browser file input을 연다. 별도 임의 SCX 선택기는 추가하지 않는다.

이미지 stage가 성공하면 다음 초기 transform을 만든다.

- 출력은 원본 종횡비를 유지한다.
- 초기 출력은 현재 맵 안에 들어가는 가장 큰 비율 유지 크기로 시작한다.
- 원본 픽셀 수를 타일 수로 직접 해석하지 않으며 최소 `1×1`, 최대 맵 크기로 제한한다.
- 초기 위치는 맵 중앙이다.
- 모든 값은 정수 타일 좌표다.

초기 transform은 UX 기본값일 뿐 권한이나 최종 적용 값이 아니다.

### 6.2 캔버스 조작

사진 배치가 활성화된 동안 canvas pointer 우선순위는 다음과 같다.

```text
resize handle
  > image body move
  > pan gesture
  > 기존 select/inspect hit testing
```

지원 조작:

```text
Drag image body        위치 이동
Drag corner handle     원본 비율 유지 크기 조절
Arrow                   1타일 이동
Shift + Arrow           8타일 이동
Esc                     임시 배치 취소 확인
Enter                   최신 preview가 유효하면 후보에 반영
```

규칙:

- transform은 항상 정수 타일 좌표다.
- 전체 출력 사각형은 맵 내부여야 한다.
- 최소 출력은 `1×1`이다.
- 최대 출력은 맵 `width×height`다.
- 비율 고정은 해제할 수 없다.
- 정수 반올림으로 원본 비율과 소수점 오차가 생길 수 있으며 실제 `width×height`를 숫자 필드에 표시한다.
- drag 외에 `X/Y/너비/높이` 입력을 제공한다.
- icon-only handle/control은 `aria-label`을 갖는다.

### 6.3 미리보기

두 보기 모드를 제공한다.

1. `원본 오버레이`
   - source image를 반투명하게 표시한다.
   - 위치와 구도를 빠르게 잡는 용도다.
2. `적용 결과`
   - 서버가 생성한 대표색 PNG를 pixelated 확대 표시한다.
   - 각 미리보기 픽셀은 최종 타일 하나를 뜻한다.

리사이즈 중 매 pointermove마다 서버 변환을 호출하지 않는다.

- 이동 중에는 기존 preview bitmap을 새 위치에 그린다.
- 크기 조절 중에는 기존 bitmap을 임시 확대한다.
- pointerup과 숫자 입력 debounce 후 실제 preview를 다시 요청한다.
- preview 요청에는 monotonically increasing sequence를 포함한다.
- 늦게 도착한 sequence는 폐기한다.
- confirm은 최신 transform과 preview digest가 일치할 때만 활성화한다.

### 6.4 배치 검사 패널

사진 오버레이와 함께 다음을 표시한다.

- 원본 파일명
- 원본 픽셀 크기
- 출력 타일 크기
- 배치 좌표
- 실제 변경 셀 수
- 고유 MTXM 타일 수
- 보행 가능 상태 변경 셀 수
- 고도 변경 셀 수
- protect 충돌 셀 수
- preview 생성 상태/오류

`protect 충돌 셀 수 > 0`이면 confirm을 차단한다. 보행/고도 변화는 사진 재현 우선 결정에 따라 경고만 표시한다.

### 6.5 확인과 취소

버튼 문구:

- `후보에 반영`
- `취소`

`후보에 반영`은 최신 preview digest와 transform을 backend에 보낸다. backend는 preview 결과를 신뢰하지 않고 attachment와 transform으로 변환을 다시 계산한 뒤 digest가 같은지 확인한다.

성공 시:

- 정확히 한 candidate revision을 생성한다.
- image placement 임시 상태를 제거한다.
- candidate 또는 diff 보기를 표시한다.
- 기존 candidate revert/discard가 동작한다.

취소 시:

- candidate와 원본 바이트는 변하지 않는다.
- 임시 overlay/cache만 정리한다.
- 사용하지 않은 draft attachment는 기존 attachment cleanup 정책에 맡긴다.

## 7. 에이전트 배치 UX와 공통 지형 권한

### 7.1 Request-local image reference

Map Agent composer의 기존 이미지 첨부 흐름을 유지한다. `참고 이미지`와 `지형 사진으로 사용`을 나누는 별도 권한 토글은 추가하지 않는다.

`MapChatCommand.attachments`의 session-bound 이미지가 현재 request에 들어오면 backend가 안정된 request-local reference를 만든다.

```text
image-1
image-2
image-N
```

각 reference의 backend 상태:

```text
session_id
request_id
attachment_id
attachment_sha256
source_width
source_height
candidate_revision_key
baseline_hash
```

모델에는 기존 `localImage` 입력과 함께 사용할 수 있는 `imageRef` 목록을 제공한다. tool runtime은 `imageRef`를 실제 attachment에 해석한다. opaque attachment ID, 로컬 파일 경로, SHA-256은 모델이 만들거나 바꿀 수 없다.

`imageRef`는 입력 asset binding이지 쓰기 권한이 아니다. 현재 request가 끝나거나 reset되면 더 이상 도구 입력으로 사용할 수 없다. candidate replay는 attachment나 `imageRef`에 의존하지 않는다.

### 7.2 공통 지형 작업 범위

에이전트 `map_image_place`는 일반 지형 operation과 같은 request 작업 범위를 사용한다. 사용자 직접 배치는 아래 no-target terrain 범위를 사용한다.

```text
current agent request에 target mention 없음
  → current candidate의 전체 terrain 허용

current agent request에 target mention 1개 이상
  → target 셀 중 terrain이 허용된 셀의 합집합만 허용

protect 영역
  → 위 범위에서 terrain protect 셀 제외
```

`target`은 사진 배치 권한을 생성하지 않는다. 기본 전체 맵 범위를 특정 부분으로 좁히는 선택적 제약이다. 자연어 좌표는 target/protect를 확대하거나 무시하지 못한다.

사진 변환 사각형은 맵 내부여야 한다. verifier는 최종 `before_mtxm != after_mtxm`인 실제 변경 셀마다 공통 terrain 작업 범위를 검사한다. 투명 픽셀 등으로 기존 타일을 유지한 셀은 변경 권한을 소비하지 않는다.

사용자 직접 배치는 모델 요청 권한이 아니라 신뢰된 캔버스 조작이다. 일반 no-target terrain 범위와 같은 전체 candidate terrain을 사용하고 persistent protect만 제외한다. 저장되어 있지만 현재 agent request에 멘션되지 않은 target은 직접 배치를 제한하지 않는다.

### 7.3 에이전트 도구

전용 MCP 도구:

```text
map_image_place
```

입력:

```json
{
  "imageRef": "image-1",
  "x": 0,
  "y": 0,
  "width": 64,
  "height": 64
}
```

계약:

- `imageRef`는 현재 Map request의 session-bound 이미지여야 한다.
- 위치와 크기는 정수 타일이다.
- 출력 사각형은 맵 내부여야 한다.
- server가 이미지를 디코딩하고 타일 배열을 만든다.
- 변환 결과를 일반 `TerrainBlit`으로 구성한다.
- 생성된 `TerrainBlit`을 기존 request draft의 공통 terrain 권한과 verifier에 전달한다.
- target이 없으면 전체 terrain이 허용된다.
- target이 있으면 실제 변경 셀이 target terrain 범위 안에 있어야 한다.
- protect와 겹치는 실제 변경 셀은 거부한다.
- 모델이 `tiles` 행렬, MTXM ID, 파일 경로, palette를 전달하지 않는다.
- conversion metadata와 tile grid digest는 감사·재현 정보이며 권한이 아니다.

사진 변환은 draft를 seal하지 않는다. 권한이 동일하므로 같은 request에서 다음을 허용한다.

- 여러 사진의 연속 배치
- 사진 배치 후 일반 `map_draft_patch`
- 일반 지형 수정 후 사진 배치
- `map_draft_reset` 후 재시도

모든 operation은 같은 target/protect 범위를 개별적으로 통과해야 한다.

### 7.4 모델 프롬프트 규칙

Map Agent system guide에 다음을 추가한다.

- target이 없으면 현재 candidate 전체가 기본 작업 범위다. target 누락을 이유로 mutation을 거부하거나 사용자에게 영역 선택을 요구하지 않는다.
- target이 있으면 사용자가 특정 부분에 초점을 맞춘 것이므로 실제 변경을 그 셀·레이어 안으로 제한한다.
- protect는 항상 변경 금지다.
- 사용자의 요청이 첨부 사진을 지형으로 적용하는 것이면 해당 request의 `imageRef`로 `map_image_place`를 호출한다.
- 첨부 이미지를 분석·참고만 하라는 요청에서는 지형 mutation을 만들지 않는다. 이 구분은 별도 권한 토글이 아니라 사용자의 작업 의도다.
- image placement는 terrain만 바꾼다.
- target이 없으면 맵 분석 결과로 `x/y/width/height`를 결정한다.
- target이 있으면 target 안에서 위치와 크기를 결정한다.
- 사진 타일은 보행/고도를 바꿀 수 있으므로 도구 결과의 경고를 사용자 답변에 포함한다.
- 원본 Apply를 요청하거나 암시하지 않는다.

## 8. 이미지 디코딩과 정규화

### 8.1 지원 형식

- PNG
- JPEG
- WebP
- GIF 첫 프레임

애니메이션을 지형 애니메이션으로 변환하지 않는다.

### 8.2 제한

기존 `MAX_IMAGE_BYTES = 10 MiB`를 유지하고 디코딩 제한을 추가한다.

- 최대 원본 dimensions: codec metadata를 읽은 직후 검사
- 최대 decode pixels: 16,777,216 pixels
- 최대 normalized dimensions: 현재 맵 dimensions 이하
- 최대 output cells: 65,536
- 0 width/height 거부
- integer multiplication overflow는 allocation 전에 거부
- truncated/corrupt/unsupported color layout은 명시적 오류

이미지 디코더는 Rust dependency의 default features를 끄고 필요한 `png`, `jpeg`, `webp`, `gif` codec만 활성화한다. 범용 image manipulation API를 UI나 모델에 노출하지 않는다.

### 8.3 정규화 캐시

반복 resize preview가 매번 10MB 원본을 재디코딩하지 않도록 `MapImageService`가 제한된 in-memory cache를 소유한다.

cache key:

```text
session_id + attachment_id + attachment_sha256
```

cache value:

```text
bounded RGBA source + original dimensions
```

규칙:

- normalized source의 긴 변은 최대 256이다.
- session당 활성 placement image 하나만 강하게 보존한다.
- 취소, session 전환, source reload에서 해제한다.
- candidate manifest는 캐시에 의존하지 않는다.

## 9. 사진용 타일 팔레트

### 9.1 소유 계층

팔레트는 `native/isom`이 생성한다. Rust나 React가 타일 asset을 다시 파싱하지 않는다.

이유:

- native layer가 이미 CV5/VX4/VR4/WPE를 로드한다.
- exact tile graphics validity가 native authority다.
- 타일 metadata와 실제 SD pixel을 같은 asset snapshot에서 읽을 수 있다.
- 별도 두 번째 타일 파서를 만들면 규칙과 결과가 갈라진다.

### 9.2 팔레트 엔트리

각 엔트리:

```text
rgb
mtxm
walkability_class
height_class
scan_order
```

생성 규칙:

1. 현재 tileset의 CV5 group/variant를 안정된 순서로 순회한다.
2. `tileGraphicsValid`인 exact tile만 허용한다.
3. SD terrain pixel에서 고정된 대표색을 추출한다.
4. 같은 RGB가 여러 타일에 있으면 가장 이른 안정된 scan order의 타일을 사용한다.
5. 결과 순서를 tileset과 asset bytes에 대해 결정적으로 유지한다.
6. transparent/invalid graphics를 palette에 넣지 않는다.

대표색 추출 좌표와 quantizer version은 코드 상수로 버전 관리한다. 변경은 기존 candidate replay를 바꾸지 않지만 동일 입력 재현 결과를 바꾸므로 문서와 golden fixture를 함께 갱신해야 한다.

### 9.3 캐시

StarCraft assets는 기존 native cache를 재사용한다. 사진 팔레트는 `tileset + asset identity + quantizer version`별로 한 번 계산한다.

## 10. 리사이즈·알파·양자화

### 10.1 리사이즈

- output `width×height`는 타일 해상도다.
- source aspect ratio를 유지한다.
- UI와 backend 모두 같은 integer dimension resolver를 사용하되 backend 결과가 authority다.
- high-quality downsampling을 사용한다.
- output이 최대 256×256이므로 전체 작업 메모리는 bounded다.

### 10.2 알파 처리

각 output cell에서 source RGBA를 계산한다.

- alpha `0`: 현재 candidate의 해당 MTXM을 그대로 사용한다.
- alpha `1..254`: 현재 candidate 타일의 대표 미니맵 RGB 위에 source RGB를 alpha composite한 뒤 양자화한다.
- alpha `255`: source RGB를 바로 양자화한다.

이 규칙은 투명 로고 주변의 지형을 불필요하게 덮지 않는다. 실제 변경 셀은 `before_mtxm != after_mtxm`인 셀만 센다.

### 10.3 Bayer 8×8 ordered dithering

- 고정된 8×8 threshold matrix를 사용한다.
- threshold와 channel adjustment는 정수 연산으로 구현한다.
- nearest palette distance는 결정적이다.
- 거리 동률은 palette scan order가 빠른 엔트리를 고른다.
- SIMD나 병렬화가 결과 순서를 바꾸면 안 된다.
- 동일 source bytes, transform, candidate tiles, tileset, quantizer version은 동일 tile grid SHA-256을 생성해야 한다.

### 10.4 출력

변환 결과:

```text
tiles: rectangular Vec<Vec<u16>>
preview_rgb: one representative color per output tile
changed_cells: canonical row spans
unique_tile_count
walkability_changed_cells
height_changed_cells
protected_conflicts
tile_grid_sha256
quantizer_version
```

preview는 기존 `png` crate로 bounded RGB PNG를 인코딩한다.

## 11. Candidate와 검증 계약

### 11.1 preview는 mutation이 아니다

`map_agent_image_preview`는 다음을 변경하지 않는다.

- candidate file
- candidate manifest
- request draft
- visible revision
- source SCX
- write coordinator
- backup/journal

preview는 attachment, current candidate snapshot, transform을 읽는 순수 계산이다.

### 11.2 confirm의 server 재계산

UI가 보내는 preview PNG나 tile grid를 신뢰하지 않는다.

`map_agent_image_confirm`은 다음을 받는다.

```text
session_id
attachment_id
revision_key
x/y/width/height
preview_digest
preview_sequence
```

backend는 다음을 수행한다.

1. Map Agent window label 확인
2. session/candidate/revision/baseline 확인
3. attachment session binding과 SHA-256 확인
4. 같은 transform으로 변환 재계산
5. preview digest 일치 확인
6. confirm 시점의 공통 terrain 작업 범위, protect, 맵 경계 검사
7. request-owned temporary draft에서 일반 `TerrainBlit` 실행
8. MTXM/TILE 및 candidate verification
9. 정확히 한 visible revision으로 finalize
10. 실패 시 temporary draft와 변환 metadata 삭제

### 11.3 이미지 변환 metadata

candidate manifest에는 일반 `TerrainBlit` operation과 함께 다음 변환 metadata를 저장한다.

```text
kind = image_conversion
attachment_sha256
source_dimensions
placement
quantizer_version
tile_grid_sha256
changed_cells
walkability_changed_cells
height_changed_cells
```

metadata는 감사, diff 설명, 동일 입력 재현 확인을 위한 정보다. 권한을 부여하거나 범위를 확대하지 않는다. attachment ID와 로컬 파일 경로는 장기 replay authority가 아니다. 최종 typed `TerrainBlit` operation과 operation order가 candidate replay의 입력이며, tile grid digest는 저장된 operation의 무결성을 확인한다.

### 11.4 공통 권한 검증

일반 지형 operation과 사진 변환 operation은 같은 `MapRequestAuthority` 계산과 검증을 사용한다.

- target이 없으면 current candidate의 전체 terrain cell을 허용한다.
- target이 하나 이상이면 terrain이 허용된 target cell의 합집합으로 범위를 좁힌다.
- protect terrain cell은 두 경우 모두 제외한다.
- `map_draft_patch`의 terrain operation과 `map_image_place`가 만든 `TerrainBlit`은 같은 cell-level verifier를 통과한다.
- 사진 변환 경로는 target mask를 만들거나 별도 추가 권한을 주지 않는다.
- image batch 뒤에도 draft를 seal하지 않으며 후속 operation을 같은 권한으로 검증한다.
- 각 batch 적용 직후와 final replay에서 operation 결과, MTXM/TILE 일치, 작업 범위, protect를 다시 확인한다.

### 11.5 Apply와 replay

최종 candidate manifest에는 실제 tile values가 있으므로 attachment cleanup 이후에도 deterministic replay가 가능하다.

기존 Apply rails를 변경하지 않는다.

- 현재 저장 `OpenMapName`
- project/path 일치
- source hash 일치
- compiling guard
- no-share lock probe
- full-file backup
- deterministic revision-chain replay
- same-directory atomic replacement
- post-write canonical/container verification
- pending Apply journal
- explicit user undo

## 12. IPC와 데이터 모델

### 12.1 Rust 모델

추가 타입의 책임:

```text
MapImageRequestRef
MapImagePlacement
MapImageDescriptor
MapImagePreviewRequest
MapImagePreview
MapImageConfirmCommand
MapImageConversionReport
MapImageConversionMetadata
```

모든 외부 입력 구조는 `deny_unknown_fields`와 camelCase를 유지한다.

`MapImagePlacement` 제약:

```text
x: u16
y: u16
width: u16, >= 1
height: u16, >= 1
```

맵 내부 검사는 deserialize 이후 current candidate dimensions에 대해 수행한다.

기존 `MapRequestAuthority` 계산도 함께 바꾼다.

- region target이 없으면 map dimensions 전체와 Map Agent 지원 레이어를 기본 허용한다.
- region target이 있으면 좌표 기반 작업을 target의 셀·레이어 합집합으로 좁힌다.
- protect는 두 경우 모두 제외한다.
- request-local image reference는 authority가 아니라 attachment binding이다.

### 12.2 Tauri commands

추가 command:

```text
map_agent_image_preview
map_agent_image_confirm
map_agent_image_cancel
```

세 command 모두 `require_map_window`를 통과해야 한다. main window나 외부 WebView 호출은 거부한다. `map_agent_image_cancel`은 candidate/source를 건드리지 않고 해당 session의 normalized image cache만 해제한다.

preview 응답의 PNG는 기존 binary Tauri IPC 패턴을 사용하고, metadata/report는 bounded JSON header 또는 별도 serializable envelope로 전달한다. base64 PNG를 JSON에 넣지 않는다.

### 12.3 MCP tool

`map_image_place`는 Map session registry에만 등록한다.

등록하지 않는 곳:

- EPS session
- main agent tool registry
- original Apply surface

tool descriptor는 정확한 schema만 노출하며 `parameters` wrapper를 사용하지 않는다.

도구 실행은 별도 image authority를 확인하지 않는다. 현재 request의 `imageRef`를 해석한 뒤 생성한 `TerrainBlit`을 일반 terrain 권한 검사에 전달한다.

### 12.4 이벤트

confirm/finalize 성공 후 기존 `map_candidate_state` event를 재사용한다. 사진 기능 전용 candidate 상태를 두 번째로 만들지 않는다.

preview 진행 상태는 UI-local request state로 처리하며 conversation event history에 기록하지 않는다.

## 13. 코드 변경 계획

### 13.1 Native/C ABI

대상:

- `native/isom/IsomTerrain/MapAgentCore.cpp`
- `native/isom/IsomTerrain/MapAgentCore.h`
- native C ABI header/implementation
- `crates/isom-sys`
- `crates/isom/src/lib.rs`

변경:

- 사진용 tile palette 생성/cache
- representative SD color 추출
- walkability/height metadata 반환
- bounded RGBA→tile quantization C ABI
- native allocation/free와 오류 변환
- panic/exception이 ABI를 넘지 않도록 기존 containment 유지

### 13.2 Rust core

대상:

- `src-tauri/Cargo.toml`
- `src-tauri/src/lib.rs`
- `src-tauri/src/attachment.rs`
- `src-tauri/src/map_model.rs`
- `src-tauri/src/map_candidate.rs`
- `src-tauri/src/map_verify.rs`
- `src-tauri/src/map_agent.rs`
- `src-tauri/src/tools.rs`
- `src-tauri/src/tool_exec.rs`
- 필요 시 새 `src-tauri/src/map_image.rs`

변경:

- feature-gated image decoder dependency
- session-bound image resolve API
- bounded decode/normalize/cache
- target이 없을 때 전체 맵을 허용하는 `MapRequestAuthority` 계산
- target이 있을 때 셀·레이어 범위를 좁히고 protect를 제외하는 검증
- request attachment의 request-local `imageRef` binding
- preview/confirm service
- server-generated 일반 `TerrainBlit` candidate batch
- 권한과 분리된 conversion metadata
- direct Tauri commands
- Map MCP tool와 execution

`map_image.rs`는 디코딩·변환 orchestration이라는 하나의 응집된 책임을 가지므로 새 파일이 정당하다. generic `utils`나 이미지 전반 abstraction은 만들지 않는다.

### 13.3 React panel

대상:

- `panel/src/map/MapAgentApp.tsx`
- `panel/src/map/MapCanvas.tsx`
- `panel/src/map/MapToolbar.tsx`
- `panel/src/map/mapProtocol.ts`
- 필요 시 `panel/src/map/ImagePlacementControls.tsx`

변경:

- toolbar entry와 file input
- placement state ownership
- canvas overlay draw/drag/resize
- numeric/keyboard controls
- preview sequence와 stale response 폐기
- report/warning UI
- confirm/cancel
- 기존 attachment UI 재사용; 별도 이미지 권한 토글이나 image mention UI는 추가하지 않음

기존 map canvas의 crop cache와 bitmap cleanup 규칙을 유지한다. 새 `ImageBitmap`도 취소·교체·unmount에서 `close()`한다.

### 13.4 문서

구현 완료 시 이 계획을 실제 결과에 맞춰 다음 authority 문서에 반영한다.

- `hivemind/docs/architecture.md`: no-target 전체 candidate 기본 범위, target 축소, protect 우선 흐름
- `hivemind/docs/rules.md`: “target만 쓰기 권한” 계약을 “기본 전체 맵 + 선택적 target 축소” 계약으로 교체하고 사진 변환이 일반 terrain 권한을 사용하도록 갱신
- `hivemind/docs/tech-stack.md`: 최소 image decoder와 native quantizer dependency
- `hivemind/docs/verify.md`: no-target/target/protect 권한 회귀와 사진 배치 smoke
- Map Agent 실제 feature 문서와 system guide: target 누락 시 거부·선택 요구 제거

이 계획 파일은 구현 전 단일 계획 authority다. 구현 완료 후에는 역사 계획으로 남고 실제 동작 계약은 위 문서가 authority가 된다.

## 14. 구현 순서

### Phase A — Map Agent 기본 권한 모델

1. 현재 target-required `MapRequestAuthority` 생성 경로를 한 곳으로 모은다.
2. region target이 없으면 current candidate dimensions 전체와 지원 레이어를 기본 허용한다.
3. region target이 있으면 좌표 기반 작업 범위를 target 셀·레이어 합집합으로 좁힌다.
4. protect 셀·레이어를 항상 작업 범위에서 제외한다.
5. reference/anchor와 exact object/location mention의 기존 읽기·instance-bound 의미를 유지한다.
6. tool runtime과 verifier가 동일한 계산 결과를 사용하게 한다.
7. Map Agent system guide에서 target 누락 시 거부·선택 요구 지침을 제거한다.

완료 조건:

- target 없는 terrain/unit/building/doodad/sprite/location candidate mutation이 허용된다.
- target이 있으면 좌표 기반 변경이 해당 셀·레이어 밖에서 거부된다.
- protect는 target 유무와 관계없이 변경을 거부한다.
- 모델은 여전히 원본 Apply와 지원하지 않는 CHK 영역을 사용할 수 없다.

### Phase B — 변환 엔진

1. image decoder dependency를 최소 feature로 추가한다.
2. bounded decode/normalize를 구현한다.
3. native 사진 팔레트와 metadata API를 구현한다.
4. deterministic Bayer 8×8 quantizer를 구현한다.
5. alpha composite와 current-terrain preservation을 구현한다.
6. tile grid digest, preview, 게임성 report를 만든다.

완료 조건:

- synthetic palette fixture에서 결과가 byte-stable하다.
- 8개 tileset real-asset fixture에서 모든 output tile이 graphics-valid다.

### Phase C — Candidate 서비스

1. session-bound image resolve와 request-local `imageRef`를 추가한다.
2. preview를 순수 계산으로 연결한다.
3. 변환 결과를 일반 `TerrainBlit`으로 기존 candidate patch 경로에 전달한다.
4. 권한과 분리된 conversion metadata를 저장한다.
5. direct confirm이 정확히 한 revision을 finalize하게 한다.
6. 사진 배치 전후의 일반 terrain operation 조합을 허용한다.
7. replay/revert/discard를 연결한다.

완료 조건:

- preview/cancel은 candidate bytes를 바꾸지 않는다.
- confirm은 candidate만 바꾼다.
- 사진과 일반 terrain operation이 같은 target/protect 검증을 통과한다.
- attachment 삭제 뒤에도 revision replay가 된다.

### Phase D — 직접 배치 UI

1. toolbar entry를 추가한다.
2. canvas image interaction mode를 추가한다.
3. drag/resize/numeric/keyboard 조작을 구현한다.
4. original/result preview toggle을 구현한다.
5. report와 confirm/cancel을 연결한다.
6. stale preview와 source reload를 처리한다.
7. confirm 시점에 전체 candidate terrain과 persistent protect를 backend에서 검증한다.

완료 조건:

- 별도 target 선택 없이 맵 어디든 직접 배치할 수 있다.
- persistent protect와 겹치는 실제 변경은 거부된다.
- 1280×800과 1920×1080에서 horizontal overflow가 없다.
- drag 외 조작 대안이 있다.
- 최신 preview가 아니면 confirm할 수 없다.

### Phase E — 에이전트 배치

1. 현재 request의 이미지 attachment를 `imageRef`로 tool runtime에 바인딩한다.
2. `map_image_place` schema와 execution을 추가한다.
3. 생성한 `TerrainBlit`을 일반 terrain authority와 verifier에 전달한다.
4. 같은 request에서 여러 사진 배치와 일반 terrain operation 조합을 허용한다.
5. Map Agent system guide를 갱신한다.

완료 조건:

- 별도 이미지 권한이나 target 선택 없이 첨부 사진을 전체 맵 terrain에 배치할 수 있다.
- target이 있으면 사진의 실제 변경 셀이 target terrain 범위를 벗어나지 않는다.
- protect와 겹치는 실제 변경은 거부된다.
- 사진 배치와 일반 terrain patch가 동일한 권한 계약을 사용한다.

### Phase F — 통합 검증과 문서

1. focused Rust/native/panel test를 실행한다.
2. 실제 Map Agent WebView2에서 배치 UI를 smoke한다.
3. 실제 SCX candidate render와 minimap 결과를 비교한다.
4. target 없음/target 있음/protect의 실제 agent 흐름을 smoke한다.
5. 기존 Apply/backup/undo를 smoke한다.
6. authority 문서를 실제 구현에 맞춰 갱신한다.

## 15. 테스트 계획

### 15.1 변환 단위 테스트

- aspect ratio dimension resolver
- 1×1, 세로형, 가로형, 정사각형
- 최대 256×256
- decode pixel cap과 overflow
- PNG/JPEG/WebP/GIF first frame
- corrupt/truncated image
- 완전 투명 픽셀의 before tile 보존
- 부분 alpha composite
- Bayer 8×8 threshold golden
- nearest-color tie의 stable scan order
- 같은 입력의 tile grid SHA-256 안정성
- palette에 invalid graphics가 없음

### 15.2 Native real-asset 테스트

8개 tileset 각각:

- palette가 비어 있지 않음
- 모든 MTXM이 `tileGraphicsValid`
- RGB duplicate가 결정적으로 dedupe됨
- preview color와 chosen tile representative color 일치
- output `TerrainBlit` round-trip
- MTXM과 TILE 동일

실제 StarCraft asset이 필요한 검사는 기존 ignored native fixture 패턴을 따른다.

### 15.3 Candidate/권한 테스트

- target 없는 request가 terrain과 나머지 지원 레이어 mutation을 허용
- target 없는 `map_image_place`가 맵 전체 terrain 안에서 성공
- target이 있으면 target 내부 terrain mutation 성공
- target이 있으면 target 외부 실제 변경 셀 거부
- protect actual-change 충돌은 target 유무와 관계없이 거부
- reference/anchor가 작업 범위를 확대하지 않음
- exact object/location mention의 기존 instance binding 유지
- preview는 candidate file/manifest를 바꾸지 않음
- cancel은 visible revision을 바꾸지 않음
- direct confirm은 한 revision만 생성
- stale revision key 거부
- stale baseline hash 거부
- 다른 session/request의 `imageRef` 거부
- 맵 밖 placement 거부
- transparent unchanged cell은 target/protect 충돌 수에 포함되지 않음
- 사진 변환 뒤 일반 `map_draft_patch` 성공
- 일반 terrain patch 뒤 사진 변환 성공
- 같은 request에서 여러 `map_image_place` 성공
- attachment cleanup 뒤 candidate replay 성공
- Apply는 기존 user-only command만 가능

### 15.4 Panel 테스트

- toolbar disabled/enabled state
- file selection과 stage error
- initial centered placement
- drag move tile snap
- corner resize aspect lock
- boundary clamp
- numeric inputs
- arrow/Shift+arrow
- original/result toggle
- out-of-order preview response 폐기
- stale preview에서 confirm disabled
- 별도 target 없이 직접 배치가 맵 전체에서 가능
- persistent protect conflict에서 confirm disabled
- candidate event 후 placement state cleanup
- Esc/cancel에서 candidate 불변
- 기존 attachment UI에 별도 권한 토글이 추가되지 않음
- 현재 request image가 backend의 request-local `imageRef`로 전달됨
- 1280×800, 1920×1080 horizontal overflow 없음

### 15.5 실제 UI smoke

실제 Tauri/WebView2와 실제 저장 SCX에서 다음을 수행한다.

1. Map Agent를 연다.
2. 사진 배치를 시작한다.
3. overlay를 서로 다른 두 위치로 이동한다.
4. 크기를 한 번 키우고 한 번 줄인다.
5. 원본/적용 결과를 전환한다.
6. report가 갱신되는지 확인한다.
7. `후보에 반영` 전 원본 SHA-256이 유지되는지 확인한다.
8. candidate render와 diff bounds가 배치 사각형에 맞는지 확인한다.
9. candidate revert가 이전 revision을 복원하는지 확인한다.
10. 다시 배치하고 기존 Apply를 실행한다.
11. backup이 생성되고 원본이 candidate와 일치하는지 확인한다.
12. Apply undo가 exact backup bytes를 복원하는지 확인한다.

에이전트 흐름:

1. target selection 없이 사진을 첨부하고 지형 적용을 요청한다.
2. 에이전트가 별도 영역 선택을 요구하지 않고 맵 안 위치와 크기를 정하는지 확인한다.
3. 일반 terrain 수정과 사진 배치를 같은 request에서 수행한다.
4. target 영역을 추가하고 사진 실제 변경 셀이 그 안으로 제한되는지 확인한다.
5. target 밖 배치 도구 호출이 거부되는지 확인한다.
6. protect 영역을 추가하고 겹치는 실제 변경을 거부하는지 확인한다.
7. 수정 batch가 candidate revision만 만들고 원본은 유지하는지 확인한다.

## 16. 완료 기준

다음이 모두 충족되어야 완료다.

1. target이 없는 Map request가 현재 candidate 전체의 지원 레이어를 수정할 수 있다.
2. target이 있으면 좌표 기반 변경 범위가 해당 셀·레이어로 좁아진다.
3. protect는 target 유무와 관계없이 해당 셀·레이어 변경을 차단한다.
4. target 누락만을 이유로 에이전트가 mutation을 거부하거나 영역 선택을 요구하지 않는다.
5. 사진 선택만으로 원본 SCX와 candidate가 바뀌지 않는다.
6. 오버레이가 현재 맵 위에서 타일 단위로 이동한다.
7. 원본 비율을 유지하며 크기를 조절할 수 있다.
8. drag 외 숫자와 키보드 조작 대안이 있다.
9. 실제 양자화 결과를 mutation 전에 볼 수 있다.
10. preview와 confirm 재계산의 tile grid digest가 일치한다.
11. `후보에 반영`은 정확히 한 candidate revision만 만든다.
12. candidate revert/discard가 사진 적용 전 상태를 복원한다.
13. 기존 `맵에 적용` 전에는 원본 SCX SHA-256이 유지된다.
14. 기존 Apply의 backup, lock, compiling, hash, journal, atomic replace, verification이 유지된다.
15. 첨부 사진은 별도 권한 없이 현재 request의 `imageRef`로 제공된다.
16. `map_image_place`는 일반 terrain 권한과 verifier를 사용한다.
17. target이 없으면 에이전트가 맵 전체에서 사진 위치와 크기를 결정할 수 있다.
18. target이 있으면 사진의 실제 변경 셀이 target terrain 범위를 벗어나지 않는다.
19. 에이전트가 선택한 위치는 맵 내부이고 protect를 침범하지 않는다.
20. 모델은 파일 경로, tile grid, MTXM ID를 직접 제공하지 않는다.
21. 사진 배치와 일반 terrain operation을 같은 request에서 조합할 수 있다.
22. 8개 tileset에서 모든 생성 MTXM이 graphics-valid다.
23. MTXM과 TILE은 저장 후 동일하다.
24. 같은 입력과 상태는 같은 tile grid SHA-256을 만든다.
25. 보행 가능 상태와 고도 변화 수가 확인 전에 표시된다.
26. 실제 WebView2에서 배치·후보 검토·Apply·undo 전체 흐름이 동작한다.

## 17. 주요 위험과 대응

### 17.1 사진 재현이 게임성을 바꿈

원인: 색상에 가장 가까운 타일은 기존 셀과 보행 가능 여부·고도가 다를 수 있다.

대응:

- 확인 전에 변화 수를 계산한다.
- 사진 재현 우선 결정에 따라 warning으로 표시한다.
- protect mask는 계속 강제한다.
- 향후 게임성 보존 모드는 별도 승인된 범위로 추가한다.

### 17.2 기본 전체 맵 권한으로 예상보다 넓은 후보 변경

원인: target이 없을 때 agent가 current candidate 전체의 지원 레이어를 수정할 수 있다.

대응:

- 모든 agent mutation은 원본과 분리된 candidate에만 기록한다.
- 지원 operation과 레이어 allowlist를 유지한다.
- protect는 기본 전체 맵 권한보다 우선한다.
- 사용자는 특정 부분만 바꾸고 싶을 때 target으로 범위를 좁힌다.
- candidate 전체 diff와 verification을 Apply 전에 표시한다.
- 원본 Apply는 계속 신뢰된 사용자 동작만 허용한다.

### 17.3 미리보기와 실제 결과 불일치

원인: frontend resize/preview를 최종 결과로 신뢰하거나 out-of-order 응답을 표시한다.

대응:

- backend가 preview와 confirm 모두 계산한다.
- sequence와 digest를 사용한다.
- confirm에서 재계산한다.
- 최신 preview가 아니면 confirm을 비활성화한다.

### 17.4 대형 이미지 메모리 사용

원인: compressed size가 작아도 dimensions가 큰 이미지가 decode 메모리를 크게 사용할 수 있다.

대응:

- codec metadata에서 dimensions를 먼저 검사한다.
- decode pixel cap을 강제한다.
- output은 최대 256×256이다.
- session당 bounded normalized source 하나만 cache한다.

### 17.5 상류 코드 라이선스 불명확

원인: UseMapEditor 저장소에 명시적 라이선스가 없다.

대응:

- 코드를 복사하지 않는다.
- 사용자 관찰 가능 동작만 참고한다.
- 기존 native asset APIs와 자체 deterministic quantizer를 구현한다.

### 17.6 exact tile 편집과 ISOM 의미 차이

원인: 사진은 의미 지형 브러시가 아니라 정확한 MTXM/TILE 모자이크다.

대응:

- 기존 `TerrainBlit`과 동일하게 MTXM/TILE을 함께 쓴다.
- dimensions/tileset/ISOM section을 임의로 변경하지 않는다.
- 문서와 UI에 실제 지형 타일 변경임을 명시한다.
- 후속 ISOM 편집이 사진 영역을 다시 바꿀 수 있다는 점을 결과 설명에 포함한다.

## 18. 최종 불변식

- 원본 SCX는 신뢰된 사용자 Apply 전에는 바뀌지 않는다.
- target이 없으면 current candidate 전체의 Map Agent 지원 레이어가 기본 작업 범위다.
- target이 있으면 좌표 기반 작업 범위가 target 셀·레이어 합집합으로 좁아진다.
- protect는 target 유무와 관계없이 항상 우선한다.
- reference/anchor는 쓰기 범위를 확대하지 않는다.
- 사진 배치는 별도 권한이 아니라 일반 terrain 권한을 사용하는 `image → TerrainBlit` 변환이다.
- request-local `imageRef`는 attachment binding이며 권한이 아니다.
- 사진 배치는 terrain 외 레이어를 바꾸지 않는다.
- 모델은 original Apply를 호출할 수 없다.
- 모델은 타일 ID 행렬을 선택하지 않는다.
- 변환 결과는 유효한 현재 tileset 타일만 사용한다.
- MTXM과 TILE은 동일한 최종 타일 배열을 가진다.
- failed/cancelled/stale placement는 visible candidate와 원본을 byte-for-byte 유지한다.
- candidate revision은 attachment 수명과 무관하게 replay할 수 있다.
- 같은 입력 상태는 같은 tile grid digest를 만든다.
