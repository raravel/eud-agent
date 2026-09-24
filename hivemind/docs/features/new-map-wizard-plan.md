# New Map Wizard — Authoritative Implementation Plan

Status: implemented (2026-09-20). This document is the contract for blank-map project creation
from the launcher. Implementation notes: the shim's exists/delete/replace calls were switched to
wide Win32 APIs (ABI v7) so non-ASCII project paths work; `config.starcraft_path` now takes
precedence over the default install folder; start locations are assigned only to
human/computer/rescuable slots; the `broodWar` option writes UTF-8 strings (SC:R semantics).

## 1. 목표

`eud-agent`에서 새 프로젝트를 만들 때 SCMDraft 2를 먼저 켜서 빈 맵을 저장할 필요가 없어야 한다.
런처의 "새 프로젝트"는 두 갈래를 제공한다.

1. **기존 맵으로 시작** — 현재 `setup_create_project` 경로. 유지한다.
2. **빈 맵으로 시작** — 새 마법사. 타일셋·크기·초기 지형은 물론 SCMDraft 2가 별도 다이얼로그로
   미루는 맵 제목/설명, 플레이어 슬롯·종족·포스, 시작 위치까지 한 흐름에서 끝내고, 생성된 맵의
   미리보기를 보여준 뒤 프로젝트를 연다.

SCMDraft 2 New Map 다이얼로그의 입력은 타일셋, 가로, 세로, 초기 지형 네 가지다. 이 마법사는 그
네 가지를 포함하는 상위 집합이어야 하며, 어떤 항목에서도 SCMDraft 2로 돌아갈 이유를 남기지
않는다.

## 2. 확정 결정

1. **빌드 출력 이름은 `build/[EUD]<name>.<ext>`다.** 새 맵 마법사와 기존 맵 열기 경로
   (`create_project_from_map`) 모두 이 규칙을 쓴다. 이미 생성된 프로젝트의 매니페스트는 손대지
   않는다.
2. **빈 맵 생성은 런처 마법사에만 노출한다.** 에이전트 도구나 설정 화면에는 노출하지 않는다.
   활성 프로젝트의 소스 맵 교체는 별개의 안전 경계이며 이번 범위가 아니다.
3. **StarCraft 데이터 경로(CASC)가 없으면 fail-closed다.** 지형 데이터 없이는 ISOM 정합 맵을
   만들 수 없다. 마법사는 옵션을 비활성화하고 이유를 표시하며, 그 자리에서 SC 경로를 선택해
   `config.starcraft_path`에 저장할 수 있게 한다.
4. **생성은 네이티브 엔진(`MappingCoreLib` + `MapGenCli::newFilledMap`) 한 번의 호출로 끝난다.**
   맵을 만든 뒤 `playeredit`/`mapedit`를 연쇄 호출하지 않는다. 시작 위치·슬롯·종족·포스·제목은
   저장 전에 같은 `MapFile` 객체에 설정한다.
5. **생성된 맵은 저장 직후 재열기·CHK 추출·검증을 거친다.** 요청한 크기/타일셋/플레이어 수/시작
   위치 수가 CHK에서 그대로 읽혀야 성공이다.
6. **destination은 비어 있어야 하고, 실패 시 비어 있지 않은 사용자 폴더를 삭제하지 않는다.**
   맵은 `<destination>/maps/<name>.scx`에 직접 생성하며 임시 위치를 거치지 않는다.
7. **매니페스트 경로의 `[`/`]` 금지는 EDS 섹션 헤더에 등장하는 경로에만 유지한다.** `output_map`은
   `[main]`의 값 위치에만 쓰이므로 대괄호를 허용한다. 소스·플러그인·Python·source_map 경로는
   지금처럼 금지한다.
8. **마법사 UI는 SCMDraft 2와 같을 필요가 없다.** 기능 집합만 상위여야 한다.

## 3. 근거와 현재 기반

### 3.1 네이티브 엔진

- `native/isom/MappingCoreLib/MapFile.h:29` — `MapFile(tileset, width, height)` 새 맵 생성자,
  `save(path, overwriting)` StormLib 저장.
- `native/isom/IsomTerrain/MapGenCli.cpp:74` — `newFilledMap(tileset, w, h, terrainType)`: ISOM
  캐시로 전체를 한 지형으로 채우고 `updateTilesFromIsom`으로 TILE/MTXM을 정합시킨다. 타일셋 이름
  매핑(`tilesetNames`)과 `findTerrainType(brush name|id)`도 같은 파일에 있다.
- `native/isom/MappingCoreLib/Scenario.h` — `setScenarioName`, `setScenarioDescription`,
  `setSlotType`, `setPlayerRace`, `setForceName`, `setForceFlags`, `addUnit` (Start Location은
  `Sc::Unit::Type::StartLocation` 유닛).
