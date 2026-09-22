# Subagent delegation and EPS–Map team handoff plan

Status: Phase 0 implemented (2026-09-18); Phase 1 (`delegate_read`) and Phase 2 (`map_task_request`
team handoff) implemented 2026-09-20 with the deviations listed under "Implemented result"; Phase 3
proposed. 이 문서는 3단계(위임 읽기 run →
EPS↔Map 팀 핸드오프 → 병렬 읽기/리뷰 워커) 전체의 설계 권위이며, 구현 후에는 `architecture.md`, `rules.md`,
`features/05_agent-core.md`, `features/sessions.md`, `verify.md`가 동작 권위를 이어받는다.

관련 문서: [autonomous loop](autonomous-agent-loop-plan.md),
[provider runtime unification](provider-runtime-unification-plan.md),
[map agent workbench](map-agent-workbench-plan.md),
[candidate lifecycle](map-agent-candidate-lifecycle-and-mcp-schema-plan.md),
[resource mentions](resource-mentions-plan.md), [sessions](sessions.md).

## 1. 문제

현재 런타임은 "한 세션 = 한 워커 = 한 foreground run"이다. 이 구조에서 두 가지가 반복적으로 비싸다.

1. **컨텍스트 소모형 탐색.** `source_search`/`docs_get`/`map_objects_read`/`map_render` 같은 읽기
   결과가 전부 부모 transcript에 쌓인다. `IterationBoundaryReason::ContextPressure`와 compaction이
   있어도 탐색 자체가 부모 컨텍스트를 점유하는 사실은 바뀌지 않는다.
2. **EPS ↔ Map 교차 작업.** "이 지역에 로케이션을 만들고 유닛을 배치한 다음, 그 로케이션을 쓰는
   트리거를 작성해라"는 하나의 사용자 의도가 EPS 세션(트리거)과 Map 세션(배치)으로 갈라져 있어
   사용자가 두 창을 오가며 중계해야 한다. EPS 세션에는 `location_write`/`switch_write`/sound 도구만
   있고 terrain/units/buildings/doodads/sprites 배치 권한이 없다. Map 세션은 EPS 소스를 만지지 못한다.

기존 기반은 충분히 있다.

| 현재 요소 | 재사용 지점 |
|---|---|
| `StructuredJobExecutor` (`provider_runtime/runtime/structured.rs`) | 툴·스토리지·UI sink 없는 격리된 1회성 모델 호출. 자식 run 실행기의 원형 |
| `SessionKind::Eps`/`Map` + `tool_registry()`/`map_tool_registry()` | 역할별 툴 집합과 system prompt가 이미 분리됨 |
| `RunIdentity`, `RunGate`, run별 native MCP endpoint | 자식 run에 부모 request/취소 세대를 상속시키는 자리 |
| `ask_waiting` watch 채널 (`runtime/step.rs`, `runtime/tool_batch.rs`) | 부모 active deadline을 멈춘 채 외부 완료를 기다리는 검증된 메커니즘 (app37 native ASK) |
| `ProjectWriteCoordinator` | 세션 간 canonical 쓰기 직렬화 — 팀 동시성 규칙의 기존 권위 |
| `CandidateStore::save_selection`/`prepare_request` | EPS `MapRegion`/`MapLocation` 멘션과 같은 형태의 selection을 받는 Map 요청 입구 |
| `map_agent_candidate_apply`/`apply_undo`, `verify_current_for_apply` | 신뢰된 사용자 Apply, 저장된 원본 따라가기(`follow_source`), exact undo |
| actual39 C18 | Map 읽기와 OpenCode 읽기 병행이 세션 경계를 지키며 완료됨 |

## 2. 목표

1. 부모 run이 **read-only 자식 run**에 탐색을 위임하고 스키마 검증된 요약만 돌려받는다. 5개
   프로바이더에서 동일하게 동작한다.
2. EPS 세션이 **typed 핸드오프**로 Map 세션에 배치 작업을 요청하고, Map 세션은 평소처럼 후보 `rN`을
   만들며, 사용자가 Map 창에서 Apply/폐기한 결과를 EPS run이 받아 이어간다.
3. 하나의 사용자 요청에서 생긴 EPS changeset과 Map apply를 **하나의 팀 작업 ID**로 묶어 리뷰
   패널에서 함께 보이게 한다.
4. 읽기 자식 run을 제한된 수로 **병렬** 실행하고, 완료 전 **위임 리뷰**를 선택적으로 요구할 수 있다.
5. 기존 exactly-once 툴 완료, semantic journal, 리뷰, ASK, 취소, provider continuation, stale
   authority 보호를 그대로 유지한다.

## 3. 비목표

- 모델의 원본 Map Apply. 팀이 되어도 Apply/Undo는 Map 창의 신뢰된 사용자 동작이다.
- EPS 쓰기 워커의 병렬화. `ProjectWriteCoordinator`가 canonical 쓰기를 직렬화하므로 이득이 없다.
- 자식 run이 부모의 write registration을 공유하거나 canonical 파일을 쓰는 것.
- Codex/Claude CLI 내부 서브에이전트 기능에 의존하는 것. 프로바이더별로 동작이 달라진다.
- Map → EPS 방향 핸드오프. 이 계획에서 리더는 항상 EPS 세션이다.
- EPS changeset reject 시 연결된 Map apply의 **자동** undo. 완료된 map 변경의 자동 롤백은 기존
  "never automatically replay or roll back" 경계를 깬다.
