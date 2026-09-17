# 프로젝트 루트 cwd 전환 — 구현 계획

Status: proposed (2026-09-14). `features/sessions.md`의 "Workspace isolation" 절과
`features/sessions-write-queue-plan.md`의 session-root 계약을 대체한다.

## 1. 목표

Provider CLI(Codex/Claude Code)의 작업 디렉터리를 **`project.eap`가 있는 프로젝트 루트**로 옮긴다.
에이전트가 실제 프로젝트 구조(`project.eap`, `src/`, `dat/`, `maps/`, `build/`)를 그대로 보고,
매 턴 발생하는 AppData 문서 복사와 `source/` 미러 재생성을 없앤다.

동시에 **검토/롤백 계약은 유지**한다: 정본 소스와 문서 변경은 계속 저널·changeset·write lease를
통해서만 반영된다.

## 2. 문제 제기

### 2.1 현재 위치

정본 권위는 이미 프로젝트 루트 안에 있다.

```
<project>/.eud-agent/workspace/{specs,plans,decisions,worklog}   ← accepted 정본
<project>/.eud-agent/state/workspace.json                        ← trusted 소유권 상태
<project>/.eud-agent/memory/                                     ← 프로젝트 메모리
```

실측(`E:\proj\eud\proj1\native`): specs 7, plans 76, decisions 2, worklog 58 = **143개 문서**.

그런데 CLI의 cwd는 거기가 아니다.

```rust
// provider_runtime/runtime/state.rs:57
request.turn.workspace_root = Some(prepared.root.clone());
// workspace.rs:1146 → config.rs:361-368
%APPDATA%/eud-agent/workspaces/.sessions/<workspace-id>/<session-id>/
```

매 턴 `sync_documents`(`workspace.rs:2663-2682`)가 정본 143개 문서를 **양쪽 모두全文 읽어**
해시 비교 후 AppData로 복사하고, `refresh_source`(`:1721-1767`)가 `source/` 미러를
`remove_dir_all` 후 재생성한다.

### 2.2 비용

| 증상 | 근거 |
|---|---|
| CLI가 프로젝트 구조를 못 본다 | cwd에 `project.eap`·`dat/`·`maps/`·`src/` 없음. `source/` 미러뿐 |
| 경로 이원화 | 미러는 `src/` 접두가剥離됨(`workspace.rs:149-153`, `:2236`). CLI가 보는 `source/main.eps`와 도구가 받는 `src/main.eps`가 다름. `native_source_path()`(`tool_exec.rs:4450-4457`)가 강제 접두로 봉합 |
| 매 턴 全文 복사 | `sync_documents`가 정본+세션 양쪽을 `scan_text_tree`로 메모리에 읽음 |
| 미러 오염 | `source/__epspy__/*.py` 10파일 378.2 KiB가 정본 소스처럼 복제됨 (실측: 미러 20파일 중 10개가 생성물 `.py`) |
| 모델 혼란 유발 | 최근 세션에서 미러의 `__epspy__/`를 "import 루트 마커"로 오판 (`18d4edf0` panelLog id 39) |

### 2.3 "자유 CRUD"의 실제 현 상태

이전 검토에서 문서 디렉터리가 자유 CRUD라고 기술한 것은 **틀렸다**. 정정한다.

```rust
// codex_client.rs:906-907
"permissions.eud_workspace_read={... \":workspace_roots\"={\".\"=\"read\"} ...}"
"permissions.eud_workspace_write={... \":workspace_roots\"={\".\"=\"read\", \".tmp/**\"=\"write\"} ...}"
// codex_client.rs:1701-1703 (테스트가 고정)
assert!(!implementation_profile.contains("\".\"=\"write\""));
```

Claude Code는 더 좁다 — `--tools ""` + `--allowedTools mcp__eud-tools__*`
(`claude_client/adapter/request.rs:84-87`)로 **파일시스템 도구가 아예 없다**.

즉 파일시스템 쓰기는 `.tmp/` 하나뿐이고, 문서·소스 변경은 전부 MCP 도구 → 저널 → 검토 경유다.
이 구조는 이 계획에서도 **유지**한다(§4 참조).

## 3. 지상 사실 (Ground Truth)

전환 설계가 반드시 지켜야 하는 현재 불변식.

### 3.1 baseline 스캔은 텍스트 전용 + 한도

```rust
// workspace.rs:1233
let snapshot = scan_text_tree(&workspace.root, ScanMode::Writable)?;
// :36-38   MAX_FILES 2048 / MAX_FILE_BYTES 1 MiB / MAX_TOTAL_BYTES 32 MiB
// :2606-2611  파일당 1 MiB 초과 → Err
// :2638-2651  decode_utf8 — BOM/비UTF-8 → Err
```

프로젝트 루트를 그대로 스캔하면 즉시 깨진다:

```
compat/editor-project.e3s   1,598,295 B = 1.52 MiB   → 1 MiB 한도 초과
maps/proj1.scx                                       → 바이너리, decode_utf8 실패
build/proj1_EUD.scx                                  → 바이너리
```

**→ 스캔 대상을 명시적 allowlist로 좁히는 것이 이 전환의 최우선 선행 작업이다.**

### 3.2 문서 저널·병합·거부

- 저널 대상: `JournalTarget::WorkspacePath { workspace_id, session_id, path }` (`journal.rs:166-169`)
- 승격: `promote_entries`(`workspace.rs:1541-1623`)가 세션→정본을 3-way merge
- 충돌: `merge_workspace_content`(`:2198-2228`) → `ConcurrentWriteConflict`(`:2379-2381`)
- 거부: `restore_file(workspace_id, session_id, path, content)`(`:1519-1539`) —
  `session_id`가 있으면 **세션 사본만** 복원

