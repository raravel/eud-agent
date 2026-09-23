# 자연 에이전트 전환 — 구현 계획

Status: proposed (2026-09-23).

다음을 **대체**한다:

- `features/project-root-cwd-plan.md` §4 **D1** (쓰기는 저널 경유, 네이티브 `Edit`/`Write`는 "앞으로도 불가능하다")
- `features/staged-workflow-plan.md` 전체 (triage → research → plan → approve → execute → verify)
- `features/autonomous-agent-loop-plan.md` Goals 6 / Invariants 중 저널·리뷰 항목
- `rules.md`의 "Mutations require evidence and a project write registration", "Journal every accepted
  semantic mutation", "Reject/rollback applies inverse operations in reverse sequence"

## 1. 목표

에이전트(Codex / Claude Code / OpenCode Go / Ollama / Antigravity)가 **프로젝트 루트에서
자유롭게 파일을 읽고 쓰게** 한다. 저널·changeset 리뷰·역순 롤백 계약을 **git**으로 대체하고,
staged 워크플로 파이프라인을 제거해 에이전트가 한 컨텍스트에서 읽고·고치고·빌드하게 만든다.

검증 권위는 둘만 남는다: **`build_run`**(기계적) 과 **사람의 게임 플레이**(실제).

## 2. 무엇을 뒤집는가

`project-root-cwd-plan.md` §4 D1 원문:

> **D1 — cwd = 프로젝트 루트, 읽기는 전체 / 쓰기는 계속 저널 경유**
> 이유: "루트에서 자유 CRUD"를 파일시스템 쓰기로 확장하면 changeset Reject/Undo, write lease
> 직렬화, 저널 롤백이 전부 성립하지 않는다.
> **포기하는 것:** CLI 네이티브 `Edit`/`Write`로 소스를 직접 수정하는 경로. 앞으로도 불가능하다.

D1의 논리는 옳았다 — **단 "changeset Reject/Undo·저널 롤백을 유지한다"를 전제했을 때만** 옳다.
그 전제를 버리고 git을 되돌리기 권위로 세우면 결론이 뒤집힌다.

### 2.1 뒤집는 근거

| D1이 지키려던 것 | git으로 대체 가능한가 |
|---|---|
| changeset Reject/Undo | `git revert` / `git checkout`. 에이전트 변경뿐 아니라 **사용자의 외부 편집(SCMDraft 등)까지** 추적하므로 더 넓다 |
| 저널 역순 롤백 (`journal.rs` 3,125줄) | 커밋 단위 되돌리기가 역연산 재생보다 단순하고 검증하기 쉽다 |
| write lease 직렬화 | **git과 무관한 별개 이유**(빌드 마커·Map 쓰기 배제)로 존치한다 (§4 N6) |

### 2.2 staged 파이프라인을 함께 걷는 근거

`staged-workflow-plan.md`가 도입 근거로 적은 문제 중 다수가 **"에이전트가 프로젝트를 자연스럽게
못 본다"의 증상**이다. 자유 CRUD가 원인을 제거한다.

| 적혀 있던 근거 | 전환 후 |
|---|---|
| "모델이 매 턴 `list_files`/`read_file`로 트리를 재발견한다" | 소멸 — 네이티브로 직접 본다 |
| "research는 서버가 강제하는 의식(evidence gate, `tools.rs:2337`)" | 소멸 — 알아서 조사한다 |
| "`[project state]`가 5줄뿐" | 소멸 — `[project map]` 주입 대부분 불필요 |
| "같은 컨텍스트가 자기 일을 자기가 승인한다" | **소멸하지 않지만, 독립 검증으로도 못 고친다** (§2.3) |

### 2.3 독립 검증을 두지 않는 이유

검증 스테이지가 판정할 수 있는 것은 "계획대로 코드가 바뀌었나"까지다. 이 프로젝트의 실제
리스크는 **빌드는 통과하지만 게임에서 동작하지 않는 것**이고, 그것은 사람이 스타크래프트에서
돌려봐야만 안다. `architecture.md`가 이미 "Gameplay remains required for the harness"로 못박았고,
런타임 trace harness를 모델 도구에서 뺀 것(`2f6673d`)도 같은 판단이었다.

