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

- **Phase 2 — 완료 (2026-09-23).** §N4의 세 항목을 실제 코드에 맞춰 확인하고 빠진 것만 채웠다.
  - §N4.1 경로 규칙·MainFile 존재·소스/출력 맵 별칭, §N4.3 `project.eap` 전체 검증은
    **이미 `NativeProject::open`이 하고 있었다.** `build_with_cancellation`이 매번 `open`을
    부르므로 빌드 경계에서도 이미 걸린다. 새로 만들 필요가 없었다.
  - 실제로 비어 있던 것은 둘이다. (a) 스파스 DAT `before` == 카탈로그 원본 검사는 `dat_patch`
    쓰기 시점에만 있었다. (b) `src/` 아래에 프로젝트가 쓸 수 없는 이름(`[`/`]` 등)의
    `.eps`/`.py`가 생기면 모든 소스 목록이 **조용히 건너뛰었다**.
  - `NativeProjectManager::build_preflight`가 빌드 시작 전에 둘 다 본다. 위반은 `Err`가 아니라
    `ok: false` + `source: "preflight"` 진단을 단 `NativeBuildResult`로 돌려준다. 모델이
    컴파일 오류와 같은 모양으로 읽고, 같은 위반이 반복되면 no-progress 판정에도 걸린다.
  - **계획과 다른 점:** §N4.2는 "`dat/*.json` 로드 경로로 옮긴다"였지만 로드 경로
    (`NativeProject::open`)에는 `DatCatalog`가 없다 — 카탈로그는 compat 자산 루트를 받는
    빌드 쪽 물건이다. 그래서 로드가 아니라 **빌드 경계**에 뒀다. `dat_patch`의 쓰기 시점
    검사는 §N4의 "기존 도구는 제거하지 않는다"에 따라 그대로 남겼다(즉시 피드백).

- **Phase 3 — 코드 완료 (2026-09-23), 실사용 확인 대기.**
  - **Codex** (`codex_client.rs`): `eud_workspace_write`의 `:workspace_roots`를
    `{"."="write", "maps"/"maps/**"="read", "references"/"references/**"="read",
    ".git"/".git/**"="read", ".claude"/".claude/**"="read", ".mcp.json"="read"}`로.
    `codex sandbox -P`로 **실측 확인**(codex-cli 0.156.1): `src/`·`dat/`·`project.eap` 쓰기 성공,
    `maps/`·`references/`·`.claude/` 쓰기와 `maps/` 재귀 삭제는 `UnauthorizedAccessException`.
    더 구체적인 항목이 `"."`를 덮는다는 것도 같은 probe로 확인했다.
  - **Claude Code** (`adapter/request.rs`): 인터랙티브 턴만 `--tools ""`를 걷고
    `--tools "Read,Edit,Write,Glob,Grep"` + `--allowedTools "…,mcp__eud-tools__*"` +
    `--disallowedTools "Edit(maps/**) Edit(references/**) Edit(.git/**) Edit(.claude/**)
    Edit(.mcp.json)"`. `Bash`는 **열지 않는다**(§N1). compaction과 structured job은
    `--tools ""` 그대로다 — 도구가 있으면 안 되는 경로다.
    경로 규칙은 `Edit(...)`로만 쓴다: Claude Code는 `Write(...)` 경로 규칙을 받아들이지만
    **참조하지 않고** 시작 시 경고한다(2.1.280 permissions 문서).
  - **계획에 없던 추가:** `.claude/**`와 `.mcp.json`도 쓰기 금지에 넣었다. 자유 CRUD 전에는
    `validate_workspace_boundary`가 턴 시작에서 둘의 존재를 거부하는 것으로 충분했지만,
    이제는 턴이 그 파일을 **만들 수** 있다. 세션이 자기 권한을 스스로 정하게 두지 않는다.
  - **알려진 한계 (실측):** Codex의 Windows 샌드박스는 **이미 존재하는** 경로만 막는다.
    `.claude/`가 없는 상태에서 Codex 턴이 `.claude/settings.json`을 새로 만드는 것은 막히지
    않았다(생긴 다음부터는 거부). 파급은 한정적이다 — Claude 쪽은 도구 단위 deny라 존재 여부와
    무관하게 막히고, Codex가 만든 `.claude/`는 다음 Claude 턴이 기존 경계 검사로 거부하며,
    턴 커밋에 남아 `git revert`로 되돌릴 수 있다. 권한 상승이 아니라 가시적인 고장이다.
  - **아직 안 한 것:** §7 수용 1~3(네이티브 편집 → `build_run` 성공, Claude `Write`/`Grep`,
    `maps/**`·`.git/**` 거부)은 **실제 앱 런이 필요하다.** `verify.md` 기준으로 Claude는
    로그아웃 상태라 Claude 경로는 지금 확인할 수 없다.

