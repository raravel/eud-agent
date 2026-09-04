# Native euddraft Cutover Freeze — 2026-08-26

> Freeze ID: `EUD-NATIVE-CUTOVER-2026-08-26`
>
> 이 문서는 구현 시작 전의 사용자 결정, 실험 결과, 목표 아키텍처와 완료 기준을
> 고정한다. **수정하지 않는다.** 이후 결정 변경은 새 decision 문서로만 supersede한다.

## 1. 사용자 확정 결정

2026-08-26 대화에서 다음을 명시적으로 확정했다.

1. 이번 작업 안에서 EUD Editor 3 runtime 의존성을 완전히 제거한다.
2. `../euddraft` 저장소는 분석 대상으로 사용하지만 수정하지 않는다. 안정화 adapter와
   native project/build 구현은 `eud-agent` 저장소에만 둔다.
3. Native 프로젝트의 canonical source는 `project.json` manifest, EPS 파일 트리, schema-backed
   sparse DAT JSON이다. SQLite는 canonical authoring format으로 사용하지 않는다.
4. 기존 EUD Editor 프로젝트는 새 프로젝트만 지원하거나 one-way import에 그치지 않고,
   **양방향 import/export**를 지원한다.
5. 이 문서와 지금까지의 benchmark/code 결과를 별도 commit으로 고정한 뒤, 해당 commit에서
   새 구현 worktree/branch를 만든다.
6. 최종 cutover는 compatibility fallback이나 dual runtime을 남기지 않는 clean cutover다.

## 2. 문제 정의

기존 v2는 standalone Tauri 앱이지만 project authority와 build frontend는 EUD Editor 3에
남아 있다.

```text
Codex
  -> eud-tools
  -> BridgeIo file IPC
  -> Lua bridge
  -> EUD Editor BindingManager / project model
  -> Editor-generated EDS and build assets
  -> euddraft
```

최종 목표는 다음이다.

```text
Codex
  -> batch reads + dat_patch(changes[])
  -> request-local typed changeset
  -> semantic review / journal
  -> NativeProjectStore
  -> deterministic EDS generator
  -> euddraft
  -> output SCX
```

EUD Editor process, Lua bridge, heartbeat/status files, editor-path bootstrap, editor launch,
BindingManager, editor build initiation은 최종 runtime에서 제거한다.

## 3. 고정된 benchmark 결과

모든 실험은 `gpt-5.6-sol`, 동일한 mixed fixture와 exact-state 검증을 사용했다.
실제 사용자 EUD 프로젝트는 변경하지 않았다.

### 3.1 Fixture 구성

| 변경 수 | DAT | XDAT | TBL | Requirements | Buttons |
|---:|---:|---:|---:|---:|---:|
| 50 | 35 | 5 | 5 | 3 | 2 |
| 200 | 140 | 20 | 20 | 10 | 10 |

### 3.2 50개 변경

| 방식 | 성공 | 중앙 시간 | 중앙 input | 중앙 output |
|---|---:|---:|---:|---:|
| 개별 MCP setters | 3/3 | 27.204s | 73,171 | 1,001 |
| schema-rich batch MCP | 3/3 | 51.987s | 73,464 | 2,058 |
| JSON 전체 파일 | 3/3 | 52.999s | 92,077 | 2,222 |
| Schema JSON patch | 3/3 | 62.474s | 77,242 | 2,632 |
| SQLite SQL patch | 3/3 | 78.925s | 154,172 | 3,104 |

### 3.3 200개 변경

| 방식 | 성공 | 중앙 시간 | 중앙 input | 중앙 output |
|---|---:|---:|---:|---:|
| schema-rich batch MCP | 3/3 | 38.497s | 89,521 | 1,040 |
| Schema JSON patch | 3/3 | 45.026s | 142,109 | 1,467 |
| 개별 MCP setters | 1/3 | 44.683s | 90,980 | 1,784 |
| JSON 전체 파일 | 3/3 | 60.751s | 126,750 | 2,269 |
| SQLite SQL patch | 3/3 | 90.615s | 264,000 | 2,604 |