따라서 빌드 성공과 사람 사이에 모델 런을 한 겹 더 두는 것은 **비용만 늘리고 실제 리스크를
줄이지 못한다.** verifier 스테이지는 제거한다.

**감수하는 것:** 빌드가 녹색인데 요청과 다른 변경을 에이전트가 "완료"라고 말할 수 있다.
완화 수단은 (a) 턴 커밋이 만드는 `git diff` — 사람이 한 화면에서 본다, (b) 어차피 사람이
게임에서 확인한다, (c) 되돌리기가 `git revert` 한 번이다.

## 3. 지상 사실

전환이 반드시 마주치는 현재 상태.

### 3.1 프로바이더별 네이티브 도구

| 프로바이더 | 현재 | 근거 |
|---|---|---|
| Codex | 루트 **읽기만**, 쓰기는 `.eud-agent/workspace/.tmp/**` | `codex_client.rs:906-907` |
| Claude Code | **전부 차단** (`--tools ""` + `--allowedTools mcp__eud-tools__*`) | `claude_client/adapter/request.rs:130-141`, 테스트 `adapter_tests.rs:246-248` |
| OpenCode Go / Ollama / Antigravity | **네이티브 도구 개념 없음** — 엔진이 준 디스크립터가 전부 | `provider_tool_loop/profile.rs` |

프롬프트(`engine.rs:53`)는 이미 "cwd는 프로젝트 루트다, 트리를 직접 읽어라"라고 지시한다.
Claude 세션에서 이 문장은 현재 **거짓**이다.

### 3.2 저널과 changeset의 실제 관계

`journal.rs`는 changeset의 **공급원**이다 — `WriteTool` → `ChangesetItem` 매핑이
`journal.rs:725-860`에 있다. 리뷰가 사라지면 이 매핑의 소비자가 사라진다.

**MapSafe는 별개다.** `mapsafe.rs`는 `{map, backup}` 자체 기록(`:310`, `:517`)과 중단된 Apply의
복구 경로(`:1080-1132`)를 독자적으로 갖는다. `JournalStore`에 의존하지 않는다.

`harness.rs`는 `accepted_entries: Vec<JournalEntry>`(`:132`)를 입력으로 받는 **수락 후** 작업이다.
도구를 쓰지 않고 인라인 컨텍스트만 사용한다(`:658`).

### 3.3 검증이 걸려 있는 위치

| 검증 | 현재 위치 | 자유 CRUD 이후 |
|---|---|---|
| DAT 값 범위 | `native_build.rs:934` `validate_numeric_override` (**빌드 시점**) | 그대로 유효 |
| DAT sparse `before` == 카탈로그 baseline | `native_project.rs:1081` `apply_dat_patch` (**쓰기 시점**) | **이동 필요** |
| 경로 규칙 (`[`/`]`, `.eps`/`.py`, `src/` 한정) | 도구 스키마 + `native_project` 쓰기 경로 | **이동 필요** |
| 동시 편집 3-way 머지 | `tool_exec.rs:3564-3584` → `workspace.rs:2358` | git dirty 확인으로 대체 |

### 3.4 baseline 스캔 한도

`workspace.rs:35-37` — `MAX_FILES 2048`, 파일당 1 MiB, 총 32 MiB. 루트 전체가 writable이 되면
이 스캔은 성립하지 않는다. git이 diff를 담당하면 **스캔 자체를 제거**할 수 있다(전환의 부수 이득).

## 4. 핵심 결정

### N1 — 프로젝트 루트 자유 CRUD

에이전트는 프로젝트 루트에서 자유롭게 생성·읽기·수정·삭제한다.

**Codex** — `codex_client.rs:907`의 쓰기 프로필을 `":workspace_roots"={"."="write"}`로 넓힌다.
`codex_client.rs:1702`의 `assert!(!implementation_profile.contains("\".\"=\"write\""))`를 제거한다.
읽기 프로필 `eud_workspace_read`는 read 유지(위임 런이 사라져도 구조적 읽기 전용 경로는 남긴다).