- 자식 run의 restart 후 재개. 자식은 항상 비재개(`allow_resume: false`)이다.
- 프로바이더가 보고하지 않는 토큰/비용의 추정.

## 4. 불변 조건

- `project.eap`, `src/**/*.{eps,py}`, `dat/*.json`, source map만이 authoring 권위다. 자식 run과 팀
  작업 기록은 모두 파생 상태다.
- canonical 변경은 그 변경을 수행한 **세션의** request가 write registration을 갖고, 그 세션의
  journal에 기록된다. 팀 작업은 두 세션의 journal을 **연결**할 뿐 병합하지 않는다.
- 자식 run은 부모 `session_id`/`request_id`/`cancellation_generation`을 상속하고 고유 `run_id`를
  갖는다. 부모 취소는 자식 admission을 즉시 닫는다.
- 자식 run이 읽은 결과는 부모 mutation rail의 evidence가 아니다. 부모는 편집 전에 대상을 직접
  다시 읽는다 (기존 "resumed write run re-reads the target" 규칙과 동일).
- 하나의 툴 호출 안에서 기다리는 시간은 **240초를 넘지 않는다** (native CLI의 300초 MCP 호출 한도
  아래). ASK, 위임, 팀 후보 대기 모두 이 상한을 공유하며, 더 긴 기다림은 턴을 끝내고 다음 사용자
  메시지로 이어진다. 대기 중 부모 active deadline은 지금의 ASK처럼 멈춘다.
- 팀 Map 세션의 candidate도 저장된 원본을 따라간다(`follow_source`): 사용자 Map 창이나 맵 속성이
  원본을 바꾸면 팀 후보는 새 원본 위로 rebase되고 announced revision 번호는 유지된다. EPS 쪽 map
  도구와 Map 후보가 같은 레이어를 겹쳐 쓰지 않도록 팀 작업 중 상호 배제한다.
- Startup은 아무것도 재개하지 않는다. 진행 중이던 팀 작업은 `interrupted`로 매핑되고 후보는 Map
  창에서 여전히 사용자 검토가 가능하다.

## 5. 런타임 계층

```text
user request R (session S = EPS, journal authority)
├── foreground iteration N            run identity (S, R, run_n)
│   ├── delegated read run D1         run identity (S, R, run_d1, parent = run_n)   ← Phase 1
│   ├── delegated read run D2         병렬, 최대 3                                  ← Phase 3
│   └── tool: map_task_request ──────▶ team task T                                ← Phase 2
│                                       └── Map session M (S의 팀 세션), request R'
│                                           ├── draft → finalize → candidate rN
│                                           └── 사용자 Apply/폐기 → T 상태 전이 → S run 재개
└── changeset review (S) ◀─ 연결 ─▶ Map apply record (M)                          ← Phase 2
```

- **자식 run**은 부모 request 안에서 산다. 자체 `RunGate`, 자체 native MCP endpoint, 자체
  transcript를 갖고 종료 시 결과 값 하나만 부모 tool result로 남긴다.
- **팀 작업**은 부모 request가 소유하는 durable 레코드이고, 실제 실행은 별도 Map 세션의 정규
  `map_chat` 요청이다. Map 세션 입장에서는 "누가 프롬프트를 썼는가"만 다를 뿐 기존 후보 수명
  주기와 동일하다.

## Phase 0: ASK 타임아웃과 텍스트 전환

구현 완료(2026-09-18): `tools::ASK_WAIT_TIMEOUT`, `SessionToolRuntime::ask_scoped`의 bounded
wait/만료 기록, `AskEvent.status`/`waitSeconds`, `AutonomousPauseReason::UnansweredAsk`, 패널
`askExpired`와 `AskCard` 남은 시간 표시. 아래 계약이 동작 권위이며 검증은 `verify.md`에 있다.

실측(2026-09-17)에서 Claude/Codex CLI는 침묵하는 MCP 호출을 300초에 끊는다. 지금의 `ask`는 답이 올
때까지 무한 대기하므로 native 프로바이더에서는 5분 뒤 CLI가 툴 실패를 만들고 모델이 답 없이
진행한다. eud-agent 쪽 무한 대기 의도 대신, 다른 에이전트와 같은 계약으로 바꾼다.

### 계약

- `ASK_WAIT_TIMEOUT = 240s`. 모든 프로바이더에 동일하게 적용한다(direct 프로바이더도 같은 동작을
  가져야 모델 프롬프트가 하나로 유지된다).
- `ask` 호출 → 지금처럼 `AskEvent`를 내보내고 `AskCard`를 표시한다. 240초 안에 `ask_response`가
  오면 지금과 같은 `{ "answers": {...} }`를 반환한다.
- 240초가 지나면 pending ASK를 제거하고(`ask_waiting` 해제, 세션 활동 `Idle`이 아닌 `RunningRead`
  복귀) 툴은 다음을 반환한다:

```json
{ "status": "unanswered", "questionIds": ["q1", "q2"], "waitedSeconds": 240 }
```

- 툴 설명과 system prompt는 `unanswered`를 받은 모델이 **같은 질문을 최종 텍스트 답변으로 다시
  쓰고 턴을 끝내도록** 지시한다. 사용자의 다음 메시지가 답이며 정규 새 turn으로 들어온다. 모델은
  `unanswered` 뒤에 같은 turn에서 `ask`를 다시 호출할 수 없다(usage error).