개별 setter 200개는 두 번 `code-mode host closed its stdout`으로 부분 적용 후 실패했다.
Batch MCP는 getter 5회와 `dat_patch` 1회, 총 6회 MCP 호출로 200개를 3/3 처리했다.

### 3.4 Batch MCP 규모별 결과

| 변경 수 | 성공 | 중앙 시간 | 중앙 input | 중앙 output | MCP 호출 |
|---:|---:|---:|---:|---:|---:|
| 1 | 3/3 | 17.948s | 65,763 | 292 | 2 |
| 50 | 3/3 | 51.987s | 73,464 | 2,058 | 6 |
| 200 | 3/3 | 38.497s | 89,521 | 1,040 | 6 |

### 3.5 Benchmark 결론

- 모델은 작업 규모를 선판단하지 않는다.
- 모든 write는 하나의 closed `DatChange[]`로 누적한다.
- 모델-facing write surface는 `dat_patch(changes[])` 하나다.
- `before`는 request 시작 authoritative 값, `after`는 마지막 후보 값이다.
- 동일 target은 하나로 compact한다. `before == after`이면 제거한다.
- read-before-write, duplicate target, stale before, no-op, unknown field를 전체 commit 전에
  검증한다.
- JSON은 사용자 review artifact와 canonical sparse source에 사용한다.
- SQLite는 필요하면 catalog/cache로만 사용하며 AI authoring format으로 사용하지 않는다.

원시/정규화 결과는 다음에 고정한다.

```text
benchmark-results/dat-authoring/baseline-50.json
benchmark-results/dat-authoring/baseline-200.json
benchmark-results/dat-authoring/schema-json-50.json
benchmark-results/dat-authoring/schema-json-200.json
benchmark-results/dat-authoring/sqlite-sql-50.json
benchmark-results/dat-authoring/sqlite-sql-200.json
benchmark-results/dat-authoring/mcp-batch-1.json
benchmark-results/dat-authoring/mcp-batch-50.json
benchmark-results/dat-authoring/mcp-batch-200.json
benchmark-results/dat-authoring/comparison.json
```

## 4. Canonical Native 프로젝트

최소 layout:

```text
<project>/
  project.json
  src/
    main.eps
    ...
  dat/
    standard.json
    xdat.json
    tbl.json
    requirements.json
    buttons.json
  plugins/
    ...
  maps/
    source.scx
  build/
    generated.eds
    output.scx
```

`project.json`은 versioned closed schema다. 최소 필드:

```json
{
  "schemaVersion": 1,
  "sourceMap": "maps/source.scx",
  "outputMap": "build/output.scx",
  "mainFile": "src/main.eps",
  "plugins": []
}
```

DAT JSON은 stock baseline 전체 복사가 아니라 sparse override를 저장한다. Generated EDS와
output map은 source of truth가 아니다.

## 5. DatChange 불변 계약

```text
DatChange
  dat          {dat, objectId, field, before, after}
  xdat         {dat, objectId, field, before, after}
  tbl          {index, before, after}
  requirement  {dat, objectId, before, after}
  button       {setId, before, after}
```

- closed union, `additionalProperties=false`
- target당 한 entry
- batch 전체 stale-before validation
- batch 전체 성공 후 한 번만 commit
- review 전 canonical project 미변경
- reject는 canonical project 미변경
- accept는 journal과 canonical source를 하나의 transaction으로 갱신
- build는 accepted canonical revision만 사용

## 6. euddraft 통합 경계

`../euddraft`는 read-only external source다. eud-agent가 다음 adapter를 소유한다.