**Claude Code** — `--tools ""`를 걷고 `--allowedTools`에 `Read`,`Write`,`Edit`,`Glob`,`Grep`과
`mcp__eud-tools__*`를 함께 둔다. `adapter_tests.rs:246-248`의 고정 단언을 갱신한다.

**Bash/셸은 열지 않는다.** 자유 CRUD의 목적은 파일 작업이지 임의 프로세스 실행이 아니다.
빌드는 `build_run`이 유일 경로로 남는다(§N5).

### N2 — 자유 CRUD의 경계

루트 전체 write에는 예외가 있다.

**쓰기 금지 (유지)**

| 경로 | 이유 |
|---|---|
| `maps/**`, `references/**` | MapSafe의 백업·CHK 재추출 검증·SCMDraft 공유락이 무의미해진다. 바이너리라 `git diff`도 무력하다. Map 변경은 Map 세션 / `map_*` 도구 경로를 유지한다 |
| `.git/**` | 히스토리를 에이전트가 만지면 되돌리기 수단 자체가 사라진다 |

**쓰기 허용하되 커밋 제외** — `build/**`, `src/**/__epspy__/**`, `.eud-agent/state/**`

**쓰기 허용 + 검증 이동** — `src/**`, `dat/*.json`, `project.eap`, `.eud-agent/workspace/**` (§N4)

### N3 — git이 되돌리기 권위

- 프로젝트 open 시 루트가 git repo가 아니면 **앱이 `git init`** 한다.
- **턴 경계마다 자동 커밋**한다. 커밋 메시지는 요청 요약 + 세션/요청 id.
- 커밋 전 **dirty 확인**: 직전 커밋 이후 에이전트가 만지지 않은 경로가 바뀌어 있으면 사용자의
  외부 편집이므로 별도 커밋으로 분리하고 알린다(§3.3의 3-way 머지 대체).
- 앱이 생성하는 `.gitignore`: `build/`, `**/__epspy__/`, `.eud-agent/state/`, `compat/`
- **이미 git으로 관리 중인 프로젝트**: 사용자 히스토리를 오염시키지 않아야 한다. 최초 open 시
  1회 확인을 받고, 거부하면 자동 커밋 없이 동작한다(되돌리기는 사용자 책임).
- **git 미설치**: 자동 커밋 없이 동작하되 시작 시 1회 경고한다. 자유 CRUD는 막지 않는다.

### N4 — 검증을 쓰기 시점에서 load/build 경계로 이동

자유 CRUD는 도구 스키마 검증을 우회하므로, 같은 규칙을 경계에서 다시 본다.

1. **`build_run` 프리플라이트** — 경로 규칙(`[`/`]` 금지, `src/` 한정, `.eps`/`.py`),
   MainFile 존재, 소스/출력 맵 별칭 금지. 위반은 빌드 실패로 보고한다.
2. **DAT 로드 시점 검증** — `apply_dat_patch`(`native_project.rs:1081`)의 sparse `before` ==
   카탈로그 baseline 규칙을 `dat/*.json` 로드 경로로 옮긴다. 값 범위는 `native_build.rs:934`가
   이미 빌드에서 본다.
3. **`project.eap` 전체 검증** — open 시점과 `build_run` 직전에 schema v2 전체를 검증한다.
   MainFile 이동 시 매니페스트를 갱신해 주던 `file_rename`/`file_move`가 사라지므로,
   MainFile이 가리키는 파일이 없으면 명시적 실패로 보고한다.

`dat_patch`, `file_edit` 등 기존 도구는 **제거하지 않는다** — 다이렉트 프로바이더(§N7)와
배치 DAT 편집처럼 도구가 더 나은 경우가 남는다. 다만 저널/리뷰 부수효과만 걷어낸다.

### N5 — 검증은 `build_run` + 사람

- `build_run`이 유일한 기계적 빌드 권위로 남는다. 소스·플러그인·의존성 변경 후 같은 턴에
  빌드하는 규칙(`rules.md`)도 유지한다.
