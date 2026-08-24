# Map Agent Cross-Map Region Import — Authoritative Implementation Plan

Status: implemented; verification incomplete, so this remains the authoritative implementation specification.

이 문서는 Map Agent에서 다른 `.scx`/`.scm` 맵을 읽기 전용으로 열고, 정확한 영역과 지원 레이어를 프로젝트 가져오기 팔레트에 고정한 뒤, 현재 candidate에 직접 배치하거나 구조화된 멘션으로 에이전트에 전달하는 기능의 단일 구현 권위다.

이 기능의 외부 맵 입력, source pinning, 프로젝트 팔레트, cross-map stamp, 멘션, 권한, IPC, UI, 검증 또는 수명주기에 관해 다른 문서와 충돌하면 구현 완료 전까지 이 문서가 우선한다. 이 변경을 위한 경쟁 계획을 새로 만들지 않는다. 구현 완료 후 관찰된 현재 동작은 `architecture.md`, `rules.md`, `tech-stack.md`, `verify.md`로 승격하고 이 문서는 historical implementation authority로 전환한다.

## 1. Goal

다음 전체 흐름을 제공한다.

```text
현재 Map Agent
  → 다른 맵에서 가져오기
  → 별도 읽기 전용 Map Importer 윈도우
  → SCX/SCM 선택
  → 미니맵·캔버스에서 영역과 레이어 선택
  → 프로젝트 가져오기 팔레트에 고정
  → 현재 Map Agent에서 직접 배치하거나 @멘션
  → 기존 request draft/candidate 검증
  → 사용자가 기존 전체 Apply 수행
```

완료 상태는 다음 두 경로가 같은 source resolver와 stamp compiler를 사용하는 것이다.

1. **사용자 직접 배치**: 가져온 영역 카드의 `배치`로 위치를 정하고 collision preview 뒤 candidate revision 하나를 만든다.
2. **에이전트 배치**: 가져온 영역을 구조화된 `importedStamp` 멘션으로 요청에 포함하고, 모델이 `map_stamp_preview`/`map_stamp_place`를 통해 request draft에 배치한다.

두 경로 모두 외부 원본 맵과 현재 저장된 대상 맵을 직접 변경하지 않는다. 현재 저장 맵의 교체는 기존 Map Agent 윈도우의 trusted user-only 전체 Apply만 수행한다.

## 2. Confirmed Product Decisions

1. **입력 형식**: 사용자는 `.scx` 또는 `.scm` 파일을 고른다.
2. **raw CHK 제외**: 별도로 추출한 `scenario.chk`는 직접 입력으로 지원하지 않는다. 앱이 SCX/SCM 내부의 `staredit\scenario.chk`를 추출한다.
3. **별도 윈도우**: 외부 맵은 다이얼로그나 현재 작업면 전환이 아니라 별도 읽기 전용 `Map Importer` WebView 윈도우에서 연다.
4. **지원 레이어**: terrain, units, buildings, doodads, sprites, locations를 선택식으로 지원한다.
5. **타일셋**: source와 destination의 타일셋이 같을 때만 배치를 허용한다. cross-tileset 지형 변환은 없다.
6. **맵 크기**: source와 destination 전체 크기는 달라도 된다. 선택 영역의 상대 geometry가 destination 안에 들어가면 배치할 수 있다.
7. **보관 범위**: 가져온 영역은 현재 Map session이 아니라 프로젝트 단위 가져오기 팔레트에 지속한다.
8. **고정 의미**: 원본 외부 파일의 이후 이동·삭제·수정에 영향받지 않도록 선택 당시의 source map bytes를 앱 내부 content-addressed blob으로 고정한다.
9. **복사/붙여넣기 UX**: 1차 UX는 `프로젝트 팔레트에 추가` → 현재 Map Agent의 `배치`다. OS clipboard payload 또는 다른 앱과의 `Ctrl+C`/`Ctrl+V` 상호운용은 범위 밖이다.
10. **권한 의미**: imported stamp는 복사 원본을 식별할 뿐 destination write authority를 만들지 않는다.

## 3. Ground Truth

### 3.1 Current Map authority

- Map Agent의 destination authority는 현재 저장된 `OpenMapName`뿐이다.
- `MapContextService::current`는 현재 EUD 프로젝트 안의 저장된 source path를 confinement하고 file/CHK SHA-256, tileset, dimensions를 계산한다.
- unsaved SCMDraft/editor memory는 authority가 아니다.
- Map Agent 모델은 request-owned draft만 수정한다.
- original Apply와 undo는 `map-agent` 윈도우의 trusted user command다.

외부 맵 기능은 이 계약을 대체하지 않는다. 외부 source는 읽기 전용 복사 재료이며 destination, baseline, candidate 또는 Apply target이 될 수 없다.

### 3.2 Existing exact stamp path

`src-tauri/src/map_stamp.rs::compile_stamp_placement`는 이미 별도 `source_map`과 `destination_map` 경로를 받으며 다음을 구현한다.

- exact MTXM/TILE capture
- 완전 포함된 unit/building/doodad/sprite/location capture
- destination collision classification
- merge/replace semantics
- free location slot accounting
- target/protect authority check
- typed `MapOperation` batch 생성
- no-ISOM exact copy