- 만료된 `request_id`로 온 `ask_response`는 명시적 오류(`ask_expired`)를 돌려주고 패널은 카드를
  닫는다. 답변과 만료가 경합하면 oneshot에 먼저 도착한 쪽이 이긴다.
- 패널: 만료 시 `ask` 이벤트 `{ requestId, status: "expired" }`로 카드를 닫고, 대화 로그에 "질문에
  대한 답을 기다리는 시간이 지나 텍스트로 이어집니다"를 남긴다. 카드에는 남은 시간을 표시한다.
- Autonomous: ASK 시작 시 `waiting_input`, 만료 시 `running`으로 복귀한다. 만료 뒤 turn이
  `Completed`로 끝나면 run은 완료/리뷰 판정 대신 `paused`(pause reason `unanswered_ask`, blocker
  "계속을 누르면 AI가 같은 질문을 다시 합니다")로 pause한다. 패널은 paused 상태에서 입력을 막으므로
  답은 이 turn에 직접 들어가지 않는다: 사용자가 계속을 누르면 explicit resume이 blocker를
  continuation 프롬프트에 실어 보내고, 모델은 같은 질문을 다시 `ask`한다(만료 표시는 foreground run
  단위이며 새 run마다 지워진다). 리뷰 정산은 이미 있는 `unanswered_ask` 이유를 `user`로 덮어쓰지
  않는다.
- 재접속 시 pending ASK 복원은 남은 시간과 함께 유지된다(`pending_ask()`가 시작 시각 기준 잔여
  초를 보고하고 패널이 그 값으로 카운트다운을 시작한다). restart 뒤에는 run이 없으므로 복원할 ASK도
  없다(현행과 같음).

### Acceptance

- 답이 240초 안에 오면 기존 테스트가 그대로 통과한다.
- 240초 경과 → `unanswered` 결과, pending 제거, `ask_waiting=false`, 자율 상태 `running`;
  autonomous turn이 그 뒤 `Completed`로 끝나면 `paused`/`unanswered_ask`.
- 만료 후 같은 foreground run의 재`ask`는 usage error(새 run — continuation, plan feedback, write
  transition — 은 다시 허용); 만료된 id의 `ask_response`는 `ask_expired`.
- 답변/만료 경합에서 이중 완료가 없다(툴 결과는 정확히 하나).
- native 어댑터 fixture: 250초 침묵 ASK가 CLI 중단 없이 `unanswered`로 완료된다(fixture는 시간을
  주입해 실제로 기다리지 않는다).
- 패널: 카드 만료 표시, 만료 로그, 새 turn 전송이 Vitest와 실제 UI에서 확인된다.

## Phase 1: Delegated read-only run

### 요청 계약

`provider_runtime/requests.rs`에 foreground와 구분되는 위임 요청을 추가한다.

```rust
pub struct DelegatedReadRequest {
    pub identity: RunIdentity,          // parent와 같은 session/request/cancellation_generation, 새 run_id
    pub parent_run_id: RunId,
    pub binding: BindingSnapshot,       // parent 세션 binding, conversation = empty
    pub goal: String,
    pub focus: Vec<String>,             // 선택: 경로/객체 힌트
    pub tool_descriptors: Arc<[Value]>, // 아래 read 집합
    pub output_schema: Value,           // DELEGATED_READ_RESULT_SCHEMA
    pub policy: RunPolicy,              // active_deadline 240s(툴 호출 상한), max_tool_rounds 16, allow_resume false
}
```

`RunIdentity`에 `parent_run_id: Option<RunId>`를 추가한다. `SessionToolRuntime::matches_run_scope`는
request/취소 세대만 보므로 자식 dispatch는 그대로 통과하고, gate의 stale-run 검사는 자식 자신의
identity로 이루어진다.

결과 스키마는 고정한다.

```json
{
  "summary": "string",
  "findings": [{ "path": "string?", "line": "integer?", "excerpt": "string?", "note": "string" }],
  "openQuestions": ["string"],
  "toolCalls": "integer"
}
```

구조화 작업과 동일하게 **필수 결과 제출 함수**가 encoding 채널이며 EUD 툴이 아니다. 제출 전 boundary
(round/deadline/context pressure)에 도달하면 부분 결과를 만들지 않고 위임 실패로 끝난다.

### 자식 툴 집합

| 부모 kind | 자식에게 광고되는 툴 |
|---|---|
| EPS | `tool_registry()`의 read 툴 중 `build_run`, `python_dependencies_prepare`, `ask`, `delegate_read` 제외 |
| Map | `map_tool_registry()`의 read 툴 (`map_status`, `map_selection_read`, `map_objects_read`, `map_render`, `map_palette_query`, `map_tile_info`, `map_analyze`, `map_candidate_diff`) — draft 계열 제외 |

자식이 광고되지 않은 툴을 호출하면 기존 규칙대로 unknown-tool **fatal**이지만, 치명 범위는 자식
run이다. 부모는 `delegate_read` usage error `{ code: "delegation_failed", reason }`를 받아 스스로
정정한다. 자식 run은 `WorkspaceAccess::Read`로 고정되고 `write_transition_requested`는 항상 거부된다.

### 부모 측 툴

두 registry에 read 툴 `delegate_read`를 추가한다.

```json
{ "goal": "string", "focus": ["string"] }
```