- **Phase 1이 드러낸 것 — Windows 공유 위반 (2026-09-23).** 턴/open마다 git 프로세스가 도는
  순간 `map_candidate`의 원자적 승격이 `os error 5`(액세스 거부)와 `os error 32`(공유 위반)로
  깨졌다. 측정: git 준비를 켜면 4/4 실패, 끄면 0/3 실패. 같은 시그니처가 `verify.md`에
  checkpoint41의 실사용 Map finalize/revert WATCH로 이미 적혀 있었다 — 테스트가 그 경합을
  재현하기 쉽게 만들었을 뿐, 새 결함이 아니다.
  - 원인은 소유권 충돌이 아니라 경합이다. 방금 쓴 파일을 백신·인덱서가 몇 밀리초 잡고 있으면
    Windows가 다음 open/rename/delete를 거부한다. 파일이 잘못된 것이 아니므로 기다리면 된다.
  - `memory::retry_transient`가 5/32/33을 20ms부터 배수 백오프로 7회까지 기다린다.
    `write_atomic_bytes`(앱의 모든 원자적 쓰기)와 `map_candidate::remove_if_exists`가 쓴다.
    마지막 시도는 원래 오류를 그대로 보고하므로 진짜 권한 문제는 여전히 실패한다.
  - 수정 후 같은 조합을 8회 반복해 8/8 통과.
  - 그 뒤 남은 실패는 **네이티브 엔진 안쪽**으로 옮겨갔다: `MapAgentCore.cpp`의
    `map.save(temporary, ...)`가 같은 경합으로 실패하며 `"map save failed before output
    promotion"`을 낸다. C++을 고치면 정적 라이브러리를 다시 빌드해야 하므로,
    `isom::mapedit`가 **그 정확한 메시지일 때만** 20ms부터 4회까지 다시 부른다. 배치는
    결정적이고 임시 파일은 실패 시 엔진이 지우므로, 재시도는 같은 연산을 다시 하는 것이지
    두 번 하는 것이 아니다. 다른 엔진 오류는 절대 재시도하지 않는다(테스트로 고정).