- `MapAgentCore.cpp:2089 catalogQuery` — `kind: "brushes"`로 타일셋별 지형 브러시 목록을 이미
  제공한다. 마법사의 초기 지형 드롭다운은 이를 그대로 쓴다.
- `isom_capi.h`에는 새 맵 생성 엔트리가 없다. `isom_map_sound_add`가 JSON 보고서를 돌려주는
  shim 패턴(`guardSeh`, 인자 검증, `ISOM_ERR_*`)을 따른다.
- `crates/isom-sys/build.rs`가 MSBuild로 `isom_capi.sln`을 소스 빌드하므로 C ABI 추가는 일반
  소스 변경이다. `MapGenCli.cpp`는 이미 rerun-if-changed 목록에 있다.

### 3.2 Rust / Tauri

- `src-tauri/src/setup.rs create_project_from_map` — 매니페스트 생성 + 맵 복사. 출력 이름은
  구현 전 `build/{name}_EUD.{extension}`였고 지금은 `default_output_map`이 `build/[EUD]{name}.{ext}`를 만든다.
- `src-tauri/src/map_context.rs:209 resolve_starcraft_path` — `STARCRAFT_PATH` env → 표준 설치
  경로 → `config.starcraft_path`. 패널에서 이 값을 설정하는 커맨드는 없다.
- `src-tauri/src/native_project.rs:2085 normalize_relative` — 모든 매니페스트 경로에서 `[`/`]`를
  거부한다. 근거는 euddraft `readconfig.py:32`의 섹션 헤더 정규식 `\[(.+)\]$`이며, `[main]` 값
  위치의 `output: build/[EUD]x.scx`는 `fullmatch`에 걸리지 않는다. `applyeuddraft.py:158-250`은
  `output`을 경로로만 사용한다.
- `src-tauri/src/chk.rs` — `parse_map_header`, `parse_players`(start_locations), `parse_units`,
  `parse_strings`로 생성 결과를 독립 검증할 수 있다.
- `src-tauri/src/map_agent.rs:835 thumbnail_rgba` + `encode_rgba_png` — 미리보기 렌더 재사용.

### 3.3 패널

- `panel/src/setup/ProjectLauncher.tsx` — `onCreate/onOpenPicker/onImport` 세 액션.
- `panel/src/App.tsx:2015 runProjectSetupAction` — `setup_pick_project_path | setup_create_project`
  두 커맨드를 같은 busy/오류 표면으로 실행한다.
- `panel/src/lib/ipc.ts:730 setupCreateProject` — 응답은 `SetupStatusResponse`.

## 4. 설계

### 4.1 C ABI: `isom_map_new`

```c
int isom_map_new(
    const char* output_map_path,
    const char* starcraft_path,
    const uint8_t* spec_json, size_t spec_len,
    uint8_t** out_report_json, size_t* out_report_len);
```

spec (schema `eud-agent/map-new/1`, 미지 필드 거부):

```json
{
  "schema": "eud-agent/map-new/1",
  "tileset": 0,
  "width": 128, "height": 128,
  "terrainType": 3,
  "title": "My Map", "description": "",
  "players": [
    { "slot": 0, "type": "human", "race": "userSelectable", "force": 0,
      "start": { "x": 2048, "y": 2048 } }
  ],
  "forces": [ { "name": "Force 1", "allied": true, "alliedVictory": true, "sharedVision": false, "randomStart": false } ]
}
```

- `tileset` 0–7, `width`/`height` 64–256, `terrainType`은 해당 타일셋 브러시 인덱스(0은 무효).
- `players` 0–8개, `slot` 중복 금지. `type` ∈ human|computer|rescuable|neutral|inactive|closed.
  `race` ∈ zerg|terran|protoss|userSelectable|random. `start`가 있으면 픽셀 좌표가 맵 안이어야 한다.
- `forces` 1–4개. `players[].force`는 존재하는 인덱스여야 한다.
- 처리: `newFilledMap` → SPRP/OWNR/SIDE/FORC/Start Location 설정 → `save(path, false)` → 재열기
  → 보고서 `{ok, outputSha256, width, height, tileset, startLocations, chkDigest}`.
- 출력 경로가 이미 존재하면 `ISOM_ERR_INVALID_ARG`. 모든 예외/SEH는 기존 shim 규칙대로 상태 코드로.

### 4.2 Rust `crates/isom`

- `pub fn map_new(output, starcraft, spec: &MapNewSpec) -> Result<MapNewReport, NativeCallError>`.
  `MapNewSpec`/`MapNewReport`는 `serde` + `deny_unknown_fields`.
- 테스트: `crates/isom/tests`에 실제 SC 데이터가 있을 때만 도는 생성→`chk_extract`→헤더/플레이어
  검증 테스트(기존 environment-ignore 관례).

### 4.3 Tauri 커맨드 (`setup.rs`)