- `requires_write_workspace: false`, `requires_project_transaction: false`. 읽기 예산을 쓰지 않는다.
- 실행 경로: `SessionToolRuntime`가 provider 실행을 소유하지 않도록, ASK와 같은 emitter 패턴으로
  `set_delegation_executor(...)`를 `SessionEngineManager`가 주입한다. 실행기는 `production_adapter`로
  새 adapter를 만들어 `DelegatedRunExecutor::run`을 돌리고 결과 값을 tool result로 돌려준다.
- 대기 중 부모는 `ask_waiting`과 같은 `delegation_waiting` watch 채널로 active deadline을 멈춘다.
  자식 deadline은 240초이며 이것이 부모 툴 호출의 상한이다. 초과 시 자식은 실패하고 부모는
  `delegation_failed`를 받는다.
- 중첩 금지: 자식 descriptor에 `delegate_read`가 없다.
- 한 iteration당 위임 상한 8회(soft, 툴 액션으로 계수). 초과는 boundary가 아니라 usage error다.

### 이벤트·저널·사용량

- 자식의 text/reasoning은 부모 스트림에 들어가지 않는다 (structured job과 동일).
- 새 scoped 이벤트 `delegation`: `{ parentRunId, childRunId, status: queued|running|completed|failed|cancelled, toolCalls, elapsedMs, usage? }`. 자식 툴 호출 이름은 nested progress로만 흐른다.
- 자식은 canonical 변경을 못 하므로 journal 항목이 없다. native receipt store는 자식 run에 만들지
  않는다: 자식 결과는 복구 대상이 아니며 restart 시 부모가 위임 실패를 본다.
- 자식 usage는 세션 `context_usage.total`에 합산하고 `last`에는 반영하지 않는다. 보고되지 않은
  사용량은 만들지 않는다.

### 프로바이더별 동작

- Direct (OpenCode Go, Antigravity, Ollama): 새 빈 conversation으로 direct step/tool-result 루프.
  기존 `strict: false` 일반 descriptor, `strict: true` 결과 제출 descriptor 정책 유지.
- Native (Codex, Claude): 새 native 세션, cwd는 project root, 부모와 동일한 `--tools`/MCP 제한.
  구조화 작업처럼 continuation은 채택하지 않는다. 부모 CLI는 MCP 호출 `delegate_read`의 응답을
  기다리며, 이는 app37에서 검증된 native ASK 대기와 같은 경로다.
- **실측된 native CLI 한도(2026-09-17, `verify.md` 참조)**: Claude Code 2.1.274는 http MCP 호출이
  300초 동안 응답/progress가 없으면 중단하고, Codex 0.154.0은 `tools/call`을 300초에서 중단한다.
  두 CLI 모두 서버가 `progressToken`을 받아 30초마다 progress를 보내도 연장되지 않았다. 따라서
  eud-tools의 어떤 툴 호출도 240초 안에 반환한다(Phase 0의 ASK, 위임 자식 deadline, 팀 후보 대기).
  CLI 타임아웃 설정을 올리지 않으므로 어댑터 인자는 바뀌지 않는다.

### Context pressure 연계

자동 위임은 하지 않는다. `ContextPressure` boundary의 continuation 프롬프트에 "탐색은
`delegate_read`로 위임하라"는 고정 제어 문구를 추가하는 데서 멈춘다.

### Acceptance

- 자식 identity가 부모 session/request/취소 세대를 상속하고 고유 run_id/parent_run_id를 갖는다.
- 부모 취소 → 자식 gate 닫힘 → 진행 중 툴 정착 → 부모 tool result `cancelled`.
- 자식이 write 툴/`delegate_read`/`ask`를 호출하면 자식만 fatal, 부모는 usage error를 받고 계속된다.
- 결과 제출 전 boundary → `delegation_failed`, 부분 결과 없음.
- 자식 usage가 `total`에만 합산된다.
- 자식 대기 중 부모 active deadline이 진행하지 않는다.
- 다섯 프로바이더 fixture에서 위임 완료/실패/취소가 동일한 normalized 결과를 낸다.

## Phase 2: EPS → Map 팀 핸드오프

### 팀 Map 세션

EPS 세션 S의 **팀 Map 세션** M은 `map_task_request`마다 새로 만든다(2026-09-21 사용자 결정:
이전 요청의 provider thread·transcript·미적용 후보가 다음 요청 아래에 쌓이지 않도록, 항상 저장된
원본 맵의 r0에서 시작). S의 이전 팀 세션은 그 Apply를 Map 창에서 아직 undo할 수 있을 때만 남기고
나머지는 후보와 함께 삭제(retire)한다; 실행 중인 턴이 있는 세션은 그대로 두고 다음 요청 때 다시
시도한다. 삭제된 세션을 아직 보고 있던 Map 창의 늦은 로그 저장은 조용히 버려진다.

- `SessionKind::Map`, 이름 `"<S 이름> · 맵 작업"`, provider binding은 S의 binding을 복사(전역
  기본값이 아님; 세션 binding 불변 규칙 유지).
- `SessionMeta`에 `team_parent: Option<String>`을 추가한다. 팀 세션은 Map 세션 목록에 표시되지만
  parent 배지를 갖고, S 삭제 시 함께 삭제된다(후보가 남아 있으면 삭제를 거부하고 안내한다).
- 세션 이동/이름 변경/삭제는 기존 규칙을 따른다. 팀 세션이 다른 세션의 실행 lane을 훔치지 않는다.

### 팀 작업 레코드

S의 `SessionRecord`에 durable `team_tasks: Vec<TeamTask>`를 추가한다.