현재 호출자는 source와 destination을 같은 visible candidate/request draft 계열로 제한한다. 또한 compiler는 source, destination, authority dimensions가 모두 같아야 한다고 요구한다. cross-map import는 새 복사 엔진을 만들지 않고 이 경로의 source resolver와 dimension contract를 확장해야 한다.

### 3.3 Existing selection palette is not an import palette

현재 `selection-palette.json`의 `PersistentSelection`은 다음 의미를 가진다.

- 현재 candidate revision에 다시 bind됨
- target/reference/protect/anchor 역할을 가짐
- current candidate의 live content를 stamp source로 읽음
- protect selection은 destination authority 계산에 참여함

외부 영역은 이 저장소에 넣지 않는다. 외부 영역은 destination role이 없는 immutable source snapshot이며 별도 `import-palette.json`에 저장한다. imported entry를 current selection으로 위장하거나 target/protect로 해석하는 경로를 만들지 않는다.

### 3.4 Existing render UI

`MapCanvas`와 `MapMinimap`은 현재 `mapRender`를 직접 호출한다. Map Importer용 canvas/minimap 구현을 복제하지 않는다. 두 컴포넌트가 strict render source/callback을 받도록 좁게 리팩터링하고 selection, transform, minimap navigation, object overlay 로직을 공유한다.

## 4. Scope

### 4.1 In scope

- singleton `map-import` WebView window 생성·focus
- trusted user file picker로 SCX/SCM 선택
- bounded streaming copy와 SHA-256
- pinned source blob 저장과 reference-aware cleanup
- source CHK validation, metadata, minimap/crop render, object/location pages
- rectangle/free-mask selection과 set operations
- six-layer selection
- 프로젝트 import palette add/list/delete
- imported region thumbnail
- same-tileset compatibility 상태
- 서로 다른 source/destination dimensions
- direct placement preview/confirm
- imported stamp structured mention
- model-facing stamp source union
- request-bound source resolution
- candidate manifest metadata and attachment/path-free deterministic replay
- stale, corruption, project switch, source switch failure states
- focused Rust/panel tests and real SCX/SCM smoke
- 구현 후 canonical docs 업데이트

### 4.2 Out of scope

- raw `scenario.chk` 직접 열기
- cross-tileset terrain approximation or conversion
- trigger, briefing, switch, force, player controller, tech/upgrade, sound 복사
- 외부 MPQ custom asset 복사
- unsaved SCMDraft state
- 외부 source map 수정
- 모델이 file picker, arbitrary path, import create/delete/list를 호출하는 기능
- 현재 Map Agent 안의 side-by-side source/destination editor
- source map을 candidate/session baseline으로 여는 기능
- source map 전체를 current map으로 교체하는 기능
- OS clipboard serialization
- 자동 collision policy 선택
- imported stamp의 destination 권한 확대

## 5. Required Invariants

1. 현재 저장된 `OpenMapName`만 destination baseline/apply authority다.
2. 외부 source path는 trusted user picker에서만 들어온다. 자연어, chip label, model args 또는 arbitrary IPC string path를 source authority로 사용하지 않는다.
3. 모델-facing tool에는 picker, path, blob path, raw CHK, MTXM matrix를 노출하지 않는다.
4. source bytes는 선택 직후 앱 내부 blob으로 복사하고 hash 검증한 뒤에만 render/save/place에 사용한다.
5. original external SCX/SCM을 다시 읽어 placement를 수행하지 않는다.
6. imported stamp는 project id, source file SHA-256, source CHK SHA-256, tileset, source dimensions, canonical mask, layers에 bind된다.
7. source와 destination tileset이 다르면 preview 전에 fail closed한다.
8. source와 destination dimensions는 같을 필요가 없다. source mask validation과 destination placement validation은 서로 다른 dimensions를 사용한다.
9. destination coordinates와 changed cells 전체는 기존 `MapRequestAuthority`를 통과해야 한다.
10. target mention이 있으면 imported placement 전체가 target union 안에 있어야 한다.
11. persistent 또는 mentioned protect는 imported placement에도 동일하게 적용한다.
12. imported stamp mention은 source identity만 제공하며 target/reference/protect/anchor 역할을 갖지 않는다.
13. direct preview는 read-only다. confirm은 source hash, snapshot hash, destination revision, tileset, bounds, authority, collisions를 모두 다시 계산한다.
14. terrain은 exact MTXM/TILE 값으로 복사하고 ISOM correction을 실행하지 않는다.
15. object/location은 source mask에 complete footprint가 포함된 항목만 복사한다.
16. merge/replace/cancel은 기존 exact stamp semantics를 유지한다.
17. request가 실패·취소·미finalize되면 visible candidate와 두 원본 맵은 byte-for-byte 불변이다.
18. candidate revision이 publish된 뒤 replay와 Apply는 imported blob/path에 의존하지 않고 persisted typed operations만 사용한다.
19. import entry 삭제 또는 blob 손상은 미래 mention/direct placement를 stale/unavailable로 만들지만 이미 publish된 candidate replay를 손상시키지 않는다.
20. original Apply와 undo는 계속 `map-agent` window label 전용이다.