| 커맨드 | 역할 |
|---|---|
| `setup_map_new_options` | SC 경로 해석 결과 + 타일셋 목록 + 타일셋별 브러시(`catalog_query kind=brushes`) 반환. 경로가 없으면 `starcraft: {available:false, reason}`. |
| `setup_pick_starcraft_path` | 폴더 선택 → CASC 존재 검증 → `config.starcraft_path` 저장 → 옵션 재반환. |
| `setup_create_blank_project` | `{destination?, spec}` 수신. destination 미지정 시 폴더 다이얼로그. 비어있음 검사 → `maps/<name>.scx` 생성(`isom::map_new`) → `chk` 독립 검증 → 매니페스트(`build/[EUD]<name>.scx`) → 미리보기 PNG(base64) 포함한 `SetupStatusResponse` 확장 반환. 어느 단계든 실패하면 생성된 맵/매니페스트만 제거하고 폴더는 남긴다. |

`create_project_from_map`의 출력 이름도 `build/[EUD]{name}.{extension}`으로 바꾼다.
`normalize_relative`는 `allow_brackets` 구분을 두어 `output_map` 검증에서만 대괄호를 허용한다.

### 4.4 시작 위치 자동 배치

패널이 계산하고 spec에 픽셀 좌표로 넣는다(엔진은 좌표만 검증). 프리셋:

- 마진 4타일. 모든 시작 위치를 맵 좌상단에 모아 둔다: 4×3타일 셀을 한 줄에 4개씩 좌→우, 위→아래로
  채워 P1..P8이 순서대로 읽히고 서로 겹치지 않는다. 퍼뜨리는 배치는 제공하지 않으며, 제작자가
  이후 원하는 자리로 옮긴다(사용자 결정, 2026-09-20).
- "자동 배치 안 함"을 고르면 `start`를 생략한다. 좌표 직접 입력은 이번 범위에서 제외한다.

### 4.5 패널 마법사

`panel/src/setup/NewMapWizard.tsx`, 런처 "새 프로젝트" 아래 "빈 맵으로 시작" 진입. 단계:

1. **기본** — 프로젝트 이름(맵 파일명·빌드명·SPRP 제목 기본값), 맵 설명(선택), 저장 폴더.
2. **지형** — 타일셋(8), 가로/세로(64·96·128·192·256 프리셋 + 64–256 직접 입력), 초기 지형
   (선택한 타일셋의 브러시 목록).
3. **플레이어** — 인원수 1–8, 슬롯별 타입/종족/포스, 포스 수 1–4(각 포스 이름·플래그, 구성원
   요약), 빠른 구성 버튼(전원 한 포스 / 슬롯별 개별 포스 / 2팀 균등 — 배정을 한 번에 다시 쓰는
   편의 기능일 뿐 자유 배정을 제한하지 않음), 시작 위치 자동 배치 토글. 포스 수를 줄이면 사라진
   포스의 슬롯은 마지막 남은 포스로 옮겨진다.
4. **완료** — 생성 후 미리보기 썸네일, 요약(크기·타일셋·플레이어·출력 경로), "프로젝트 열기".

SC 경로 부재 시 1단계 진입 전에 안내 카드 + "StarCraft 폴더 선택" 버튼. 진행 중에는 트리거
비활성 + 같은 표면의 진행 표시. 모든 텍스트 한국어, Lucide 아이콘, 키보드 조작 가능.

## 5. 검증

- Rust: `normalize_relative` 대괄호 허용/거부 경계 테스트, `create_project_from_map` 출력 이름
  테스트, `setup_create_blank_project` 실패 시 폴더 보존 테스트(가짜 엔진), spec 검증 테스트.
- isom: 실제 SC 데이터 환경에서 8 타일셋 × 대표 크기 생성 → `chk_extract` → 헤더/플레이어/시작
  위치 일치. 데이터 없으면 기존 관례대로 ignore.
- 패널: Vitest로 시작 위치 프리셋 계산, 마법사 상태 전이, IPC 파서. TypeScript 빌드.
- 실제 acceptance: 마법사로 만든 프로젝트를 열고 `build_run` → `build/[EUD]<name>.scx` 생성 →
  생성된 맵을 SCMDraft 2로 열어 지형/플레이어/시작 위치가 그대로 보이는지 확인. Tauri 표면 검증.
- `hivemind/docs/architecture.md` "Panel and setup", `rules.md` "Native project paths"의 대괄호
  규칙 문장 갱신. `features/13_isom-ffi.md`에 `isom_map_new` 추가.

## 6. 범위 제외

- 에이전트 도구로서의 빈 맵 생성, 활성 프로젝트의 소스 맵 교체.
- 시작 위치 좌표 직접 입력, 대칭 지형 생성, 템플릿 맵.
- 기존 프로젝트 매니페스트의 출력 이름 마이그레이션.
