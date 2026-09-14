# Provider transcript SSE delta 병합 구현 계획

Status: proposed (2026-09-14)

## 1. 목표

OpenCode Go direct provider가 하나의 assistant 응답을 여러 SSE delta로 전송하더라도 UI 스트리밍은 그대로 유지하고, provider 재호출 및 crash-safe checkpoint에 사용하는 transcript에는 인접 delta를 병합한 semantic block만 저장한다.

이번 작업은 최근 `opencode-go` 세션에서 발생한 다음 오류만 해결한다.

```text
provider protocol failed: provider transcript has too many blocks
```

## 2. 관찰된 원인

최근 실패 세션의 마지막 유효 checkpoint는 revision 19이며 SHA-256 pointer 검증을 통과한다.

- 전체 transcript block: 3,999개
- `AssistantReasoning`: 3,970개
- reasoning 본문 합계: 13,868자
- 한 응답의 reasoning block: 3,429개
- 해당 응답의 reasoning 본문: 11,755자
- 평균 reasoning block 길이: 3.43자
- `MAX_ENTRIES`: 4,096개

`src-tauri/src/opencode_go/stream.rs::wire_delta_blocks()`는 SSE text/reasoning delta마다 `NormalizedBlock`을 생성한다. `src-tauri/src/provider_runtime/runtime/events.rs::handle_event()`는 이 블록을 UI sink에 내보내는 동시에 `StepResult.blocks`에도 개별 추가한다. 이후 direct transcript writer가 `StepResult.blocks`를 `TranscriptBlock`으로 일대일 저장하므로 transport fragmentation이 영구 transcript block 수를 소모한다.

블록 제한은 schema v1의 semantic `TranscriptEntry`를 위한 방어 한도로 도입됐다. 현재 실패는 실제 컨텍스트 고갈이 아니다. 오류 시점 사용량은 약 73K tokens이고 선택된 모델의 기록된 context window는 1M tokens였다.

## 3. 확정 결정

1. UI에는 현재와 동일하게 SSE delta를 도착 순서대로 전달한다.
2. `StepResult.blocks`에는 인접한 text/reasoning delta를 병합한다.
3. ToolCall, ToolResult, continuation/signature 및 응답 경계는 보존한다.
4. 정확히 빈 text/reasoning delta는 transcript block으로 만들지 않는다. 공백 문자열은 보존한다.
5. `MAX_ENTRIES = 4096`은 변경하지 않는다.
6. 블록 수를 이유로 자동 compaction을 추가하지 않는다.
7. 테스트 과정에서 생성된 기존 과다 블록 transcript를 위한 migration이나 새 저장 schema를 추가하지 않는다.
8. 현재 테스트 세션은 구현 검증 후 기존 conversation reset/delete 경로로 일회성 정리한다.

## 4. 비목표

- 4,096 제한 상향 또는 128K block 허용
- 토큰 사용률이 낮은 대화의 자동 compaction
- 기존 generation의 restore-time 정규화
- transcript schema v3 또는 호환성 변환
- append-only/chunked transcript 저장소 재설계
- stream event와 durable transcript 타입 전체 리팩터링
- 도구 실행 admission 및 오류 타입 체계의 일반 재설계
- `RunPolicy.max_output_bytes` 의미 변경
- Codex/Claude native session 동작 변경

## 5. 변경 대상

### 5.1 공통 런타임의 durable block 누적

대상:

- `src-tauri/src/provider_runtime/runtime/events.rs`

`handle_event()`의 `AdapterEventKind::Block` 처리에서 UI sink 발행과 `StepResult.blocks` 누적을 분리한다.

UI sink에는 수신한 delta를 현재와 동일하게 발행한다. durable accumulator에는 다음 규칙을 적용한다.

#### Text 병합

다음 조건이 모두 참이면 마지막 `NormalizedBlock::Text`의 문자열 끝에 새 delta를 추가한다.

- 마지막 블록과 새 블록이 모두 `Text`
- 두 블록의 `response_id`가 동일
- 두 블록이 인접

그 외에는 새 블록을 추가한다.

#### Reasoning 병합

다음 조건이 모두 참이면 마지막 `NormalizedBlock::Reasoning`의 문자열 끝에 새 delta를 추가한다.

- 마지막 블록과 새 블록이 모두 `Reasoning`
- 두 블록의 `response_id`가 동일
- 두 블록이 인접
- 마지막 블록과 새 블록의 `continuation`이 모두 `None`

continuation/signature가 존재하는 reasoning 블록은 병합하지 않는다. ToolCall 또는 ToolResult가 사이에 있으면 인접하지 않으므로 병합하지 않는다.

#### 소유권과 할당

현재 경로는 durable vector와 UI event 사이에서 블록을 한 번 clone한다. 변경 후에도 clone 횟수를 늘리지 않는다. UI용 clone을 발행하고 원본 블록을 durable accumulator로 이동하거나, 기존과 동등한 한 번의 clone만 사용한다.

### 5.2 빈 OpenCode delta 제거

대상:

- `src-tauri/src/opencode_go/stream.rs`

`wire_delta_blocks()`가 `Some("")`인 text/reasoning을 `NormalizedBlock`으로 만들지 않도록 한다.