## 6. User Experience Contract

### 6.1 Open the importer

Map Agent toolbar에 `다른 맵에서 가져오기` 버튼을 추가한다.

- `map-import` 윈도우가 없으면 `/map-import.html`을 새로 연다.
- 이미 있으면 show/focus하고 새 윈도우를 만들지 않는다.
- 현재 project id와 destination `OpenMapName` probe를 backend에서 다시 해석한다.
- Map Agent candidate/session을 변경하지 않는다.
- project 또는 `OpenMapName`이 바뀌면 importer의 staged source는 저장 불가 상태가 되고 새 destination context로 다시 열어야 한다.

### 6.2 Importer layout

```text
Top toolbar
  source picker
  source display name / tileset / dimensions / short hashes
  current destination tileset compatibility

Left controls
  rectangle | free mask
  replace | add | subtract | invert | clear
  terrain | units | buildings | doodads | sprites | locations
  label
  selected cells / bounds
  project palette save

Center
  read-only source MapCanvas

Bottom or secondary pane
  source MapMinimap
  source layer visibility
  captured object/location counts
  errors and stale state
```

Map Importer에는 agent conversation, candidate original/candidate/diff controls, Apply, undo, image placement, target/reference/protect roles를 넣지 않는다.

### 6.3 Select and save

사용자는 source map에서 rectangle 또는 free mask를 만들고 레이어를 선택한다. `프로젝트 팔레트에 추가`를 누르면 backend가 source dimensions를 기준으로 rows를 canonicalize하고 captured counts를 계산한 뒤 immutable imported entry를 만든다.

저장 성공 후 importer는 같은 source에서 추가 영역을 계속 만들 수 있다. 열린 Map Agent에는 `map-import-palette-changed` event를 보내 현재 project가 일치할 때 목록을 즉시 갱신한다.

### 6.4 Current Map palette

`MapPalette`는 두 source 종류를 시각적으로 구분한다.

```text
영역 스탬프
  현재 후보 내용

가져온 영역
  외부 맵 고정 스냅샷
```

Imported card는 다음을 표시한다.

- label
- source display name
- source tileset
- selection width × height
- selected cell count
- selected layers
- bounded preview thumbnail
- `배치`
- `멘션 추가`
- `삭제`

호환 상태:

- tileset mismatch: disabled, retained
- stamp bounds larger than destination: disabled, retained
- blob missing/hash mismatch: unavailable, retained until user deletes or reimports
- compatible: direct place and mention enabled

### 6.5 Direct placement

`배치`는 기존 stamp ghost overlay와 `StampPlacementControls`를 재사용한다.

1. candidate view로 전환한다.
2. source mask shape을 destination에 ghost로 표시한다.
3. pointer drag, click, numeric x/y, keyboard delta로 top-left를 정한다.
4. settled position마다 bounded preview를 요청한다.
5. preview report가 최신 destination revision/source snapshot과 일치할 때만 confirm을 활성화한다.
6. object/location collision이 없으면 merge semantics로 바로 confirm할 수 있다.
7. collision이 있으면 merge/replace/cancel을 명시적으로 선택한다.
8. confirm은 candidate revision 하나를 publish하고 original destination map은 변경하지 않는다.

### 6.6 Agent mention

Imported card의 `멘션 추가`는 다음 opaque snapshot을 Map prompt에 넣는다.

```ts
{
  kind: "importedStamp",
  importId: string,
  snapshotHash: string,
}
```

Compact trusted context에는 placement에 필요한 bounded metadata만 포함한다.

```json
{
  "kind": "importedStamp",
  "importId": "...",
  "label": "언덕 입구",
  "sourceMap": "source-map.scx",
  "tileset": "jungle",
  "width": 20,
  "height": 20,
  "selectedCells": 400,
  "layers": ["terrain", "units", "buildings"]
}
```

다음은 포함하지 않는다.

- original filesystem path
- internal blob path
- raw CHK
- MTXM/TILE matrix
- complete object records
- picker or import-management permission

## 7. Source File and Storage Model

### 7.1 Storage roots

Persistent metadata와 큰 pinned map bytes를 분리한다.

```text
%appdata%\eud-agent\
  map_candidates\
    <project-id>\
      import-palette.json

%localappdata%\eud-agent\
  map_imports\
    blobs\
      <file-sha256>.map
      <temporary-id>.tmp
```

`DataDirs`에 `map_imports_dir()`를 추가하고 `ensure_dirs()`에서 생성한다. `import-palette.json`은 project-scoped durable metadata이며, blobs는 machine-local persistent bytes다.

### 7.2 Source staging

`map_import_source_pick`는 backend-owned picker로 경로를 얻고 다음 순서로 처리한다.