- 독립 검증 스테이지(verifier)는 **두지 않는다**(§2.3).
- 에이전트는 빌드 성공만으로 "게임에서 동작함"을 주장하지 않는다 — 프롬프트 규칙으로 명시하고,
  무엇을 사람이 확인해야 하는지 답변에 적게 한다.

### N6 — 존치 목록 (혼동 주의)

리뷰 계약처럼 보이지만 **다른 이유로 존재**하므로 남긴다.

| 대상 | 존치 이유 |
|---|---|
| **리비전 해시** (`native_project.rs:1099`) | 빌드 no-progress 판정, autonomous resume 검증, Python 의존성 토큰 바인딩, Map source follow, 멘션 stale 판정 — 20개 모듈이 소비한다 |
| **`ProjectWriteCoordinator`** | 빌드 마커 보유 중 Map/사운드 쓰기 배제, 세션 간 공유 트랜잭션 직렬화. 리뷰와 무관 |
| **MapSafe 전체** | 자체 백업·검증·롤백·공유락. `JournalStore`에 의존하지 않는다(§3.2) |
| **`build_run` 마커 / 신선한 출력 맵 요구** | 빌드 권위 |
| **`propose_plan` 도구** | 스테이지가 아니라, 에이전트가 필요하다고 판단할 때 스스로 계획을 제시하는 수단으로 남긴다 |

### N7 — 다이렉트 프로바이더 3종 (결정 필요)

OpenCode Go / Ollama / Antigravity는 네이티브 도구가 원천적으로 없다. 파이프라인이 사라져도
이 셋은 여전히 `read_file`/`source_search`/`file_write`만 쓴다.

**권고: `fs_*` 순수 IO 도구를 eud-tools에 추가한다.** 저널·리뷰·evidence 부수효과 없이
파일시스템만 다루는 `fs_read`/`fs_write`/`fs_edit`/`fs_glob`/`fs_grep`. §N2의 경계를 그대로 적용한다.

- 다섯 프로바이더가 동등해진다.
- `source_search`의 한계(대소문자 무시 리터럴 substring, glob 없음, 매 호출 전량 재읽기 —
  `tool_exec.rs:4024-4034`, `:4841-4861`)가 `fs_grep`/`fs_glob`으로 해소된다.
- Codex/Claude에게도 MCP 폴백이 된다.

**대안(비권고):** 비대칭 수용. 변경량은 적지만 프로바이더별 성능 격차가 고착된다.

### N8 — 워크스페이스 문서와 harness

- `.eud-agent/workspace/{specs,plans,decisions,worklog}`는 자유 CRUD 대상이 된다.
  "구현 턴에는 문서를 건드리지 말라"는 `WORKSPACE_GUIDE` 금지(`engine.rs:55-56`)를 걷는다.
- **harness 재배선**: 현재 트리거는 "changeset 수락", 입력은 `accepted_entries`(`harness.rs:132`).
  수락이 사라지므로 트리거를 **턴 커밋 성공**으로, 입력을 **해당 커밋의 diff**로 바꾼다.
  harness의 내부 동작(인라인 컨텍스트, 도구 없음, 스펙/메모리 갱신)은 바꾸지 않는다.
- harness 자체의 존폐는 이 계획의 범위 밖이다.

### N9 — 패널

- `WorkflowStrip`(파악→조사→계획→승인→실행→검증→검토), 계획 승인 카드, `workflow` 이벤트,
  재접속 후 계획 복원 로직을 제거한다.
- `ChangesetView`를 **git diff 뷰**로 대체한다. 턴 커밋 단위로 보고, 되돌리기 버튼은
  `git revert`를 호출한다.
- `changeset_decision` 명령(`ipc.rs:682-686`), `changeset` 이벤트(`ipc.rs:1047`),
  `ChangesetReview` 알림 채널(`ipc.rs:95`)을 제거한다.

## 5. 제거·변경 인벤토리

### 제거