`session_id` 분기가 사라지면 거부 경로가 정본 복원으로 바뀌어야 한다.

### 3.3 소스 baseline (낙관적 동시성)

```rust
// tool_exec.rs:1019-1028 → workspace.rs:2230-2241
pub(crate) fn read_source_baseline(workspace_root, relative) {
    let source_root = workspace_root.join(SOURCE_DIR);   // <session>/source/
    let relative = relative.strip_prefix("src/").unwrap_or(relative);
    ...
}
```

`file_write`/`file_edit`/`file_rename`/`file_move`/`file_delete`가 이 값을 stale 검사 기준으로 쓴다
(`tool_exec.rs:2799-2931`). 미러가 사라지면 이 기준을 **명시적으로 캡처**해야 한다.

### 3.4 Provider 별 차이

| Provider | cwd 사용 | FS 도구 | 샌드박스 |
|---|---|---|---|
| Codex | `thread/start`·`thread/resume`의 `cwd` (`codex_client.rs:951-952,968-970`) | 있음 | Windows elevated sandbox, `permissions.*` 프로필 (`:571-597,899-908`) |
| Claude Code | `command.current_dir(cwd)` (`claude_client/adapter/process.rs:44`) | **없음** (`--tools ""`) | `CLAUDE_CONFIG_DIR` 격리 + ambient 설정 거부 (`request.rs:47-54,100-101`) |
| OpenCode Go / Antigravity / Ollama | HTTP — cwd 미사용 | MCP만 | 해당 없음 |

**→ 이 전환의 실수혜자는 Codex다.** Claude·직접 프로토콜 3종은 cwd 변화의 영향을 거의 받지 않는다.

### 3.5 TEMP 리디렉션

```rust
// codex_client.rs:1050-1053
let private_tmp = cwd.as_ref().join(crate::workspace::TEMP_DIR);
if private_tmp.is_dir() { command.env("TEMP", &private_tmp).env("TMP", &private_tmp); }
```

공유 루트에서 세션 간 `TEMP` 충돌을 막으려면 세션별 하위 디렉터리가 필요하다.

### 3.6 epScript import 해석 (euddraft 원본 검증 완료)

`E:\proj\eud\euddraft` 원본 코드로 확정한 규칙. 추측이 아니라 세 파일 라인 인용 +
절대경로 호출 재실험(7개 케이스)으로 검증했다.

```python
# euddraft.py:127-133 — argv[1]이 절대경로일 때만 트리거
dirname, sfname = os.path.split(sfname)
if dirname:
    os.chdir(dirname)
    sys.path.insert(0, os.path.abspath(dirname))   # sys.path[0] = EDS 폴더

# pluginLoader.py:202-204 — 매 플러그인마다, dirname 무관 항상 실행
pluginDir = os.path.dirname(pluginPath)            # getPluginPath가 abspath화 (:54-63)
if pluginDir and pluginDir not in sys.path:
    sys.path.insert(1, os.path.abspath(pluginDir)) # sys.path[1] = 플러그인 파일 폴더
# :236 매 플러그인 후 복원

# epsimp.py:177-185 — EPSFinder
def find_spec(self, fullname, path, target=None):
    if path is None:
        path = sys.path        # 절대 import는 sys.path 순회 + Python namespace package 규칙
```

`getPluginPath`(`pluginLoader.py:54-63`)는 `.eps`/`.py`를 `os.path.abspath`로 정규화하므로
EDS의 플러그인 섹션이 상대경로(`../../src/main.eps`)든 절대경로든 `pluginDir`은 항상
**플러그인 파일의 실제 폴더**다. native 빌드는 main EPS를 `../../src/<main>.eps`로 참조하므로
`pluginDir = <project>/src`가 매번 `sys.path[1]`에 들어간다.

**핵심 결론:** `<project>/src`는 pluginDir로서 EDS 위치와 무관하게 항상 `sys.path`에 있다.
따라서 src-relative import는 어떤 레이아웃에서도 동작한다.

검증 매트릭스 (절대경로 호출, 플러그인 섹션 = `<abs>\src\main.eps`):

| EDS 위치 | sys.path[0] | `import src.leaf` | `import leaf` / `import sub.deep` (src-relative) |
|---|---|---|---|
| 프로젝트 루트 | `<project>` | ✅ namespace pkg | ✅ |
| `build/euddraft` (현재 native) | `build/euddraft` | ❌ `No module named 'src'` | ✅ |

중첩 추가 실측 (MainFile이 `src/`): `src/sub/a.eps`에서 `import sub.b`(src-relative dotted) ✅,
`import .b`(same-folder relative) ✅, `import b`(bare sibling) ✅. `import ..leaf`(top-level 초과)
는 `ImportError: attempted relative import beyond top-level package`(`helper.py:65-66`) ❌.

**→ A2 결정의 근거:** src-relative import(`import leaf`, `import sub.deep`, `import .leaf`)는
EDS를 옮기지 않아도 native 레이아웃에서 동작한다. `src.` 접두 절대 import만 EDS를 루트로
올려야 산다. 따라서 import 재작성은 **접두 제거**(src-relative화)가 정답이고 `native_build.rs`의
EDS 생성 위치를 손댈 필요가 없다. §D9·Phase 7.4 참조.

(실험 잔해는 정리 완료. 첫 프로브가 `cd <dir> && euddraft t.eds` 상대경로로 호출해
`dirname==""` → `sys.path[0]` 미삽입 상태였던 것이 초기 오판의 원인이었다 — §3.6은
절대경로 재실험으로 정정된 결과다.)

## 4. 핵심 결정