1. extension을 case-insensitive `.scx`/`.scm`으로 제한한다.
2. canonical path가 regular file인지 확인한다.
3. `MAX_IMPORT_MAP_BYTES = 256 * 1024 * 1024`(256 MiB)를 metadata에서 먼저 검사한다. 테스트와 canonical docs도 이 값을 사용한다. 무제한 전체 파일 읽기는 금지한다.
4. source를 staging temp로 스트리밍 복사하면서 file SHA-256을 계산한다.
5. temp를 flush한 뒤 copied length와 metadata limit를 다시 확인한다.
6. pinned temp에서 `isom::chk_extract`를 실행한다.
7. DIM, ERA, MTXM과 render/object snapshot 필수 구조를 검증한다.
8. CHK SHA-256, tileset, dimensions를 계산한다.
9. 현재 destination tileset과 비교한다.
10. content-addressed blob path로 same-directory atomic promote한다. 같은 hash blob이 있으면 bytes/hash 검증 후 dedupe한다.
11. original source path는 이후 render/save/place에 사용하지 않는다.

Frontend response:

```ts
interface MapImportSource {
  sourceId: string;
  displayName: string;
  fileSha256: string;
  chkSha256: string;
  tileset: Tileset;
  width: number;
  height: number;
  fileSize: number;
}
```

`sourceId`는 in-process staged source binding이다. path가 아니다. importer reload/process restart 후에는 source를 다시 선택해 새 imported entry를 만들 수 있지만, 이미 저장된 entries는 persistent blob hash로 직접 resolve된다.

### 7.3 Import palette schema

```json
{
  "schema": "eud-map-import-palette/1",
  "entries": {
    "<import-id>": {
      "id": "<import-id>",
      "label": "언덕 입구",
      "snapshotHash": "...",
      "sourceDisplayName": "source-map.scx",
      "sourceFileSha256": "...",
      "sourceChkSha256": "...",
      "sourceExtension": "scx",
      "sourceTileset": "jungle",
      "sourceWidth": 128,
      "sourceHeight": 128,
      "bounds": {
        "left": 10,
        "top": 20,
        "right": 30,
        "bottom": 40
      },
      "selectedCells": 400,
      "rows": [],
      "layers": ["terrain", "units", "buildings", "doodads", "sprites", "locations"],
      "createdAt": "..."
    }
  }
}
```

Rust type는 `deny_unknown_fields`를 사용한다. map key와 entry id가 다르면 library 전체를 invalid로 취급한다. writes는 temp + atomic replace를 사용한다.

### 7.4 Snapshot hash

`snapshotHash`는 deterministic canonical serialization으로 다음을 bind한다.

- schema/version discriminator
- source file SHA-256
- source CHK SHA-256
- source tileset
- source width/height
- canonical bounds/rows/selectedCells
- selected layers

Presentation-only label과 created timestamp는 snapshot hash에서 제외한다. 라벨을 바꾸더라도 content identity가 바뀌지 않는다. geometry, layers 또는 source bytes가 달라지면 새 snapshot hash가 필요하다.

### 7.5 Cleanup

- identical source file hashes share one blob.
- referenced blobs는 age-based candidate cleanup에서 제외한다.
- staging temp는 startup과 cancelled pick에서 정리한다.
- delete는 import entry를 먼저 atomic remove하고, 다른 project/entry 및 active request가 blob을 참조하지 않을 때만 blob을 GC 대상으로 만든다.
- active direct preview/model request가 참조한 blob은 request 종료 전 삭제하지 않는다.
- GC failure는 entry deletion을 되돌리지 않지만 로그로 남기고 다음 startup에 재시도한다.

## 8. Rust Domain Model

### 8.1 Imported entry

`src-tauri/src/map_import.rs`에 다음 책임을 둔다.

- `MapImportStore`
- `ImportedStampLibrary`
- `ImportedStamp`
- staged source bindings
- source/blob validation
- project library locks
- blob reference/GC
- source render/object/thumbnail resolution

`map_candidate.rs` 또는 `map_stamp.rs`에 filesystem picker와 import library persistence를 넣지 않는다.

### 8.2 Stamp source union

```rust
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum MapStampSourceRef {
    CandidateSelection {
        selection_id: String,
        snapshot_hash: String,
    },
    Imported {
        import_id: String,
        snapshot_hash: String,
    },
}
```

Direct IPC는 full snapshot hash를 전달한다. Model tool input은 current request의 validated mention binding과 compact id를 사용하되 runtime에서는 같은 `MapStampSourceRef`로 normalize한다.

### 8.3 Mention variant

기존 persisted `MapMentionSnapshot::Stamp`는 current candidate selection 의미로 유지한다. 과거 Map session log migration을 만들지 않는다. 새 variant를 추가한다.

```rust
ImportedStamp {
    import_id: String,
    snapshot_hash: String,
}
```

`CandidateStore::prepare_request`는 imported mention마다 다음을 검증한다.

- current project library entry exists
- entry id/key exact match
- snapshot hash exact match
- pinned blob exists
- blob file hash exact match
- extracted CHK hash exact match when first resolved into the request
- source dimensions/tileset match entry
- destination tileset compatible

검증 결과는 request-owned allowed imported source binding으로 저장한다. 자연어 또는 label은 import id를 만들거나 다른 entry로 확대할 수 없다.

## 9. Cross-Map Stamp Compiler

### 9.1 Dimension contract

현재 equality requirement를 다음으로 교체한다.

```text
parsed source dimensions == imported selection source dimensions
parsed destination dimensions == authority map dimensions
source tileset == destination tileset
selection rows are canonical within source dimensions
each shifted destination mask is bounded within destination dimensions
```