```rust
pub struct TeamTask {
    pub id: String,
    pub parent_request_id: String,
    pub map_session_id: String,
    pub map_request_id: Option<String>,
    pub goal: String,
    pub selections: Vec<TeamSelection>,        // MapRegion/MapLocation 스냅샷 → Map selection id
    pub layers: Vec<MapLayer>,                 // 요청 범위; Map 워커의 target 권한
    pub source_map_sha256_at_create: String,
    pub status: TeamTaskStatus,
    pub candidate: Option<TeamCandidateSummary>, // revision, diff 요약, 생성/변경된 location 이름·ID, 객체 수
    pub applied_source_sha256: Option<String>,
    pub map_apply_journal_ref: Option<String>,   // Map 세션 apply record 참조
}

pub enum TeamTaskStatus {
    Queued, Running, CandidateReady, Applied, Discarded,
    Failed { reason: String }, Cancelled, Interrupted,
}
```

### EPS 측 툴

```json
map_task_request {
  "goal": "string",
  "regions": [MapRegionMentionV1],     // 선택
  "locations": [MapLocationMentionV1], // 선택
  "layers": ["terrain"|"units"|"buildings"|"doodads"|"sprites"|"locations"],
  "await": "candidate" | "apply"
}
```

- 분류: `requires_write_workspace: false`, `requires_project_transaction: false`. 후보는 canonical이
  아니다. 그러나 **evidence 게이트는 통과해야 한다**: 요청 전에 `map_info` 등으로 현재 map 상태를
  읽은 증거를 요구한다.
- 실행 순서:
  1. `TeamTask` 생성·영속(`Queued`), source map hash 기록.
  2. 새 팀 Map 세션 M 생성, 이전 팀 세션 retire(undo 가능한 것 제외).
  3. `regions`/`locations`를 M의 `CandidateStore::save_selection`으로 `target` 역할 selection으로
     저장. stale 스냅샷은 usage error.
  4. M에 정규 `map_chat(request_id = 새 ID, text = 고정 템플릿(goal + selection 참조 + layers))`를
     제출. `Running`.
  5. M의 run이 후보를 finalize/commit하면 `CandidateReady`, 실패/취소면 `Failed`/`Cancelled`.
  6. 툴은 최대 240초까지 `CandidateReady`/`Failed`/`Cancelled`를 기다린다. 그 안에 끝나지 않으면
     `running`으로 반환한다.
- 대기 메커니즘은 Phase 1과 같은 `delegation_waiting` 채널이다. **Apply는 툴 안에서 기다리지
  않는다.** `candidate_ready` 또는 `running`을 받은 모델은 사용자에게 Map 창에서 후보를 검토·적용해
  달라는 텍스트로 턴을 끝낸다. Apply/폐기 결과는 `TeamTask` 상태로 영속되어 다음 turn의 task-state
  projection에 들어가고, 패널의 팀 작업 카드는 `candidate_ready`부터 "이어서 진행" 버튼으로 정해진
  사용자 메시지를 보낸다.
- **백그라운드 정산의 자동 이어받기.** 툴이 `running`으로 반환한 뒤(`detached`) 태스크가
  `candidate_ready`/`failed`로 정산되면 settler는 `TeamCommand::ContinueInteractive`를 보내고,
  디스패처는 `SessionEngineManager::team_continue`로 EPS 세션에 "이어서 진행"과 같은 고정 메시지
  (`TEAM_TASK_CONTINUE_TEXT`)의 정규 interactive turn을 시작한다. 엔진 lock을 기다려 `running`을
  받은 턴이 끝난 뒤에만 시작하고, phase가 `Idle`이 아니거나(검토·계획 대기) autonomous run이
  살아 있거나 태스크가 그 사이 다른 상태가 되면 시작하지 않는다 — 그 경우 다음 사용자 턴의
  `[map tasks]`가 전달한다. 시작 직전 `team_task` 이벤트에 `continuation{clientTurnId, text}`를
  실어 패널이 사용자 말풍선을 기록하고 `chatSent()`로 턴 진행 상태에 들어간다. `cancelled`는
  이어받지 않는다(사용자가 멈춘 것).
- 반환값:

```json
{ "taskId": "...", "status": "candidate_ready" | "running" | "failed" | "cancelled",
  "candidate": { "revision": 3, "summary": "...", "locations": [{"id": 12, "name": "Spawn A"}], "objectCounts": {"units": 8} } }
```

같은 turn에서 `map_task_status { taskId }` read 툴로 상태를 다시 읽을 수 있다(240초 상한 내 재시도).
`applied`가 아니면 로케이션/유닛은 **아직 원본 맵에 없다**. 툴 설명과 system prompt에 명시한다:
후보 상태에서 그 로케이션 이름을 참조하는 트리거를 빌드하면 euddraft가 실패하므로, 빌드가 필요한
흐름은 Apply 뒤의 다음 turn에서 이어간다.

### 상호 배제

- S의 request에 `Running`/`CandidateReady` 팀 작업이 있으면 S의 `location_write`, `switch_write`,
  `map_sound_*` 호출은 usage error `map_task_in_progress`로 완료된다.
- M의 Apply는 기존 `verify_current_for_apply`(source hash, build marker, no-share probe, 백업)를 그대로
  받는다. S가 그 사이 map을 바꿨다면 후보는 새 원본 위로 옮겨진 뒤 적용된다(옮길 수 없으면
  `sourceDiverged`; `map_task_diff`가 이를 보고하고 `map_task_apply`는 거부하므로 사용자만 Map 창에서
  덮어쓸 수 있다).