### D1 — cwd = 프로젝트 루트, 읽기는 전체 / 쓰기는 계속 저널 경유

CLI는 프로젝트 루트에서 **전체를 읽는다**. 파일시스템 **쓰기 허용은 `.eud-agent/workspace/.tmp/**`
하나**로 유지한다. 소스·문서 변경은 기존 MCP 도구 → 저널 → changeset 검토 경로를 그대로 쓴다.

이유: "루트에서 자유 CRUD"를 파일시스템 쓰기로 확장하면
changeset Reject/Undo(`features/06_changeset-review-panel.md`), write lease 직렬화
(`sessions-write-queue-plan.md:11`), 저널 롤백(`journal.rs:1239-1418`)이 전부 성립하지 않는다.
얻는 이득(구조 가시성·미러 제거·경로 이원화 소멸)은 읽기 개방만으로 전부 달성된다.

**포기하는 것:** CLI 네이티브 `Edit`/`Write`로 소스를 직접 수정하는 경로. 앞으로도 불가능하다.

### D2 — 스캔 allowlist

`ScanMode`에 프로젝트 루트용 범위를 추가한다. baseline/diff 스캔 대상은 정확히:

```
.eud-agent/workspace/specs/**       .eud-agent/workspace/plans/**
.eud-agent/workspace/decisions/**   .eud-agent/workspace/worklog/**
.eud-agent/workspace/.tmp/**        (스캔 제외 — 쓰기 전용 스크래치)
```

`src/`·`dat/`·`maps/`·`build/`·`compat/`·`project.eap`은 **스캔하지 않는다.**
소스 baseline은 §D4의 별도 캡처가 담당한다.

이것이 없으면 전환 자체가 불가능하다(§3.1).

### D3 — `source/` 미러 제거

`refresh_source`, `SOURCE_DIR`, `native_eps_snapshot`의 `src/` 剥離, `ScanMode::Writable`의
`source` 스킵, `WorkspaceFileEntry.source`, `ipc.rs:905-906`의 `source` 판정,
패널의 "읽기 전용 소스" 표시를 전부 제거한다.

CLI는 실제 `src/`를 읽고, 경로 이원화가 사라진다. `native_source_path()`의 강제 접두는
하위 호환을 위해 유지하되(에이전트가 `src/` 없이 보내는 경우가 있으므로) 더 이상 필수가 아니다.

### D4 — 소스 baseline을 명시적 캡처로 교체

`begin_turn`이 문서 baseline과 함께 **소스 baseline을 separately 캡처**한다:

```
%APPDATA%/eud-agent/workspaces/.state/baselines/<request_id>/<workspace_id>/
  documents/…      (기존 ScanMode::Writable 결과)
  source/…         (src/ 하위 .eps/.py全文, 생성물 제외)
  BASELINE         (마커)
```

`read_source_baseline`은 이 위치를 읽는다. `strip_prefix("src/")`는 유지 —
baseline 트리 구조를 `src/` 없이 저장하면 기존 호출부와 호환된다.

캡처 비용: 실측 `src/` 정본 소스는 `.eps` 9파일 138 KiB(생성물 제외, §5 참조).
미러 전체 복사보다 작다.

### D5 — 문서 저널에서 `session_id` 제거

`JournalTarget::WorkspacePath.session_id`를 없애고 경로는 항상 정본 기준
`.eud-agent/workspace/` 상대 경로로 기록한다.

- 승격(`promote_entries`) 3-way merge는 **유지** — 세션 사본 대신 baseline 바이트가 `base`가 된다
- 거부(`restore_file`)는 `session_id: None` 분기(정본 복원)만 남는다
- 직렬화 호환: 기존 저널의 `session_id`는 `#[serde(default)]`로 읽어 무시한다.
  미결 저널이 있는 상태로 업그레이드하는 경로는 §Phase 6에서 처리

### D6 — 세션 격리는 `.tmp`만 유지

문서·소스는 정본 하나를 공유한다. 동시성은 기존 write lease가 담당하므로
세션별 사본이 필요 없다. 읽기 전용 턴은 동시 실행 가능하며 쓰지 않는다.

`TEMP`/`TMP`는 `<project>/.eud-agent/workspace/.tmp/<session-id>/`로 세션별 격리한다.

**질문 1 답:** `specs`/`plans`/`decisions`/`worklog` 4종은 **이미** `<project>/.eud-agent/workspace/`
아래가 정본이다(`prepare_snapshot`이 반환하는 canonical root, `workspace.rs:1055-1061,1123-1128`;
실측 143파일). 이 계획은 그 4종을 옮기는 게 아니라 **AppData의 세션 사본과 복사·승격 왕복을
제거**하는 것이다(`sync_documents` `:2663-2682`, `promote_entries` `:1541-1623`).
`.tmp`만 신규로 `<project>/.eud-agent/workspace/.tmp/<session-id>/`로 옮겨진다(현재는 세션 루트 안).
`baseline`·`journal`은 **옮기지 않는다** — cwd 안에 두면 CLI가 자기 rollback 기준·감사 기록을
읽고 건드릴 수 있다. §3.2·§5 참조.

### D7 — Claude ambient 설정 경계 재정의

`validate_workspace_boundary`(`request.rs:47-54`)는 현재 cwd에 `.claude`, `.mcp.json`,
`CLAUDE.md`, `CLAUDE.local.md`가 있으면 **거부**한다. 세션 미러는 앱이 생성하므로
ambient 파일 존재 자체가 변조 의미였다. 프로젝트 루트에서는 그 전제가 사라진다.

분리 처리:

| 파일 | 결정 | 이유 |
|---|---|---|
| `CLAUDE.md` | **허용** | 사용자가 작성한 프로젝트 규칙. prose만 주입되며 `--tools ""`로 도구 추가 불가 |
| `CLAUDE.local.md` | **허용** | 동일 |
| `.claude/settings.json`, `.claude/settings.local.json` | **거부 유지** | `permissions.allow`, `hooks`로 도구·권한을 바꿀 수 있음 |
| `.claude/` 기타 | 거부 유지 | agents/commands/plugins가 도구를 추가할 수 있음 |
| `.mcp.json` | **거부 유지** | `--strict-mcp-config`에도 불구하고 등록 시도 자체를 막는 것이 안전 |

Codex는 이미 `project_doc_max_bytes=0`(`codex_client.rs:901`)으로 `AGENTS.md` 자동 로딩을
끄고 있으므로 변경 불필요.

### D8 — 생성물 `src/__epspy__/`, `src/__pycache__/`를 소스 트리에서 제외

선행 필수. 현재 상태:

```
src/__epspy__/*.py    9파일 378.2 KiB   mtime 20:20:26 (빌드 중 생성)
src/__pycache__/*.pyc 5파일 330.9 KiB   (.pyc는 확장자 필터로 이미 제외)
src/*.eps             9파일 138.4 KiB   ← 실제 정본
```

`collect_text_files`(`native_project.rs:644-648`)는 `src/` 전체를 훑고
`is_editable_source_path`(`:2050-2056`)는 `.py`를 허용하므로 `__epspy__` 그림자가
**정본 소스로 취급**된다. 결과:

- `revision()` 해시 포함(`:1103-1106`) → 빌드 1회 후 리비전 변화 → build 무진행 게이트 오작동 가능
- `has_direct_python()`이 true(`:611-618`) → `export_e3s`가
  `"직접 Python 소스 또는 의존성이 있는 프로젝트는 E3S로 내보낼 수 없습니다"`로 **영구 거부**
  (`e3s_nrbf.rs:239-241`). 이 프로젝트는 `pythonEntrypoints: []`, `pythonDependencies: []`,
  `pythonLock: null`인데도. compat base가 살아있는 수입 프로젝트라 export가 동작해야 정상
- `list_files`/`source_search`/changeset에 생성물 노출
- cwd 전환 시 CLI가 이 378 KiB를 실제 소스로 읽고 혼란 (`18d4edf0` id 39가 실제로 발생)

두 디렉터리 이름을 `collect_text_files`와 `copy_tree`(`trace_test.rs:755-761`이 이미
`__pycache__`를 스킵)에서 일괄 제외한다.

### D9 — E3S import 시 epScript import 재작성 (A2: 접두 제거)

§3.6에서 확정한 규칙에 따라, **E3S → native import 시점**에 본문 import 경로를
src-relative 형태로 재작성한다. `TriggerEditor`류 최상위 폴더 접두를 제거한다.

```
import TriggerEditor.survivor_hero as hero;   →   import survivor_hero as hero;
import TriggerEditor.sub.mod       as m;      →   import sub.mod as m;
```

**왜 import 시점인가:** EE3의 `TriggerEditor` 폴더는 `IsTopFile=true`라
`collect_te_sources`(`e3s_nrbf.rs:626-630`)가 폴더명을 native 경로 prefix에서 **제외**한다.
그래서 E3S 본문 `TriggerEditor.survivor_hero`는 native 파일 `src/survivor_hero.eps`에 대응한다.
파일을 옮기면서 본문 import를 그대로 두면(현재 상태) 빌드가 `No module named 'TriggerEditor'`로
깨진다 — 최근 두 세션에서 빌드 6회 중 5회가 정확히 이 때문이었다.

**재작성 규칙:**
1. 발견된 최상위(IsTopFile) 폴더 집합을 import prefix 후보로 사용한다. 이 프로젝트는
   `TriggerEditor` 하나(E3S 실측: import 10건 전부 `TriggerEditor.<module>`).
2. `import <candidate>.<rest>` 형태만 재작성한다. 문자열 리터럴·주석 안의 동일 텍스트는
   건드리지 않는다 — epScript `import` 문법 위치에서만 매칭한다.
3. `__epspy__` 그림자는 빌드 시 재생성되므로 import 단계에서 손대지 않는다(D8이 스냅샷에서 제외).
4. **export 역재작성:** `export_e3s`가 native `src/<rest>`를 E3S로 되돌릴 때
   `<candidate>.<rest>`로 복원해 round-trip을 보존한다
   (`real_fixture_import_export_import_is_semantically_stable`, `e3s_nrbf.rs:2051` 유지).
   후보 폴더명은 import 시 `editorCompatibility`에 보존한다.

**A1(기각):** `TriggerEditor.P → src.P`로 재작성하고 EDS를 프로젝트 루트로 옮기는 안.
§3.6 매트릭스에서 `import src.X`는 EDS가 루트에 있을 때만 산다(namespace package).
EDS를 옮기려면 `native_build.rs`의 `eud-agent.eds` 생성 위치 + `[DataEditor.py]`·
`[ExtraDataEditor.py]`·`custom_txt.tbl`·`[PythonPath.py]` 4개 생성물 경로를 전부 조정해야 하고
루트에 생성물 `eud-agent.eds`가 노출된다. A2는 접두만 떼면 pluginDir=src 기준으로
EDS 위치 무관 동작하므로 native_build.rs를 손대지 않는다. 사용자가 A2 확정.

**신규 프로젝트(E3S 미경유) 계약:** 새 코드는 src-relative import를 쓴다 —
평면은 `import module;` 또는 `import .module;`, 중첩은 `import sub.module;`.
`engine.rs:119`의 `TriggerEditor.feature` root-qualified 지침을 이 계약으로 교체한다(Phase 7.4).