source와 destination의 전체 width/height equality는 요구하지 않는다.

### 9.2 Coordinate transform

`StampDestination { x, y }`는 기존처럼 source selection bounds의 destination top-left다.

각 source coordinate의 이동:

```text
dx = destination.x - selection.bounds.left
dy = destination.y - selection.bounds.top

source cell (sx, sy)
  → destination cell (sx + dx, sy + dy)
```

object pixel coordinates와 location pixel rectangles도 같은 tile delta를 적용한다. signed intermediate와 checked conversion을 사용하고 overflow/underflow는 fail closed한다.

### 9.3 Tileset contract

- source와 destination ERA/Tileset을 각각 추출한다.
- 다르면 `stamp source and destination tilesets do not match` 계열의 명확한 오류를 반환한다.
- catalog는 일치가 확인된 tileset 하나로 로드한다.
- cross-tileset tile id reuse, semantic ISOM, approximate visual mapping은 금지한다.

### 9.4 Layer semantics

- empty layer set은 기존 stamp와 동일하게 six supported layers 전체를 의미한다.
- terrain: source mask exact MTXM/TILE
- unit/building: complete source footprint only
- doodad: complete source footprint and existing overlay dedupe
- sprite: complete source footprint only
- location: complete source rectangle only; Anywhere excluded; free slot checks retained
- unsupported CHK sections/assets are never copied

### 9.5 Collision and authority

Existing semantics remain authoritative.

- terrain replacement is not an object collision
- merge preserves destination objects/locations and adds source copies
- replace deletes only fully-contained selected-layer destination items
- replace fails when a boundary-crossing destination item exists
- destination masks must be non-overlapping
- all operations are one all-or-nothing batch
- `MapRequestAuthority` checks changed cells/layers, target and protect
- preview returns counts only and never emits operations to React/model

## 10. Candidate Integration

`lib.rs`는 `MapImportStore`를 정확히 한 번 생성해 Tauri state로 manage하고 같은 clone을 `CandidateStore::new(data_dirs, imports.clone())`에 주입한다. 모든 clone은 하나의 `Arc` inner와 project/blob locks를 공유한다. 독립 store 인스턴스나 중복 lock domain은 금지한다.

다음 functions를 source union 기반으로 확장한다.

- `direct_stamp_preview`
- `draft_stamp_preview`
- `draft_stamp_place`
- `stamp_request_context`

Resolution:

```text
CandidateSelection
  → visible candidate map
  → current bound SelectionMask

Imported
  → current project ImportedStamp
  → verified content-addressed source blob
  → source-dimension SelectionMask
```

Destination:

```text
direct preview
  → visible current candidate

direct confirm/model request
  → request-owned draft
```

Direct confirm은 기존처럼 temporary direct request를 만들고 `draft_begin` → `draft_stamp_place` → `finalize` → `commit_request`를 사용한다. 새 direct mutation path를 만들지 않는다.

Revision manifest에는 non-authorizing import provenance를 기록한다.

```text
import id
source file SHA-256
source CHK SHA-256
import snapshot hash
selection dimensions
selected layers
```

Manifest replay authority는 resolved typed operations다. replay가 import blob을 다시 읽는 설계는 금지한다.

## 11. Tauri Commands and Window Trust

### 11.1 Window

- label: `map-import`
- URL: `map-import.html`
- title: `Map Importer`
- reusable singleton
- resizable
- dedicated `src-tauri/capabilities/map-import.json`
- minimal core window permissions only

query-string routing을 사용하지 않는다.

### 11.2 Commands

Authoritative command surface:

```text
map_agent_import_open
map_import_bootstrap
map_import_source_pick
map_import_source_render
map_import_source_objects
map_import_stamp_save
map_import_stamp_list
map_import_stamp_thumbnail
map_import_stamp_delete
```

Trust boundary:

- `map_agent_import_open`: `map-agent` label only
- `map_import_bootstrap`, `map_import_source_pick`, `map_import_source_render`, `map_import_source_objects`, `map_import_stamp_save`: `map-import` label only
- `map_import_stamp_list`, `map_import_stamp_thumbnail`, `map_import_stamp_delete`: `map-agent`와 `map-import` labels only
- direct placement는 기존 `map_agent_stamp_preview`/`map_agent_stamp_confirm`을 사용하며 `map-agent` label only
- original Apply/undo commands remain `map-agent` only
- none of the import management commands enter the Map MCP registry

File picker는 backend command가 Tauri dialog plugin을 호출한다. `map-import` frontend에 arbitrary filesystem read capability를 주지 않는다.

### 11.3 Binary render IPC

Source crop/minimap/thumbnail PNG는 기존 Map render처럼 raw binary Tauri IPC로 반환한다. base64 JSON을 사용하지 않는다. render command는 opaque `sourceId` 또는 `importId`와 bounded crop/layer/scale만 받는다.

## 12. Frontend Architecture

### 12.1 New entry

```text
panel/map-import.html
panel/src/map-import-main.tsx
panel/src/map/MapImportApp.tsx
panel/src/map/MapImportToolbar.tsx
panel/src/map/MapImportSelectionControls.tsx
panel/src/map/importProtocol.ts
```