- **Phase 4 — 파이프라인 제거 (2026-09-23).**
  - 파일째 삭제: `workflow.rs`(1,701), `engine/workflow_stages.rs`(1,275), `engine/workflow_tests.rs`,
    `engine/delegation.rs`, `provider_runtime/runtime/delegated.rs`,
    `contract_tests/{delegated_runs,native_delegated}.rs`, `provider_tool_loop/profile.rs`.
  - 엔진에서 제거: triage/clarify/route-note, 계획 스테이지 경유, verify 루프, `workflow` 이벤트,
    세션의 `workflow`/`interrupted_workflow` durable 상태, `workflow_resume`/`workflow_restart`
    명령, `InterruptedRequest` 투영, 시작 시 `recover_interrupted_workflows`.
  - 도구에서 제거: `delegate_read`와 그 런타임(`DelegationState`, `is_delegated_child`,
    실행 경로의 `delegated` 플래그), `DelegatedRun{Kind,Request,Outcome}`,
    `DelegatedRunExecutor`, `DelegatedToolProfile`/`ToolProfile::Delegated`/`RunGate::delegated`와
    submission-only 라운드, `delegation` 패널 이벤트.
  - **evidence gate 제거.** `search_docs`를 쓰기 전에 강제하던 규칙은 "모델이 프로젝트를
    자연스럽게 못 본다"의 증상이었다. 자유 CRUD가 원인을 없앴으므로 게이트는 의식만 남는다.
  - 존치 확인: `propose_plan`과 계획 승인 경로, `prepared_workspace`(승인 경로가 쓴다 —
    `workflow_stages`에서 `engine.rs`로 옮겼다), 팀 핸드오프(`map_task_request`) 전체,
    구조화 작업(`StructuredJobExecutor`), 리비전 해시, `ProjectWriteCoordinator`, MapSafe.
  - **프롬프트 재작성.** `INTRO`에서 "journals every mutation"을, `WORKSPACE_GUIDE`에서
    "쓸 수 있는 경로는 `.tmp` 뿐"·"구현 턴에는 문서를 건드리지 말라"를 걷고 §N2 경계와
    손으로 `dat/*.json`을 고칠 때의 `before` 규칙을 넣었다. `[triage]` 섹션은
    `[completion]`으로 대체했다: **녹색 빌드는 컴파일이지 게임 동작이 아니다**,
    바뀐 턴은 사람이 게임에서 무엇을 확인해야 하는지 구체적으로 적고 끝낸다(§N5).
    `DELEGATION_GUIDE`도 사라졌다 — 없는 도구의 사용법이었다.
  - §8 미결 4는 **현행 유지**로 결정: `autonomous_completion_blocker`는 그대로
    "런타임 변경이 있으면 현재 리비전의 성공한 빌드"만 요구한다.

- **Phase 5 — 진행 중 (2026-09-23).**
  - **요청 changeset 리뷰 제거.** `changeset_decision`이 `settle_request`가 됐다. 사용자가
    Accept를 누를 때 하던 일(위키 원장 기록, 워크스페이스 문서 리비전 확정, harness 작업 예약,
    task-state 이벤트, 쓰기 등록 해제)을 턴이 끝날 때 스스로 한다. Reject와 부분 수락 계약은
    사라졌다 — 되돌리기는 `git revert`다.
  - `Phase::ChangesetReview` 제거. 실패한 턴은 리뷰로 되돌아가지 않고 오류로 끝난다.
    `ipc`의 `ChangesetDecisionRequest`/`DecisionIds`/`AllLiteral`/`RollbackResultEvent`와
    `changeset_decision` 커맨드, `ChangesetReview` 알림 채널도 함께 갔다.
  - 위키 원장은 `accepted_ledger_entries(changeset, journal, scope)` →
    `applied_ledger_entries(changeset, journal)`. 고를 것이 없으므로 `AcceptedScope`도 없다.
  - 사운드 가드는 정산 **앞**으로 옮겼다. 빌드 없이 들어온 사운드 임포트는 정산 자체를 거부한다.
  - **계획과 다른 점 (중요):** §5는 "`journal.rs`의 changeset 공급 경로" 제거를 적었지만,
    **harness 문서 리뷰가 같은 changeset 기계를 계속 쓴다**(`ipc::ChangesetEvent`,
    `ipc_changeset_item`, `journal.changeset`). 문자 그대로 지우면 harness가 깨진다.
    실제로 죽은 것은 **역순 롤백**(`JournalRollbackTarget`, `ChangesetDecision`,
    `JournalStore::decide`, `apply_inverse`)뿐이고 그것만 걷어낸다.
  - `git.rs`에 `commit_detail`을 추가했다: 커밋 하나의 메시지와 파일별 유니파이드 diff를
    파일당 64 KiB / 커밋당 256 KiB로 묶어 돌려준다. 바이너리(맵)는 바뀌었다고만 보고하고
    내용 비교는 하지 않는다 — 맵은 diff가 가장 할 말이 없는 파일이다.