- `build_run`(S)은 project transaction만 잡는다. M의 draft 작업은 후보 파일만 만지므로 병행 가능하고,
  M의 Apply는 build marker가 잡힌 동안 기존 규칙대로 거부된다.

### 리뷰 연결

- S의 changeset 이벤트에 `linkedMapApplies: [{ taskId, revision, appliedSourceSha256, undoAvailable }]`를
  추가한다.
- S changeset **reject**는 EPS 파일만 역순 롤백한다. 연결된 Map apply는 자동으로 되돌리지 않고,
  리뷰 패널에 별도 "맵 적용 취소" 동작(기존 `map_agent_apply_undo`)을 노출한다. Undo 가능 여부는
  기존 `last_apply_record` 규칙을 따른다.
- S changeset **accept**는 팀 작업을 `Applied`로 보존한다. M의 apply journal은 M이 소유한다.
- 팀 작업 상태는 S의 task-state projection에 포함되어 다음 turn/iteration continuation에 들어간다.

### Autonomous 연계

- 팀 작업이 `CandidateReady`/`Running`인 채로 turn이 typed completion 없이 끝나면 autonomous run은
  `waiting_input`(pause reason `team_apply`)으로 pause한다. Apply/폐기 뒤의 다음 사용자 메시지(또는
  "이어서 진행")가 기존 explicit resume 검증을 거쳐 같은 run을 이어간다.
- runtime-affecting 완료 조건(현재 canonical revision의 성공 빌드)은 변하지 않는다. Map apply는
  source map hash를 바꾸므로 그 이후 빌드가 필요하다.

### Restart

- S: `Running` → `Interrupted`. `CandidateReady`는 M의 후보 revision과 source hash가 일치하면
  유지, 아니면 `Discarded`. `Applied`/`Discarded`/`Failed`는 그대로.
- M: 기존 미적용 후보 복구 규칙 그대로. 사용자는 Map 창에서 여전히 Apply할 수 있고, S의 다음
  turn은 task-state로 결과를 본다. 자동 재개는 없다.

### Acceptance

- `map_task_request` → M에 정규 요청 생성, selection 저장, `CandidateReady` 전이, S tool result 일치.
- 240초 안에 후보가 나오면 `candidate_ready`, 아니면 `running`; 어느 쪽도 Apply를 기다리지 않는다.
  Apply 뒤 다음 turn의 task-state projection에 `Applied` + 새 source hash가 들어간다.
- 폐기 → `discarded`; M 실패/취소 → 해당 상태; S 취소 → M 요청 취소 + 팀 작업 `Cancelled`.
- 팀 작업 중 S `location_write` → usage error. S가 map을 바꾼 뒤 M Apply → rebase 후 적용.
- S changeset reject가 Map apply를 건드리지 않고 "맵 적용 취소" 동작이 별도로 동작한다.
- restart 매핑이 표대로 되고 M 후보가 보존된다.
- 팀 세션 binding이 S binding과 같고 전역 기본값 변경에 영향받지 않는다.

## Implemented result (Phases 1–2, 2026-09-20)

동작 권위는 `features/05_agent-core.md` "Read delegation and Map handoff"와 `features/sessions.md`
"Team map tasks"로 넘어갔다. 계획과 다른 점:

- `delegate_read`는 EPS 세션에만 광고된다. Map 세션용 읽기 분류(`DelegatedToolProfile`의 Map
  registry 검증)가 없어 Map 세션 위임은 이후 작업이다.
- 자식 툴 집합은 계획의 4개 제외에 더해 `propose_plan`(foreground 제어 툴)을 제외한다.
  `trace_test_run`/`trace_suite_run`은 이제 어떤 세션에도 광고되지 않는다.
- `delegation_waiting` 채널은 두지 않았다. foreground `active_deadline`은 `None`이라 멈출 deadline이
  없고, 자식은 자기 240초 deadline을 그대로 갖는다. 대신 세션 활동은 `running_read`를 유지하고
  패널은 `delegation` 이벤트와 중첩 카드로 대기 상태를 보여 준다.
- 자식 usage는 세션 `context_usage.total`에만 합산되고 `last`는 부모 것이 유지된다(계획대로).
- `map_task_request`의 입력은 mention 스냅샷 대신 `selectionIds`(영구 target selection id)와
  `locationIds`(정확한 로케이션 id)다. 엔진이 팀 세션의 현재 후보 상태에서 검증된
  `MapMentionSnapshot::Region`/`Location`으로 투영한다. `await: "apply"`는 두지 않았다: Apply는
  툴 안에서 절대 기다리지 않는다.
- evidence는 같은 request에서 `map_info`가 한 번 성공했는지(`RequestState.map_inspected`)로
  판정한다. 상호 배제는 request가 아니라 **세션** 단위이며(팀 Map 세션의 후보 계보가 하나이므로),
  제외 툴에 `player_setup`을 더했다(시작 위치도 맵 쓰기).
- Map 요청은 `SessionEngineManager` 생성 시 스폰되는 팀 디스패처 태스크가 실행한다. `worker()`가
  주입하는 실행기가 `map_chat`을 직접 await하면 두 opaque future가 서로를 정의해 컴파일되지
  않으므로, 실행기는 디스패처에 명령만 보낸다.