1. 현재 euddraft CLI/config/EDS/plugin contract를 source-verified하게 해석한다.
2. version/fingerprint를 검증한다.
3. Native 프로젝트에서 deterministic EDS를 생성한다.
4. euddraft subprocess를 bounded timeout, explicit cwd/stdin/stdout/stderr로 실행한다.
5. diagnostics를 Native EPS/DAT target으로 구조화한다.
6. output map freshness와 expected path를 검증한다.

`euddraft` 저장소나 배포물을 runtime에 암묵적으로 patch하지 않는다.

## 7. EUD Editor 양방향 호환

양방향 호환은 다음 두 경계를 의미한다.

### Import

```text
EUD Editor project (.e3s + referenced assets)
  -> Native manifest / EPS tree / sparse DAT / settings / plugins
```

### Export

```text
Native project
  -> EUD Editor가 다시 열 수 있는 .e3s + referenced assets
```

Import/export는 deterministic하고 손실을 숨기지 않는다. 지원하지 않는 GUIEps/GUIPy/
ClassicTrigger 구조가 있으면 명시적으로 거부하거나 exact opaque preservation을 제공한다.
Silent projection이나 lossy rewrite는 금지한다.

## 8. 구현 계획

### Phase A — contract discovery

- euddraft entrypoint, config parser, EDS grammar, plugin loading, path semantics, encoding,
  diagnostics, output generation을 source에서 고정한다.
- 기존 BridgeIo/BindingManager/EDS build callsite와 project authority를 전부 목록화한다.

### Phase B — Native project core

- versioned manifest와 confined path resolver
- EPS source tree CRUD와 MainFile
- sparse DAT/XDAT/TBL/requirements/buttons store
- stock baseline/version metadata
- batch `dat_get` + atomic `dat_patch`
- semantic changeset/journal/rollback

### Phase C — compatibility

- `.e3s` parser/writer 또는 source-verified compatible adapter
- import/export exact roundtrip fixture
- opaque unsupported section preservation
- settings/plugin/MainFile/file-tree parity

### Phase D — build frontend

- deterministic EDS generator
- direct euddraft runner
- structured diagnostics
- output freshness/hash checks
- Editor build와 representative parity corpus 비교

### Phase E — product cutover

- setup에서 Native project open/create/import/export 제공
- panel status와 tools가 NativeProjectService만 사용
- Editor launch/path/heartbeat/bridge install 제거
- Lua bridge와 BridgeIo runtime 제거
- obsolete commands/tests/docs clean cutover

### Phase F — verification

- unit: schemas, paths, atomicity, stale/duplicate/no-op, defaults
- integration: 1/50/200 batch patch, rollback, restart persistence
- compatibility: import->export and export->import roundtrip
- build: actual euddraft success/failure fixtures and real project
- runtime: produced SCX and crash-critical DAT/button/requirement behavior

## 9. 최종 완료 조건

다음을 모두 만족하기 전에는 EUD Editor runtime을 제거한 것으로 간주하지 않는다.

1. Editor 없이 project create/open/save/build 가능
2. standard DAT/XDAT/TBL/requirements/buttons/default/reset 지원
3. EPS tree/MainFile/settings/plugins 지원
4. `.e3s` 양방향 import/export가 declared support 범위에서 lossless
5. Native EDS -> euddraft 실제 build 성공
6. build diagnostics가 source target에 매핑
7. 1/50/200 batch patch exact-state 통과
8. accept/reject/rollback/restart persistence 통과
9. core/runtime build path의 `BridgeIo`와 Editor heartbeat 의존 0개
10. editor bootstrap/launch/install/runtime code와 obsolete docs/tests 제거

## 10. 변경 금지

이 freeze 이후 구현 중 편의상 다음 범위를 축소하지 않는다.

- clean cutover를 dual runtime으로 변경
- euddraft 저장소 직접 수정
- manifest+sparse JSON을 SQLite/EDS canonical로 변경
- 양방향 호환을 one-way import로 축소
- 200개 batch patch를 개별 setter 반복으로 되돌림

변경이 필요하면 사용자 결정과 새 immutable decision 문서가 먼저다.