- **Phase 5 마무리 — 검증한 것과 하지 않기로 한 것 (2026-09-23).**
  - **역순 롤백 제거 완료.** `journal.rs` 3,125 → 1,596줄. `JournalRollbackTarget`,
    `ChangesetDecision`, `JournalStore::decide`, `apply_inverse`와 역연산 헬퍼 전부.
    `tool_exec.rs`의 트레이트 구현도 함께(−280줄). changeset 매핑 자체는 harness 문서 리뷰가
    계속 쓰므로 남겼다. `rejected_entries`는 reject와 무관해졌으므로 `selected_by_ids`로 개명.
  - **Phase 1의 결함 수정.** 커밋 경계가 `chat_turn`과 자율 iteration에만 있었고 정작 파일을
    쓰는 `continue_pending_write`에는 없었다. 그대로면 쓰기 턴의 변경을 다음 턴이 "외부 편집"으로
    기록한다 — 에이전트 자기 작업을 사용자가 한 것처럼. 쓰기 턴이 자기 작업을 커밋한다.
  - **§N8 harness:** 트리거는 턴 커밋, 입력은 **저널 엔트리 유지 + 커밋 해시 추가**로 결정했다
    (2026-09-23 사용자). diff로 갈아끼우면 `classify_runtime_verification`이 쓰는 도구 종류
    신호를 잃고, 저널은 어차피 위키 원장·task_state provenance·harness 문서 리뷰 때문에
    남는다. `HarnessJob.turn_commit`이 생겼고 worklog가 `` commit `<sha>` ``를 인용한다.
  - **§3.4 baseline 스캔은 제거하지 않는다 — 전제가 코드와 다르다.** §3.4는 "루트 전체가
    writable이 되면 이 스캔이 성립하지 않는다"고 적었지만, 스캔 대상은
    `workspace_root = <project>/.eud-agent/workspace`이지 프로젝트 루트가 아니다(`workspace.rs`
    `PROJECT_WORKSPACE_DIR`). specs/plans/decisions/worklog만 도는 작은 트리이고,
    `WorkspaceTurnRecorder::finish`가 이걸로 "이 턴이 어떤 durable 문서를 건드렸나"를 알아내
    문서 리비전 메타데이터를 기록한다(rules.md가 요구하는 것). 지우려면 그 메타데이터를 git
    커밋에서 끌어오는 별도 설계가 필요하고, 계획도 이걸 "부수 이득"이라고만 적었다.

- **Phase 5 패널 (2026-09-23).** `ChangesetView`가 `GitHistoryView`로 바뀌었다: 턴 커밋 목록 →
  선택한 커밋의 파일별 diff → "되돌리기"(확인 대화 뒤 `git_revert`). diff는 색뿐 아니라
  `+`/`-` 거터로도 구분되므로 흑백·색각이상에서도 읽힌다. 바이너리와 한도 초과는 이유 문장으로
  대체된다. 기존 저장소 동의 대화(`GitConsentDialog`)와 외부 편집 알림도 붙었다.
  - `ChangesetView`는 지울 수 없었다 — **harness 문서 리뷰가 쓰고 있다.**
    `HarnessChangesetView`로 옮기고 per-item 결정 기계(harness는 쓰지 않았다)만 걷어냈다.
  - **알림 채널 하나를 되살렸다.** 요청 changeset을 지우면서 `ChangesetReview` 종류까지
    지웠더니, 남은 두 표면(harness 문서 리뷰, 팀 후보 ready)이 "계획 승인이 필요합니다"라는
    **거짓 문구**로 알림을 띄우게 됐다. `ReviewRequired`("검토할 변경이 있습니다")를 추가하고
    설정에도 자기 행을 줬다.
  - **미검증:** rules.md는 UI 변경을 실제 브라우저/Tauri 표면에서 확인하라고 요구하는데,
    이 화면들은 아직 `tsc` + Vitest 증거뿐이다. 실제 `git_state`/`git_log`를 상대로
    렌더링된 적이 없다.

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