## 5. 대상 아키텍처

```text
<project>/                                  ← CLI cwd (신규)
  project.eap
  src/**/*.eps                              ← CLI가 직접 읽음 (미러 없음)
  dat/*.json  maps/  build/  compat/  plugins/
  .eud-agent/
    workspace/                              ← 정본 문서 (변화 없음)
      specs/  plans/  decisions/  worklog/
      .tmp/<session-id>/                    ← 유일한 FS 쓰기 허용 영역 (신규 위치)
    state/workspace.json
    memory/

%APPDATA%/eud-agent/
  workspaces/.state/baselines/<request>/<wsid>/{documents,source}/   ← 소스 baseline 추가
  journal/                                                            ← session_id 필드 제거
  (workspaces/.sessions/** 는 폐기)
```

턴 준비:

```
읽기 턴  → cwd = <project>, sandbox = read 프로필, baseline 없음
쓰기 턴  → baseline 캡처(documents + source) → write 프로필 → 저널 → 검토
```

`sessions.md:296-299`의 "delta-sync + source snapshot refresh" 2단계가 **사라진다**.
정본이 곧 cwd이므로 동기화 대상이 없다.

샌드박스 프로필:

```toml
# read
permissions.eud_workspace_read = {
  filesystem = { ":minimal" = "read", ":workspace_roots" = { "." = "read" } },
  network = { enabled = false }
}
# write  ← .tmp 경로만 변경
permissions.eud_workspace_write = {
  filesystem = { ":minimal" = "read", ":workspace_roots" = {
    "." = "read",
    ".eud-agent/workspace/.tmp/**" = "write"
  } },
  network = { enabled = false }
}
```

## 6. 구현 단계

### Phase 0 — 선행: 생성물 제외 (D8)

**파일:** `src-tauri/src/native_project.rs`

1. `collect_text_files`(`:2116-2151`)에 디렉터리 이름 스킵 추가 — `__epspy__`, `__pycache__`
   (대소문자 무시). 기존 symlink/reparse 거부 검사 **이전**에 위치시켜 재분석 지점 경고가
   생성물 때문에 발동하지 않게 한다.
2. `source_snapshot`(`:657-678`)·`source_snapshot_without_revision`(`:1110-1123`)·
   `list_source_files`(`:644-649`)는 1을 통과하므로 자동 반영된다.
3. `has_direct_python`(`:611-619`)이 `__epspy__` 때문에 true가 되지 않음을 테스트로 고정.
4. 기존 프로젝트의 잔여 `src/__epspy__/`·`src/__pycache__/`는 앱이 삭제하지 않는다
   (사용자 파일). 스냅샷·목록·검색·리비전·export에서 보이지 않으면 충분하다.

**검증:** `E:\proj\eud\proj1\native`에서 `list_files`가 `.eps` 9개만 반환,
`export_e3s`가 Python 거부 없이 통과, 빌드 전후 `revision()` 불변.

### Phase 1 — 스캔 allowlist (D2)

**파일:** `src-tauri/src/workspace.rs`

1. `ScanMode`에 프로젝트 루트용 변형을 추가하거나 `scan_directory`(`:2560-2632`)에
   루트 상대 allowlist 프레디케이트를 도입한다. `top` 판정(`:2578`)을 확장:
   - `.eud-agent` → `workspace/{specs,plans,decisions,worklog}` 하위만 재귀
   - `.eud-agent/workspace/.tmp` → 스킵
   - 그 외 전부(`src`, `dat`, `maps`, `build`, `compat`, `plugins`, `project.eap`) → 스킵
2. 반환 경로는 **정본 기준 상대 경로**로 정규화한다 — `.eud-agent/workspace/specs/x.md`가
   아니라 `specs/x.md`. 저널·패널·changeset이 기존 경로 표기를 계속 쓰도록 하기 위함.
   변환 지점은 스캔 진입부 한 곳으로 제한한다.
3. 1 MiB / 32 MiB / 2048 한도는 유지한다. allowlist가 바이너리를 배제하므로
   한도 초과 경로가 사라진다.

**검증:** 프로젝트 루트에 `compat/editor-project.e3s`(1.52 MiB)와 `maps/*.scx`가 있는 상태로
`begin_turn`이 성공하고, baseline 트리에 문서 143개만 포함됨을 단정.

### Phase 2 — 소스 baseline 캡처 (D4)

**파일:** `src-tauri/src/workspace.rs`, `src-tauri/src/tool_exec.rs`

1. `begin_turn`(`:1211-1246`)이 문서 baseline과 함께 소스 baseline을
   `<baseline_root>/source/`에 캡처한다. 소스는 `NativeProject::source_snapshot()`의
   정본 `.eps`/`.py`(Phase 0 이후 생성물 제외)를 `src/` 접두 없이 쓴다.
2. `read_source_baseline`(`:2230-2241`)을 새 위치 기준으로 교체한다.
   시그니처는 `(baseline_root, relative)`로 바뀐다 — `workspace_root`를 더 이상 받지 않는다.
3. `tool_exec.rs:1019-1028`의 `source_baseline()`이 request 상태의 `workspace_root` 대신
   **baseline 루트**를 찾도록 변경한다. `bind_workspace_root`(`:1005-1017`)가
   baseline 루트도 함께 보관하거나 별도 바인딩을 추가한다.
4. baseline이 없는 읽기 전용 턴에서 소스 도구가 호출되면 기존과 동일하게
   "the current request has no prepared session workspace" 계열 오류를 반환한다
   (메시지는 baseline 기준으로 갱신).