- 리뷰 연결(`linkedMapApplies`, 리뷰 패널의 "맵 적용 취소")과 Map 창 Apply 대화상자 문구, 세션
  활동 `waiting_team_apply`/`delegating` 값은 아직 없다. Undo는 Map 창의 기존 동작이며 팀 작업을
  `discarded`로 되돌린다.
- 트리아지 프롬프트는 배치 전용 요청을 `direct`로 보내고 foreground가 `map_task_request`를
  호출하도록 한다. 실제 5-provider 앱 검증(계획 "검증" 항목 1–5)은 아직 수행되지 않았다.
- 2026-09-21 실제 `rpg` 프로젝트에서 첫 `map_task_request`가 "the current source map belongs to
  another project"로 거부됐다. EPS 세션의 `meta.project`는 manifest 이름이고 Map 세션과
  `MapRevision.project_id`는 프로젝트 루트 해시인데 둘을 직접 비교한 탓이다. 이제 부모는 열린
  프로젝트의 manifest 이름으로, 팀 세션은 Map project id로 각각 검증·생성한다. 같은 사건에서
  모델이 실패를 "Map 창을 열어 달라"는 지시로 바꿨으므로, `[map handoff]` 가이드와 툴 설명은
  창을 요구하지 말라고 못박고, 후보가 준비되면 디스패처가 팀 세션으로 Map 창을 직접 연다
  (`open_map_window`, `map_agent_open {sessionId?}`, `map-agent-open-session` 이벤트).

## Phase 2b: 에이전트 주도 후보 검토·적용 (2026-09-21 구현)

사용자 결정: EPS 에이전트가 후보를 **읽고**, **후속 수정·폐기**하고, **스스로 Apply**까지 한다
(항상 에이전트 판단; 설정 토글 없음). 트리거 작업이 맵 배치를 전제로 하므로 배치 → 확인 →
적용 → 코드 흐름이 사용자 클릭 없이 이어져야 한다는 요구에서 나왔다.

- 읽기: `map_task_diff`, `map_task_objects`, `map_task_render`가 팀 세션의 현재 후보를 읽는다.
  후보가 태스크가 알린 리비전·해시와 다르면 거부. 성공은 `RequestState.candidate_inspected`에
  기록된다.
- 적용: `map_task_apply`는 canonical write(쓰기 전환·프로젝트 트랜잭션)이며 docs 게이트 대신
  같은 request의 후보 검사(inspection)를 요구한다. 실행은 Map 창 Apply와 동일한
  `MapAgentService::apply`(백업·검증·롤백·MapSafe)이고, 태스크는 `applied` + `appliedBy: agent`.
- 수정: 후보가 `candidate_ready`인 동안 후속 `map_task_request`를 허용한다. 같은 팀 세션 계보에
  리비전을 더하고 이전 태스크는 `superseded`가 된다. `queued`/`running` 중에는 여전히 거부.
- 폐기: `map_task_discard`는 팀 세션 후보를 버리고 태스크를 `discarded`로 만든다.
- 되돌리기: 사용자는 Map 창 Undo 또는 EPS 패널 카드의 "맵 적용 취소"(`map_task_apply_undo`)로
  에이전트의 적용을 되돌린다. 리뷰 패널 changeset 연결(`linkedMapApplies`)은 여전히 없다.
- 자율 실행: 활성 태스크로 `team_apply` pause된 실행은 태스크가 정산되면 디스패처가
  `autonomous_resume`으로 이어간다(`TeamCommand::ResumeAutonomous`). interactive 세션은 툴이
  `running`으로 돌아간 태스크가 정산되면 `team_continue`가 고정 메시지 턴을 스스로 시작한다
  (`TeamCommand::ContinueInteractive`); 240초 안에 정산된 태스크는 같은 턴에서 검토한다.
- 안전 경계: 팀 후보 툴은 위임 자식에게 모두 거부된다. 적용은 원본 맵 해시가 후보 baseline과
  일치할 때만 성공한다(MapSafe). `map_task_apply`는 journal에 기록되지 않으므로 changeset
  reject로는 되돌릴 수 없고 Undo로만 되돌린다.
- 검증: `map_task_apply_needs_candidate_inspection_and_runs_the_action_executor`,
  `a_ready_candidate_admits_a_follow_up_request_but_a_running_task_does_not`,
  `team_task_tools_are_refused_for_a_delegated_child`,
  `team_task_action_validates_the_live_candidate_and_discards_it`, 패널 카드/프로토콜 테스트.
  실제 프로바이더로 apply까지 완료한 실행은 아직 없다.

## Phase 3: 병렬 읽기 워커와 위임 리뷰

### 병렬 위임

- direct batch 안의 여러 `delegate_read`는 동시에 실행한다(최대 3, 초과분은 순서대로 대기). 각
  자식은 자체 gate/endpoint를 갖고 `SessionToolRuntime::dispatch`의 per-call `execution_lock`으로
  직렬화된다 — 읽기이므로 충분하다.
- native MCP는 CLI가 병렬 호출하는 만큼만 병렬이다; 런타임이 병렬을 만들어내지 않는다.
- 동시 자식 수는 세션 lifecycle 상태에 노출된다.

### 위임 리뷰

read 툴 `delegate_review`를 EPS registry에 추가한다.

```json
{ "scope": "changeset" | "build", "focus": ["string"] }
```