- `""`: 제거
- `" "`, 줄바꿈 및 기타 비어 있지 않은 문자열: 보존
- tool call 및 continuation 처리: 변경 없음

공통 런타임에서 provider별 프로토콜 결함을 추측하지 않는다. 빈 delta를 생성하는 OpenCode codec 경계에서만 제거한다.

## 6. 불변조건

1. UI가 관찰하는 비어 있지 않은 delta의 순서와 내용은 변경되지 않는다.
2. 병합된 transcript 문자열은 입력 delta를 순서대로 연결한 결과와 바이트 단위로 동일하다.
3. 서로 다른 `response_id`의 블록을 병합하지 않는다.
4. Text와 Reasoning을 서로 병합하지 않는다.
5. ToolCall/ToolResult 전후의 블록을 가로질러 병합하지 않는다.
6. provider continuation 및 thought signature를 삭제·복제·이동하지 않는다.
7. tool call/result 상관관계, checkpoint boundary 및 revision/pointer 검증은 변경하지 않는다.
8. native provider의 conversation/session 경로는 변경하지 않는다.

## 7. 구현 순서

1. `events.rs`에 인접 semantic block 누적을 담당하는 작은 private helper를 추가한다.
2. `handle_event()`가 UI delta 발행과 durable 누적을 각각 수행하도록 연결한다.
3. `stream.rs::wire_delta_blocks()`에서 정확히 빈 text/reasoning delta를 걸러낸다.
4. 기존 OpenCode/runtime production fixture를 확장해 4,096개를 넘는 transport fragmentation을 재현한다.
5. focused regression을 실행해 checkpoint 저장과 후속 tool result가 완료되는지 확인한다.
6. 검증 후 실패 재현에 사용한 현재 테스트 conversation을 기존 reset/delete 경로로 정리한다. 제품 migration 코드는 남기지 않는다.

## 8. 회귀 테스트

기존 `provider_runtime::contract_tests`의 실제 OpenCode Chat Completions HTTP fixture를 사용한다. prebuilt adapter 결과를 직접 반환하는 mock으로 대체하지 않는다.

한 응답을 다음과 같이 구성한다.

1. 동일한 `response_id`의 작은 reasoning delta 5,000개
2. 중간의 정확히 빈 reasoning delta
3. tool call 1개
4. 실제 direct tool gate가 반환하는 tool result 1개
5. 완료된 후속 assistant 응답

필수 assertion:

- 실행 결과가 `provider transcript has too many blocks` 없이 완료된다.
- UI/runtime sink는 5,000개의 비어 있지 않은 reasoning delta를 원래 순서로 관찰한다.
- 빈 delta는 sink 및 durable transcript에 의미 있는 출력으로 남지 않는다.
- 저장된 checkpoint에는 5,000개 delta의 연결 결과와 정확히 같은 reasoning block 1개가 있다.
- ToolCall과 ToolResult는 각각 별도 block이며 ID, 이름, batch 상관관계가 유지된다.
- 후속 provider 요청의 history에는 병합된 reasoning 내용이 정확히 한 semantic 항목으로 전달된다.
- current pointer의 revision과 generation SHA-256 검증이 통과한다.

같은 테스트 또는 작은 helper 단위 테스트에서 다음 경계도 확인한다.

```text
Text(A), Text(B)                         -> Text(AB)
Reasoning(A), Reasoning(B)               -> Reasoning(AB)
Text(A), Reasoning(B), Text(C)           -> 병합 없음
Reasoning(A), ToolCall, Reasoning(B)      -> 병합 없음
response-1 Text(A), response-2 Text(B)    -> 병합 없음
Reasoning(None), Reasoning(continuation)  -> 병합 없음
Text(""), Text(" ")                     -> 빈 문자열만 제거
```

## 9. 검증

최소 focused 검증:

```powershell
cargo test -p eud-agent provider_runtime::contract_tests::<새_회귀_테스트명> -- --exact
cargo test -p eud-agent opencode_go::tests --no-fail-fast
cargo check -p eud-agent --lib
```

검증 결과에서 확인할 사항:

- 5,000개 delta 시나리오가 영구 reasoning block 1개로 저장됨
- UI sink의 실시간 delta 순서가 유지됨
- tool result checkpoint와 후속 응답이 정상 완료됨
- 기존 OpenCode Responses, Chat Completions, Anthropic Messages 테스트에 회귀 없음
- 경고 없는 library check

## 10. 완료 기준

다음 조건을 모두 만족하면 완료다.

- 최근 오류와 동일한 원인인 4,096개 초과 transport fragmentation이 production OpenCode 경로에서 재현 테스트로 고정된다.
- 수정 후 같은 시나리오가 정상 완료된다.
- UI 스트리밍은 delta 단위로 유지된다.
- direct provider transcript에는 병합된 semantic block만 저장된다.
- 빈 OpenCode text/reasoning delta가 저장되지 않는다.
- block limit 상향, 자동 compaction, 과거 transcript migration 또는 저장소 재설계가 포함되지 않는다.
- 테스트용 기존 과다 블록 conversation은 제품 코드 추가 없이 정리된다.