`panel/vite.config.ts`의 Rollup inputs에 `mapImport`를 추가한다.

### 12.2 Shared render surface

`MapCanvas`와 `MapMinimap`은 hard-coded `mapRender` 대신 strict renderer를 받도록 변경한다.

```ts
interface MapRenderSource {
  render(command: MapCropRequest): Promise<Blob>;
}
```

Candidate renderer는 `map_agent_render`, import renderer는 `map_import_source_render`를 호출한다. 두 surface가 다음 로직을 공유해야 한다.

- canvas transform
- crop scheduling
- stale/out-of-order image rejection
- minimap fit/navigation
- rectangle/free-mask rasterization
- selection set operations
- layer visibility
- bounded object/location overlay

Importer 전용 복제 canvas/minimap을 만들지 않는다.

### 12.3 Protocol types

`mapProtocol.ts`에 current candidate/imported source를 억지로 섞지 않는다. shared geometry/layer types는 유지하고 import-specific IPC는 `importProtocol.ts`가 소유한다. Map Agent에서 필요한 `ImportedStamp`/`MapStampSourceRef`만 명시적으로 export한다.

### 12.4 Map Agent state

`MapAgentApp`은 다음을 추가한다.

- current project imported stamp list
- `map-import-palette-changed` listener
- imported direct placement state
- imported mention creation
- compatibility/stale refresh on bootstrap/session/source changes

기존 `DirectStampPlacement`는 `SavedSelection` 고정 대신 source union을 가진다. preview sequence, revision key, confirming, collision report stale checks는 그대로 유지한다.

## 13. Model Tool Contract

`map_stamp_preview`와 `map_stamp_place` input schema를 source union으로 변경한다.

Candidate source:

```json
{
  "source": {
    "kind": "candidateSelection",
    "selectionId": "selection-id"
  },
  "destinations": [{ "x": 40, "y": 20 }]
}
```

Imported source:

```json
{
  "source": {
    "kind": "imported",
    "importId": "import-id"
  },
  "destinations": [{ "x": 40, "y": 20 }]
}
```

`map_stamp_place`는 추가로 `collisionPolicy`를 요구한다. JSON Schema는 `oneOf`, required fields, `additionalProperties: false`를 사용한다. `selectionId`와 `importId`를 동시에 받는 loose optional schema는 금지한다.

Map system guide additions:

- imported stamp is a pinned external-map source snapshot
- imported mention identifies source but grants no destination permission
- exact copy uses stamp tools only
- never reconstruct imported terrain through render, tile catalog, semantic ISOM or expected-before probes
- preview after `map_draft_begin`
- use only current-request imported mentions
- never guess merge/replace when collision policy is not explicit
- imported path/blob/raw content is unavailable and must not be requested

## 14. Error and Stale Contract

| Condition | Required behavior |
|---|---|
| importer open during project switch | staged source remains read-only; save disabled; reopen destination context |
| destination `OpenMapName` changes | compatibility stale; save/place disabled |
| source extension unsupported | picker result rejected before copy |
| source oversized | rejected before allocation/full read |
| invalid MPQ/CHK | staged temp removed; no palette mutation |
| missing DIM/ERA/MTXM | source rejected |
| tileset mismatch | source may show metadata but cannot save/place; backend still rejects |
| source blob missing | imported entry unavailable |
| source blob hash mismatch | imported entry corrupt/unavailable |
| snapshot hash mismatch | direct operation or complete model request rejected |
| imported entry deleted after chip creation | complete request rejected before driver execution |
| destination revision changes after preview | confirm rejected; preview must rerun |
| placement outside map | preview rejected |
| target/protect conflict | preview reports counts; confirm/place rejected |
| insufficient location slots | preview reports required/available; confirm disabled |
| partial replacement collision | replace disabled and backend rejected |
| request failure/cancel | draft removed; visible candidate/source maps unchanged |

Rejected Map chat restores the unsent text, attachments and mentions through the existing path.

## 15. Implementation File Plan

### New files

- `src-tauri/src/map_import.rs` — source staging, pinned blobs, project import palette, render/object/thumbnail resolution, GC
- `src-tauri/capabilities/map-import.json` — minimal importer window capability
- `panel/map-import.html` — dedicated importer HTML entry
- `panel/src/map-import-main.tsx` — importer React mount
- `panel/src/map/MapImportApp.tsx` — importer orchestration
- `panel/src/map/MapImportToolbar.tsx` — source metadata/picker/compatibility
- `panel/src/map/MapImportSelectionControls.tsx` — label/layers/mask/save controls
- `panel/src/map/importProtocol.ts` — strict importer IPC types/wrappers
- focused tests matching project conventions

### Existing files with planned changes