- 자식은 현재 request의 semantic journal before/after와 최신 build diagnostics를 읽기 툴
  (`changeset_read`, `build_diagnostics_read` — 필요 시 추가하는 read 툴)로 조회하고 고정 스키마
  `{ verdict: "pass"|"concerns", findings: [...] }`를 제출한다.
- `AutonomousRunPolicy.require_delegated_review: bool`(기본 false). true면 runtime-affecting 완료
  전에 `pass` verdict가 한 번 있어야 하고, 마지막 canonical revision 이후의 verdict만 유효하다.
- verdict는 완료 조건일 뿐 자동 승인 신호가 아니다. 리뷰 pause와 사용자 accept/reject는 그대로다.

### Acceptance

- 3개 동시 자식이 각자 결과를 내고 4번째는 대기 후 실행된다.
- 한 자식의 fatal이 다른 자식이나 부모를 중단시키지 않는다.
- `require_delegated_review`가 stale verdict(이전 revision)를 거부한다.

## Panel

- EPS 대화: `delegate_read`/`delegate_review` 툴 항목 아래 nested 카드(상태, 툴 호출 수, 경과 시간,
  "위임 취소" — 자식만 취소). 팀 작업 카드(상태, 목표, "맵 창 열기", 후보 요약, Apply 결과).
- 대기 상태 문구: "맵 에이전트가 후보 r3을 만들었습니다. AI가 이어서 검토합니다. 맵 창에서 직접
  적용하거나 폐기할 수도 있습니다." 카드 힌트는 자동 턴이 시작되지 않은 경우를 위해 "자동으로
  이어지지 않으면 이어서 진행을 누르세요"를 덧붙인다.
- 리뷰 패널: 연결된 Map apply 행 + "맵 적용 취소" 버튼(Undo 가능할 때만 활성).
- Map 창: 팀 세션 배지 "EPS 세션 ○○의 작업", 읽기 전용 목표 텍스트, 정상 후보 UI. Apply 확인
  대화상자에 "적용하면 EPS 세션이 이어서 진행합니다"를 표시.
- 세션 활동에 `waiting_team_apply`, `delegating`을 추가하고 `running_read`/`running_write`와
  분리한다.
- 모든 텍스트 한국어, 시맨틱 컨트롤, 포커스/aria, 장기 작업 중 트리거 비활성 규칙 준수.

## 순서와 의존

0. Phase 0 (ASK 240초 타임아웃 → `unanswered` 결과 → 패널 만료 처리 → autonomous pause).
1. Phase 1 (identity 확장 → `DelegatedRunExecutor` → `delegate_read` → 이벤트/패널).
2. Phase 2 (팀 세션 → `TeamTask` → `map_task_request` → 상호 배제 → 리뷰 연결 → restart).
   Phase 1의 대기 채널과 nested 이벤트를 재사용한다.
3. Phase 3 (병렬 상한 → `delegate_review` → autonomous policy).

각 phase는 단독으로 배포 가능하고 이전 phase의 acceptance를 깨지 않아야 한다.

## 검증

- Rust: `provider_runtime/contract_tests`에 위임 run 계약(identity 상속, 취소 전파, budget 실패 →
  usage error, 광고되지 않은 툴 fatal 범위, usage 합산, deadline 정지)을 다섯 프로바이더 fixture로
  추가. `session.rs`/`engine.rs`에 `TeamTask` 상태 기계·restart 매핑·리뷰 연결 테스트.
  `tool_exec.rs`에 팀 작업 중 map 도구 거부, `map_candidate.rs`에 팀 selection 저장/stale 검사.
- Panel: Vitest(카드/대기 문구/리뷰 행), TypeScript, production build.
- 실제 앱: OpenCode Go(direct) 1개와 Codex 또는 Claude(native) 1개에서
  1. `delegate_read` 완료/실패/취소;
  2. EPS → `map_task_request` → 턴 종료 → Map 창 후보 → Apply → "이어서 진행" → 트리거 작성 →
     `build_run` 성공;
  3. 폐기 경로; 4. 팀 대기 중 취소; 5. 팀 대기 중 restart 후 Map 창 Apply와 다음 EPS turn.
- 선행 실측(완료, 2026-09-17): Codex/Claude CLI의 MCP 툴 응답 상한은 둘 다 300초이며 `verify.md`에
  기록되어 있다. 그래서 Phase 0이 먼저이고, 모든 툴 내 대기는 240초 상한을 갖는다.
- 완료 시 임시 fixture/스크립트 제거, `architecture.md`/`rules.md`/`05_agent-core.md`/
  `sessions.md` 갱신.

## 열린 질문

1. (해결) native CLI 상한은 기본 300초다. CLI 한도를 올리는 대신 **모든 툴 내 대기를 240초로
   묶고 더 긴 기다림은 턴 종료 + 다음 사용자 메시지로 잇는다**(Phase 0). 어댑터는 CLI 타임아웃
   설정을 넘기지 않는다.
2. 팀 Map 세션의 provider를 EPS와 다르게(예: 이미지 지원 모델) 선택하는 옵션은 유용하지만 세션
   binding 불변 규칙과 충돌하지 않는 범위(생성 시 1회 선택)에서만 고려한다. 기본은 복사다.
3. `delegate_review`가 읽을 `changeset_read`/`build_diagnostics_read`를 일반 read 툴로도 광고할지
   여부. 광고하면 부모가 스스로 리뷰할 수 있어 위임의 필요가 줄지만 컨텍스트 이점은 사라진다.