**검증:** 쓰기 턴에서 `file_write` stale 검사·3-way merge가 미러 없이 동일하게 동작.
`workspace.rs:3048-3200`대 동시성 테스트(병합/충돌)를 새 baseline 구조로 이전.

### Phase 3 — cwd 전환 (D1, D6)

**파일:** `src-tauri/src/workspace.rs`, `provider_runtime/runtime/state.rs`,
`src-tauri/src/codex_client.rs`, `claude_client/adapter/*`

1. `PreparedWorkspace.root`를 `<project>` 루트로 변경한다.
   `prepare_snapshot`(`:1034-1129`)이 이미 `project_root`를 canonicalize하므로
   그 값을 그대로 사용하고, `PROJECT_AGENT_DIR/PROJECT_WORKSPACE_DIR` 하위 생성 로직은 유지.
2. `prepare_session_snapshot`(`:1139-1165`)에서 `session_workspace_root`·`sync_documents`·
   `refresh_source` 호출을 제거한다. `session_id`는 `.tmp` 하위 경로 구성에만 사용.
3. `refresh_source`(`:1721-1767`)와 `SOURCE_DIR`(`:27`) 삭제.
4. `.tmp` 위치를 `<project>/.eud-agent/workspace/.tmp/<session-id>/`로 변경하고
   `codex_client.rs:1050-1053`의 `TEMP`/`TMP` 경로를 맞춘다.
5. 샌드박스 프로필 2종을 §5 형태로 갱신한다. `codex_client.rs:1701-1703` 테스트를
   새 경로 표기로 교체.
6. `session_workspaces_dir()`(`config.rs:367-369`)는 Phase 6 정리까지 남긴다.

**검증:** 실제 Codex 앱서버를 띄워 cwd가 프로젝트 루트임을 확인하고,
CLI가 `project.eap`·`src/survivor_mvp.eps`를 FS 도구로 직접 읽음을 관찰.
`.tmp` 외 경로 쓰기가 샌드박스에서 거부됨을 확인.

### Phase 4 — 저널·거부 경로 정리 (D5)

**파일:** `src-tauri/src/journal.rs`, `src-tauri/src/workspace.rs`, `src-tauri/src/tool_exec.rs`

1. `JournalTarget::WorkspacePath.session_id` 제거(`journal.rs:166-169`).
   `#[serde(default)]`를 유지해 기존 기록을 읽을 수 있게 한다.
2. `reject_targets`(`:1157-1199`)·`workspace_target_parts`(`:1419-1430`)·
   `RejectTarget::WorkspacePath`에서 `session_id` 분기 제거.
3. `restore_file`(`workspace.rs:1519-1539`)의 `session_id` 파라미터와
   `session_workspace_root` 분기 제거 → 항상 정본.
4. `promote_entries`(`:1541-1623`)에서 `session_id: Some(_)` 필터(`:1551`)를 제거하고
   `base`를 저널 `before` 스냅샷에서 계속 가져온다. 3-way merge는 유지.
5. `write_workspace_file`/`delete_workspace_file`(`tool_exec.rs:3571-3592`)의
   `session_id` 인자 제거.
6. `normalize_relative_path`(`workspace.rs:2418-2483`)의 `SOURCE_DIR` 분기(`:2472-2481`) 제거.
   `.tmp` 스킵은 유지.

**검증:** 문서 변경 → changeset 표시 → Reject 시 정본이 정확히 복원,
Accept 시 병합, 두 세션의 비겹침 변경 자동 병합, 겹침 변경 `ConcurrentWriteConflict`.

### Phase 5 — 패널·IPC 정리 (D3)

**파일:** `src-tauri/src/ipc.rs`, `panel/src/**`

1. `WorkspaceFileEntry.source`(`panel/src/lib/protocol.ts:80-83`) 필드 제거.
   `ipc.rs:905-906`의 `SOURCE_DIR` 판정 제거.
2. `workspace_list`(`ipc.rs:873-893`)는 정본 문서만 반환한다.
3. 패널 `WorkspaceDocument`의 "읽기 전용 소스" 분기
   (`WorkspaceDocument.test.tsx:11-13,46-50`, `WorkspaceFileTree.test.tsx:11-13`) 제거.
   소스 열람은 기존 EPS 소스 뷰를 사용한다.
4. `panel/src/lib/ipc.test.ts:1169-1174`의 `source` 검증 테스트를 새 스키마로 교체.
5. `workspace_search`(`ipc.rs:919-923`)는 문서 범위 유지 — 소스 검색은 `source_search` 담당.

**검증:** 패널 Files 탭에서 문서만 표시, 소스 미리보기가 기존 EPS 경로로 동작,
`npm test` 통과.

### Phase 6 — Claude 경계 + 이전 데이터 정리 (D7)

**파일:** `src-tauri/src/claude_client/adapter/request.rs`, `src-tauri/src/workspace.rs`,
`src-tauri/src/config.rs`

1. `validate_workspace_boundary`(`request.rs:47-54`)를 §D7 표대로 분리한다.
   `CLAUDE.md`/`CLAUDE.local.md` 허용, `.claude/settings*.json`·`.claude/` 기타·`.mcp.json` 거부.
   거부 메시지에 **어떤 파일이 왜** 막혔는지 명시.
2. 부팅 시 `%APPDATA%/eud-agent/workspaces/.sessions/**`를 정리한다.
   정본 문서는 이미 `<project>/.eud-agent/workspace/`에 있으므로 세션 사본은
   미수락 초안이며 폐기 대상(`sessions.md:293`).
   **미결 저널이 있는 경우**에는 해당 `request_id`의 baseline을 보존하고 정리에서 제외한다.