- `src-tauri/src/config.rs` — `map_imports_dir`, ensure dirs, path tests
- `src-tauri/src/lib.rs` — module/service construction, commands, importer window integration
- `src-tauri/src/map_agent.rs` — open importer, imported direct preview/confirm/list hooks, compact mention
- `src-tauri/src/map_candidate.rs` — source union resolution and request binding
- `src-tauri/src/map_stamp.rs` — cross-dimension same-tileset compiler
- `src-tauri/src/map_model.rs` — imported mention/source types
- `src-tauri/src/tools.rs` — strict stamp source schemas
- `src-tauri/src/tool_exec.rs` — source union dispatch
- `src-tauri/src/engine.rs` — Map guide and compact imported context
- `panel/vite.config.ts` — importer entry
- `panel/src/map/MapCanvas.tsx` — injected render source
- `panel/src/map/MapMinimap.tsx` — injected render source
- `panel/src/map/MapToolbar.tsx` — importer button
- `panel/src/map/MapPalette.tsx` — imported palette section
- `panel/src/map/MapAgentApp.tsx` — list/event/direct placement/mention
- `panel/src/map/StampPlacementControls.tsx` — source label/type presentation
- `panel/src/map/mapProtocol.ts` — Map Agent imported stamp/direct source types
- `hivemind/docs/architecture.md`, `rules.md`, `tech-stack.md`, `verify.md` — update only after behavior is observed

## 16. Implementation Sequence

1. Add strict imported domain types, palette schema and `DataDirs` root.
2. Implement `MapImportStore` staging, streaming hash/copy, validation, blob dedupe, atomic metadata writes and cleanup tests.
3. Extend `compile_stamp_placement` for different source/destination dimensions and explicit tileset equality; preserve all existing same-map tests.
4. Add imported source resolution to `CandidateStore` direct/request paths and persist non-authorizing provenance.
5. Add imported mention validation and request-owned allowed source bindings.
6. Change stamp MCP schemas/execution to a strict source union and update Map system guidance.
7. Add trusted Tauri importer window and commands with binary render IPC.
8. Refactor shared canvas/minimap rendering without changing current Map Agent behavior.
9. Implement Map Importer UI and source selection/save flow.
10. Add Map Agent imported palette, event refresh, direct placement, mention and delete flow.
11. Run focused Rust/panel tests.
12. Run real same-tileset cross-map direct and agent smokes; verify both source maps remain unchanged before Apply.
13. Run trusted Apply/replay smoke and prove committed revision replay no longer needs the imported blob.
14. Update canonical runtime docs only after observed behavior matches this plan.

## 17. Test Contract

### 17.1 Rust import store

`map_import::tests` must cover:

- SCX/SCM case-insensitive allowlist
- unsupported extension refusal
- pre-copy size cap
- streaming file SHA-256 and copied length
- corrupt MPQ/CHK refusal and temp cleanup
- missing/truncated DIM/ERA/MTXM refusal
- CHK/file hash persistence
- same-hash blob dedupe
- project library isolation
- strict schema and unknown field refusal
- key/id mismatch refusal
- canonical rows and empty/out-of-bounds selection refusal
- atomic add/delete
- referenced blob retention
- unreferenced staging/blob cleanup
- missing/corrupt blob unavailable state
- no original/internal path in model projection

### 17.2 Rust cross-map stamp

`cargo test -p eud-agent map_stamp --lib` additions must cover:

- different source/destination dimensions with same tileset
- source selection canonicalization against source dimensions
- destination bounds against destination dimensions
- explicit tileset mismatch refusal
- exact terrain equality after relative translation
- all six layers
- complete-footprint source capture
- doodad overlay dedupe
- free location slot accounting
- merge and replace
- partial replacement refusal
- non-overlapping multi-destination rule
- target and protect conflicts
- no `TerrainIsomBrush`
- existing same-candidate behavior unchanged

### 17.3 Candidate and request authority

`map_candidate::tests` additions must cover:

- imported direct preview/confirm
- imported request mention validation
- unmentioned imported id refusal
- stale snapshot refusal
- project mismatch refusal
- source blob/file/CHK hash mismatch refusal
- destination revision stale refusal
- failed/cancelled request byte invariance
- committed revision deterministic replay after imported blob removal
- manifest provenance contains hashes but no original/blob path

### 17.4 Tool and prompt schema

- `map_model::tests`: strict `ImportedStamp` serde and unknown fields
- `tools::tests`: exact `source.oneOf`, no loose simultaneous ids, no picker/path fields
- Map tool registry: import management and original Apply absent
- engine prompt tests: imported exact-copy, request-only source, no reconstruction, no guessed collision policy

### 17.5 Panel tests

Add focused tests for:

- singleton importer open command
- source picker loading/error/compatibility states
- import renderer use in shared canvas/minimap
- rectangle/free-mask operations
- all six layer toggles
- canonical save payload
- multiple entries from one source
- palette event refresh
- imported card metadata/thumbnail
- incompatible/corrupt unavailable states
- direct placement preview sequence/revision freshness
- merge/replace/cancel controls
- imported mention payload
- delete confirmation and stale chip behavior
- current Map Agent render regressions
- keyboard accessibility and Korean labels
- zero horizontal overflow at 1280×800 and 1920×1080

Focused command must include `cd panel && npx vitest run src/map` after the new tests are added.

### 17.6 Actual Tauri/WebView2 smoke