| 대상 | 규모 |
|---|---|
| `workflow.rs` | 1,693줄 (전체) |
| `engine/workflow_stages.rs` | 1,275줄 (전체) |
| `engine/workflow_tests.rs` | 1,042줄 (전체) |
| `engine/delegation.rs` + `delegate_read` 도구 | 위임 런 자체가 사라짐 |
| `provider_tool_loop`의 `DelegatedToolProfile` | 소비자 소멸 |
| `journal.rs`의 changeset 공급 경로 | `:725-860` 매핑 + 역순 롤백 |
| `workspace.rs`의 baseline 스캔·`sync_documents`·`merge_concurrent_text` | §3.4 |
| `tools.rs`의 evidence gate | `:2337-2366` |
| 패널 changeset/workflow 표면 | §N9 |

### 변경

| 파일 | 변경 |
|---|---|
| `codex_client.rs` | 쓰기 프로필 `"."="write"`, 고정 테스트 제거 |
| `claude_client/adapter/request.rs` | `--tools`/`--allowedTools` |
| `engine.rs` | `WORKSPACE_GUIDE` 재작성, 스테이지 호출 제거, 완료 판정 단순화 |
| `tool_exec.rs` | 쓰기 도구에서 저널/리뷰 부수효과 분리, `requires_write_workspace` 의미 축소 |
| `native_build.rs` | 프리플라이트 추가 (§N4.1) |
| `native_project.rs` | DAT/매니페스트 검증을 로드 경계로 (§N4.2, §N4.3) |
| `harness.rs` | 트리거·입력 재배선 (§N8) |
| 신규 `git.rs` | init / 턴 커밋 / dirty 확인 / revert |

### 문서

`architecture.md`, `rules.md`, `project-root-cwd-plan.md`, `staged-workflow-plan.md`,
`staged-workflow-scenarios.md`, `autonomous-agent-loop-plan.md`, `05_agent-core.md`,
`06_changeset-review-panel.md`, `sessions.md`를 갱신한다.

## 6. 단계

각 Phase는 Rust 전체 스위트 + 패널 스위트 + TypeScript 빌드를 통과시킨 뒤 다음으로 간다.

| Phase | 내용 | 산출 |
|---|---|---|
| 0 | working tree의 `scoped` 라우트 작업 처리 (커밋 또는 폐기) — 이 계획이 전부 되돌린다 | 결정 기록 |
| 1 | `git.rs` + init/턴 커밋/dirty 확인/revert. 기존 계약은 그대로 둔 채 **병행 기록** | git 히스토리가 저널과 같은 사실을 담는지 대조 |
| 2 | §N4 검증 이동 (프리플라이트 / DAT 로드 / 매니페스트) | 도구 우회 편집이 빌드에서 잡히는 테스트 |
| 3 | §N1 자유 CRUD 개방 (Codex + Claude) | 네이티브 편집 → `build_run` 성공 실측 |
| 4 | staged 파이프라인 제거 (§5 제거 목록) + 패널 표면 정리 | 요청 → 작업 → 빌드가 한 컨텍스트에서 끝남 |
| 5 | 저널/changeset 리뷰 제거, harness 재배선 (§N8) | git diff 뷰로 대체 |
| 6 | §N7 `fs_*` 도구 (승인 시) | 다섯 프로바이더 동등 |
| 7 | 문서 갱신 | §5 문서 목록 |

### 진행

- **Phase 0 — 완료 (2026-09-23).** 기록 커밋 `f83cfff`.
- **Phase 1 — 백엔드 완료 (2026-09-23).** `src-tauri/src/git.rs`가 git 탐지, `prepare`
  (`git init` + `.gitignore` + 초기 커밋), 사전 동의 상태, dirty 조회, 외부 편집 커밋,
  턴 커밋, `revert`, `log`를 갖는다. `NativeProjectManager::activate_project`가 프로젝트
  open마다 `prepare`를 부르고, 엔진이 턴 시작에서 `commit_external_edits_before_turn`을,
  턴 종료와 자율 런 iteration 경계에서 `commit_boundary`를 부른다.
  - `git revert`에는 `--no-verify`가 없다(지원 git 버전에서 usage 오류). revert는 훅을 탄다.
  - 앱의 자동 커밋은 `--no-verify` + `commit.gpgsign=false`로 돈다. 턴마다 도는 커밋이
    테스트를 돌리는 훅이나 에이전트 없는 서명 키에서 멈추면 앱이 멈추기 때문이다.
  - 커밋은 항상 `-- .` 파스펙으로 프로젝트 루트에 한정한다. 상위 저장소 안의 프로젝트가
    형제 폴더를 커밋하지 않는다.
  - **남은 것:** 사전 존재 저장소의 동의 UI와 외부 편집 분리 알림은 패널 표면이라
    Phase 5(§N9)에서 붙인다. 그때까지 사전 존재 저장소는 자동 커밋하지 않는다(안전 기본값).
    턴 경계 커밋의 엔드투엔드 테스트도 턴 경로가 확정되는 Phase 4에서 쓴다 — 지금 쓰면
    Phase 4가 지우는 staged 파이프라인 위에 쓰게 된다.