3. `session_workspaces_dir()`(`config.rs:367-369`)와 `ensure_dirs` 등록(`:542`) 제거.
4. `sessions.md`의 "Workspace isolation" 절, `sessions-write-queue-plan.md:219-243`의
   세션 루트 다이어그램, `architecture.md:59`의 "read-only source mirror" 기술을 갱신.

**검증:** 업그레이드 후 기존 프로젝트의 문서 143개가 정본에 그대로 있고
`.sessions/`이 사라짐. `CLAUDE.md`가 있는 프로젝트에서 Claude 턴이 시작되고,
`.claude/settings.json`이 있으면 명시적 거부.

### Phase 7 — E3S import 시 epScript import 재작성 (D9, A2)

사용자 2번 질문("가져올 때 해결 가능한가")의 답. **가능하다 — import 시점 재작성으로.**
§3.6에서 원본 코드로 확정한 규칙에 근거한다.

**파일:** `src-tauri/src/e3s_nrbf.rs`

1. `collect_te_sources`(`:613-700`)가 이미 TE 폴더 트리를 순회하므로, `IsTopFile=true`
   폴더명(`:626`)을 **import prefix 후보 집합**으로 수집한다. 이 프로젝트는 `TriggerEditor` 하나.
2. EPS 본문(`:664-673`에서 읽는 `_String`)을 저장하기 전에 import 문을 재작성한다:
   - epScript `import <prefix>.<rest> [as <alias>];` 형태만 매칭 (토큰 경계 검사).
   - `<prefix>`가 후보 집합에 속하면 `import <rest> [as <alias>];`로 교체.
   - 문자열 리터럴·주석 안의 동일 텍스트는 보존.
   - 재작성 카운트를 import 결과(`ImportOutcome`/`importIssues`)에 기록해
     사용자가 "N개 import를 src-relative로 변환"을 볼 수 있게 한다.
3. 후보 폴더명을 `editorCompatibility`에 보존해 export 역재작성에 쓴다.
4. **export 역재작성:** `apply_sources`(`:1240-1304`)가 native `src/<rest>`를 E3S TE 노드로
   되돌릴 때 본문 import를 `<prefix>.<rest>`로 복원한다.
   `real_fixture_import_export_import_is_semantically_stable`(`:2051`) round-trip 유지.
5. **기존 수입 프로젝트:** 이미 import된 프로젝트(`E:\proj\eud\proj1\native`)는 본문이
   `TriggerEditor.*`로 남아있다. 재수입은 destructive하므로, 별도 **마이그레이션 도구/턴**으로
   기존 `src/**.eps`의 `TriggerEditor.` 접두를 제거한다. (이 프로젝트는 이미 사용자가
   상대 import로 수동 수정해 `import .X` 형태 — §3.6 매트릭스에서 PASS.)

**검증 (V12):** E3S 픽스처 import → 모든 `src/**.eps` 본문에 `TriggerEditor.` 0건,
`build_run` 성공, export → 재import 시 본문 byte-동일.

### Phase 8 — trace 하네스 결함 + euddraft 무결성 + 프롬프트 계약

이 전환과 독립적이지만 **인게임 검증에 필요**하므로 함께 처리한다.

1. **trace 결함 1** — `prepare_build`(`trace_test.rs:643-741`)가 `<project>/build/*`만 복사하므로
   복사 트리에 `<run>/src`가 없다. EDS의 main 섹션 `[../../src/survivor_mvp.eps]`가
   `<run>/src/survivor_mvp.eps`로 해석되어 `FileNotFoundError` → `test_build_failed`.
   실증: `%LOCALAPPDATA%/eud-agent/logs/trace-tests/trace-915106b9…/build.log:33-34`.
   두 선택지 중 **(b) 채택** — `<project>/src`를 `<run>/src`로 복사한다. 그러면 EDS의 기존 상대
   섹션 `../../src/<main>.eps`가 **재작성 없이** 그대로 해석된다(`copy_tree`에 `__epspy__` 스킵
   추가 — `__pycache__`는 이미 스킵, `:755-761`). 정본 소스 135 KiB라 비용 무시 가능.
   (a) 절대경로 재작성은 §3.6에서 동작함을 실측했지만, trace 빌드가 **라이브 src**를 읽으므로
   동시 쓰기 턴과 겹치면 혼합 상태를 컴파일한다 — 하네스의 격리 계약
   (`engine.rs:122` "builds and runs an isolated map copy")과 어긋난다. 결정성은 (b) 우위.
2. **trace 결함 2** — `PERSISTENT_TEST_ROOT`(`:39`)를 `src/tests/`로 변경하고
   단위 테스트 경로(`:1801,1806,1924-1933,1960-1964,1983,2193,2218`),
   `tools.rs:1065` 설명문을 갱신한다.
   스냅샷은 `src/`만 담으므로(`native_project.rs:646`) `tests/` 접두는 영구히 매칭 불가.
3. **trace 결함 3** — `native_trace_runtime`(`tool_exec.rs:4344`)이 config의
   `starcraft_path`(현재 `""`)를 그대로 써서 `Path::new("").parent() == None`으로 실패한다.
   `map_context::resolve_starcraft_path()`(`map_context.rs:209-229`)로 교체해
   `C:\Program Files (x86)\StarCraft` 폴백을 공유한다.
4. **프롬프트 계약 (D9 연계)** — `engine.rs:119`의 `TriggerEditor.feature` root-qualified
   지침을 src-relative import 계약으로 교체한다(평면 `import module;`/`import .module;`,
   중첩 `import sub.module;`). §3.6에서 `src.` 절대 import는 EDS 위치 의존이라
   권장하지 않음을 확정. 현재 지침이 최근 두 세션 빌드 실패 5회의 출발점이었다.