1. Open a live saved destination `OpenMapName` in Map Agent.
2. Invoke `map_agent_import_open`; require exactly one page at `/map-import.html`, title `Map Importer`, mounted canvas/minimap, zero alerts and zero horizontal overflow.
3. Invoke it again; require the same window to focus.
4. Pick a real same-tileset SCX/SCM with different dimensions.
5. Require exact source name, tileset, dimensions and short hashes.
6. Select a non-empty mask and all six layers; save an imported entry.
7. Require the current Map Agent palette to update without session reload.
8. Direct-place the stamp; exercise drag/numeric/keyboard destination and collision policy.
9. Require one candidate revision, exact terrain at translated mask cells, expected captured objects/locations and no ISOM operation.
10. Require original external source and saved destination SHA-256 unchanged.
11. Mention the imported stamp in a Map request; require request-bound preview/place and live draft preview.
12. Finalize and require one published candidate revision.
13. Remove or move the original external file; require pinned import to remain deterministic.
14. Remove the pinned blob only after a committed revision; require revision replay to remain deterministic while future placement becomes unavailable.
15. Pick a different-tileset source; require clear refusal and no palette/candidate mutation.
16. Apply only through the trusted Map Agent toolbar; require existing backup, replay, verification and undo rails.

## 18. Acceptance Criteria

Implementation is complete only when all statements are true.

1. A user can open a separate reusable Map Importer window from Map Agent.
2. The importer accepts valid SCX/SCM and rejects raw/unsupported/corrupt inputs without mutation.
3. The importer renders the pinned source map and supports rectangle/free-mask selection.
4. The user can select any subset of the six supported layers.
5. An imported entry persists project-scoped and appears immediately in the current Map Agent palette.
6. The original external path is not needed after import and is never model-visible.
7. Same-tileset source and destination maps may have different dimensions.
8. Cross-tileset placement is impossible in UI and backend.
9. Direct placement uses existing candidate preview/confirm/revision rails.
10. Imported stamp mention enables exact agent placement without exposing raw tiles or paths.
11. Imported mention does not grant destination write authority.
12. target/protect, collision, free location slot and partial replacement rules remain enforced.
13. Both source maps remain byte-identical until trusted user Apply.
14. Failed/cancelled turns leave visible candidate and both source maps unchanged.
15. Published candidate replay and Apply do not depend on imported blobs or original paths.
16. Existing current-candidate saved selection stamps continue to behave unchanged.
17. Focused Rust/panel tests and actual Tauri/WebView2 smoke pass.
18. Canonical docs describe only the behavior actually observed after implementation.

## 19. Implementation and Verification Record (2026-08-24)

Implemented across Rust/Tauri and the panel without a competing plan. Observed passing:

- `cargo test -p eud-agent map_import --lib -j 1` — 13 passed, 2 ignored.
- `cargo test -p eud-agent map_stamp --lib -j 1` — 7 passed.
- `cargo test -p eud-agent map_candidate::tests --lib -j 1` — 11 passed, 3 ignored.
- `cargo test -p eud-agent tools::tests --lib -j 1` — 59 passed, 1 ignored.
- `cargo test -p eud-agent map_model::tests --lib -j 1` — 7 passed.
- Focused Map prompt, Map Agent, map_verify, and mapsafe regressions — 1, 6, 5, and 14 passed.
- Real installed-asset fixture replay: `imported_stamp_is_request_bound_and_replay_is_blob_independent`
  — passed.
- Live saved `OpenMapName` render — passed against `proj1.scx` (platform, 256×256).
- Real same-tileset/different-dimension stage/save and source/destination SHA-256 invariance —
  passed with `wf_platform_m0.scx`.
- Real same-tileset/different-dimension direct preview, request-bound agent placement, exact
  translated terrain, one revision, no ISOM, unchanged originals, and replay after import
  deletion/blob GC — passed.
- Real different-tileset refusal without palette/original mutation — passed with
  `map_agent_rich.scx`.
- `cd panel && npx vitest run src/map` — 19 files, 72 tests.
- `cd panel && npx tsc -b --noEmit`.
- `cd panel && npm run build` — emits `dist/map-import.html`.
- Mock-Tauri real-browser flows at 1280×800 and 1920×1080: importer source canvas/minimap,
  rectangle selection, six-layer canonical save, palette update; Map Agent importer command,
  imported card/thumbnail, direct numeric/keyboard preview-confirm, imported mention, delete/stale
  chip; zero horizontal overflow and zero unexpected dialogs/errors on successful flows.

Acceptance Criteria 17 and therefore the overall completion gate remain open for production
Tauri/WebView2 interaction: exact singleton/focus observation, native picker automation, and
trusted toolbar Apply/undo. A live editor and real maps were available for backend/native smoke,
but the browser driver could not attach to the WebView2 desktop process (`app.path` timed out), and
the already-running installed app must not be killed or instrumented.

The final workspace-wide clippy rerun is also blocked by concurrent out-of-scope MapSound work:
`SessionToolRuntime::restore_map_backup` still implements the previous three-argument method while
`JournalBridge::restore_map_backup` now requires `expected_sha256`. The focused feature tests and
an earlier `cargo build` passed before that unrelated cutover changed again. Do not convert this
file to historical implementation authority until the actual-window steps pass and the shared
workspace build is coherent.