Phase 1–2를 3보다 먼저 두는 이유: **되돌리기 수단과 경계 검증이 먼저 서 있어야** 자유 CRUD를
열어도 안전하다. 어느 Phase에서 중단해도 제품은 동작 가능한 상태로 남는다.

## 7. 검증

`rules.md`의 기존 검증 권위는 유지한다 — Rust 전체 스위트, 패널 전체 스위트, TypeScript 빌드,
패널 프로덕션 빌드, 실제 euddraft 대상 네이티브 빌드 수용.

전환 고유 수용 기준:

1. Codex가 네이티브 `Edit`로 `src/**.eps`를 고치고 `build_run`이 성공한다.
2. Claude Code가 네이티브 `Write`/`Grep`을 쓰고 같은 결과를 낸다.
3. `maps/**`·`.git/**` 네이티브 쓰기가 거부된다.
4. 도구를 우회해 `dat/*.json`에 잘못된 `before`를 써도 **빌드가 잡는다**.
5. `project.eap`의 MainFile을 직접 깨뜨리면 open 또는 빌드가 명시적으로 실패한다.
6. 턴 커밋이 생기고, `git revert`로 턴 이전 상태가 정확히 복원된다.
7. 사용자가 SCMDraft로 맵을 바꾼 상태에서 턴이 돌면 별도 커밋으로 분리되고 알림이 뜬다.
8. 이미 git repo인 프로젝트에서 사용자가 자동 커밋을 거부하면 히스토리가 변하지 않는다.
9. Map 세션의 candidate → Apply → Undo 경로가 전환 전과 동일하게 동작한다.
10. **게임 플레이 확인** — 전환 후 실제 요청 하나를 끝까지 수행하고 사람이 스타크래프트에서
    결과를 확인한다. 이것이 유일한 최종 수용 근거다.

## 8. 미결

해결된 항목은 결정과 날짜를 남긴다.

1. **§N7 `fs_*` 도구 추가 여부** — **결정 (2026-09-23): 추가한다.** 다섯 프로바이더를 동등하게
   만들고 `source_search`의 한계를 없앤다. Phase 6에서 구현한다.
2. **턴 커밋의 "턴" 경계** — **결정 (2026-09-23): 둘 다 커밋한다.** 포그라운드 턴 종료
   (`chat_turn`)와 자율 런의 iteration 경계(`checkpoint_autonomous_boundary`) 모두가 커밋
   지점이다. 긴 자율 런을 중간 지점으로 되돌릴 수 있어야 한다. squash는 하지 않는다 —
   히스토리 재작성은 사용자가 그 사이에 손댔을 때 위험하다.
3. **harness 존폐** — 이 계획은 재배선만 다룬다.
4. **자율 런 완료 판정** — verifier가 사라진 뒤 `autonomous_completion_blocker`(`engine.rs:633`)가
   "빌드 성공 + 에이전트 답변"만으로 완료를 인정할지, 사람 확인을 기다릴지. Phase 4에서 묻는다.
5. **Phase 0** — **완료 (2026-09-23):** working tree의 staged-workflow 작업을 기록 커밋
   `f83cfff`로 남겼다. `scoped` 라우트와 `interrupted_request` 보존이 한 덩어리라 함께 커밋했다.
   rules.md의 ISOM 결정성 문단과 isom/map/imageTool 변경은 별개 작업이라 미커밋으로 남겼다.