5. **euddraft 설치 무결성** — `EuddraftLaunch::resolve`(`native_build.rs:255-298`)이
   `euddraft.exe` 존재만 본다. `bootstrap::validate_euddraft_install`(`bootstrap.rs:1423`)을
   빌드 직전에도 적용해 손상 설치를 명확한 오류로 거부한다.
   실증: `sha256-87113da1…`(v0.10.2.5)은 선언 77개 중 **10개 누락**
   (`epTrace.exe`, `python3.dll`, `concrt140.dll`, `msvcp140*.dll`, `vcruntime140*.dll` …),
   `_update/VERSION = 0.11.0.1` — euddraft 자체 updater가 `del` → `xcopy` 중간에 끊긴 잔해.

## 7. 검증 계획

| # | 검증 | 방법 |
|---|---|---|
| V1 | 스캔 allowlist가 바이너리/대용량을 배제 | 루트에 1.52 MiB `.e3s`+`.scx` 있는 픽스처에서 `begin_turn` 성공, baseline에 문서만 |
| V2 | 소스 baseline이 stale 검사를 지탱 | 쓰기 턴에서 외부 수정 후 `file_write` → 충돌 거부 |
| V3 | cwd가 프로젝트 루트 | 실제 Codex 앱서버로 `pwd`·`project.eap` 읽기 관찰 |
| V4 | 샌드박스 범위 | CLI가 `src/` 읽기 성공, `src/x.eps` FS 쓰기 거부, `.tmp/` 쓰기 성공 |
| V5 | 경로 이원화 소멸 | CLI가 읽은 경로 `src/main.eps`를 `read_file`에 그대로 전달해 성공 |
| V6 | Reject 정본 복원 | 문서 변경 → Reject → 정본 바이트 동일 |
| V7 | 동시 세션 | A 쓰기 중 B 읽기 턴 완료, 비겹침 병합, 겹침 `ConcurrentWriteConflict` |
| V8 | 생성물 제외 | `list_files`가 `.eps` 9개만, 빌드 전후 `revision()` 불변, `export_e3s` 통과 |
| V9 | 패널 | Files 탭 문서만, `source` 필드 제거 후 `npm test` 통과 |
| V10 | trace 하네스 | `trace_suite_run({})`이 `src/tests/**`를 발견하고 1건 이상 passed |
| V11 | 이전 데이터 | 업그레이드 후 문서 143개 보존, `.sessions/` 제거, 미결 저널 baseline 보존 |
| V12 | E3S import 재작성 (A2) | E3S 픽스처 import → `src/**.eps` 본문 `TriggerEditor.` 0건 + `build_run` 성공 + export→재import byte-동일 |

Rust 전체 스위트·clippy·fmt는 마지막에 1회만 실행한다.

## 8. 수용 기준

1. Provider CLI의 cwd가 `project.eap`가 있는 디렉터리다.
2. `source/` 미러와 `.sessions/` 세션 루트가 코드·디스크 양쪽에서 사라진다.
3. 매 턴 문서 복사(`sync_documents`)가 실행되지 않는다.
4. baseline 스캔이 프로젝트 루트의 바이너리·1 MiB 초과 파일을 읽지 않는다.
5. 소스·문서 변경이 계속 저널·changeset·Reject/Undo·write lease를 통과한다.
6. `src/__epspy__/`·`src/__pycache__/`가 스냅샷·목록·검색·리비전·export에 나타나지 않는다.
7. `trace_suite_run({})`이 `src/tests/**/*.tests.eps`를 발견해 실행한다.
8. E3S import가 `TriggerEditor.*` 접두를 src-relative로 재작성하고 export가 역재작성해 round-trip을 보존한다.
9. 문서(`sessions.md`, `architecture.md`, `04_native-project-surface.md`,
   `sessions-write-queue-plan.md`)가 새 계약을 기술한다.

## 9. 위험과 완화

| 위험 | 완화 |
|---|---|
| allowlist 누락으로 baseline이 바이너리를 읽어 턴 시작 실패 | Phase 1을 단독 커밋으로 먼저 넣고 V1을 통과시킨 뒤 진행 |
| 소스 baseline 캡처 비용 증가 | 실측 정본 `.eps` 9파일 135 KiB(138,677 B) — 오늘 미러가 복사하는 514 KiB(`src/` `.eps`+`.py` 18파일 525,948 B)보다 작음 |
| 공유 정본에서 세션 간 문서 충돌 | write lease가 프로젝트 쓰기를 직렬화. 읽기 턴은 쓰지 않음 |
| 기존 저널의 `session_id` 호환 | `#[serde(default)]`로 읽고 무시. 미결 저널은 Phase 6에서 보존 |
| Claude ambient 설정으로 권한 상승 | `.claude/settings*.json`·`.mcp.json` 거부 유지 (D7) |
| 프로젝트 루트 읽기 개방으로 큰 바이너리 컨텍스트 낭비 | 읽기는 모델 선택. `MAX_STDOUT_BYTES`·도구 응답 한도가 기존대로 방어 |
| 이전 데이터 정리 중 미결 검토 유실 | `request_id` baseline 보존 후 정리 (Phase 6.2) |

## 10. 비목표

- CLI 네이티브 FS 쓰기 범위를 `.tmp` 너머로 확장 (D1)
- `src/` 단일 소스 루트 계약 변경 — `tests/`는 `src/tests/`로 들어온다 (Phase 8.2)
- E3S/NRBF export의 다중 소스 루트 지원
- Provider 5종 중 HTTP 기반 3종(OpenCode Go/Antigravity/Ollama)의 cwd 변화 — 영향 없음
- changeset 검토 UI 재설계
