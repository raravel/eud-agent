# Staged workflow scenario set

Companion to the [staged workflow plan](staged-workflow-plan.md), Phase 0. Eight fixed prompts
against one fixture project measure request routing and end state before and after the cutover.
The set is run manually per provider; results are recorded in `verify.md` under "Staged workflow
scenarios". Nothing here is automated.

## Fixture project

Create a new native project from the launcher at a disposable root (for example
`%TEMP%\eud-agent-scenarios\<date>`) with `E:\proj\eud\test.scx` as the source map, then replace
the generated sources with the files below through the project tree (not through the agent). The
source map MUST contain a location named `spawn`, player 1 as a human with a start location, and
player 2 as a computer. If `spawn` is missing, add it with `location_write` in a throwaway session
and start the record after that session is deleted.

Before the first scenario, `build_run` on the untouched fixture MUST succeed. Record the resulting
output map SHA-256 as the fixture baseline. Every scenario starts from a fresh copy of this
baseline (restore the three sources and delete `dat/*.json` overrides between rows).

`project.eap`

```json
{
  "schemaVersion": 2,
  "name": "Staged Workflow Scenarios",
  "sourceMap": "maps/test.scx",
  "outputMap": "build/test_EUD.scx",
  "mainFile": "src/main.eps",
  "settings": { "shufflePayload": false, "debug": false, "sectorSize": 15, "useCustomTbl": false },
  "plugins": [],
  "pythonEntryPoints": [],
  "pythonDependencies": [],
  "pythonLock": null
}
```

`src/main.eps`

```eps
import kills;
import spawn;

function onPluginStart() {
    kills.killsInit();
    spawn.spawnInit();
}

function beforeTriggerExec() {
    kills.killsUpdate();
    spawn.spawnUpdate();
}
```

`src/kills.eps`

```eps
const zergling = $U("Zerg Zergling");
// Never placed or spawned for any player: per-player kill store.
const killStore = $U("Terran Valkyrie");

function killsInit() {
    SetDeaths(AllPlayers, SetTo, 0, killStore);
}

function killsUpdate() {
    // Deliberate defect for S8: this counts player 1's OWN zergling deaths,
    // not zerglings killed by player 1, so the bonus never fires for a Terran player.
    if (Deaths(P1, AtLeast, 1, zergling)) {
        SetDeaths(P1, Subtract, 1, zergling);
        SetDeaths(P1, Add, 1, killStore);
    }
    if (Deaths(P1, AtLeast, 10, killStore)) {
        SetDeaths(P1, SetTo, 0, killStore);
        DisplayText("10 kills!");
    }
}
```

`src/spawn.eps`

```eps
const zergling = $U("Zerg Zergling");
const spawnLoc = $L("spawn");
// Never placed or spawned for any player: spawn timer in player 8's counter.
const timerStore = $U("Terran Battlecruiser");
const spawnIntervalFrames = 240;

function spawnInit() {
    SetDeaths(P8, SetTo, 0, timerStore);
}

function spawnUpdate() {
    SetDeaths(P8, Add, 1, timerStore);
    if (Deaths(P8, AtLeast, spawnIntervalFrames, timerStore)) {
        SetDeaths(P8, SetTo, 0, timerStore);
        CreateUnit(4, zergling, spawnLoc, P2);
    }
}
```

No `dat/*.json` override exists in the baseline.

Fixture verification (2026-09-17): the three modules compile with euddraft 0.10.2.5 against
`E:\proj\eud\test.scx` through a hand-written EDS equivalent to the native generator's output,
producing a fresh output SCX, once the `spawn` location exists (`test.scx` does not ship one; with
`Anywhere` substituted the full build passes). A `Kills(P1, AtLeast, 10, zergling)` replacement for
the S8 defect also compiles, so the S8 expected end state is reachable.

## Procedure

For each provider under test (at minimum one live provider; Codex and one OpenCode Go wire are the
acceptance targets in the plan):

1. Restore the fixture baseline.
2. Open a fresh EPS session bound to that provider with the default model and reasoning.
3. Send the prompt verbatim. Answer clarifying questions with the row's scripted answer only.
4. Approve a plan when one is presented, without feedback, unless the row says otherwise.
5. Accept the changeset when one is presented.
6. Record the row (format below), then delete the session.

Do not retry a failed row. A provider or transport failure is recorded as `blocked`, not as a
routing result.

## Scenarios

| id | prompt (verbatim) | scripted clarify answer | expected route (post-cutover) | expected artifacts | expected end state |
|---|---|---|---|---|---|
| S1 | 이 프로젝트에서 저글링은 언제 어디서 스폰돼? | — | `answer` | none | answer names `spawn.eps`, every 240 frames (about 10 seconds), 4 zerglings at `spawn` for player 2; no changeset |
| S2 | beforeTriggerExec랑 afterTriggerExec 차이가 뭐야? | — | `answer` | none | answer explains the two per-loop entry functions; no changeset |
| S3 | 저글링 스폰 주기를 10초에서 5초로 바꿔줘 | — | `direct` | changeset: `src/spawn.eps` | `spawnIntervalFrames` becomes 120 (or an equivalent single-site change); `build_run` ok; no plan or research file |
| S4 | 마린 체력을 60으로 올려줘 | — | `direct` | changeset: one `dat_patch` on units/Terran Marine HP | one DAT override; `build_run` ok; no plan or research file |
| S5 | 저글링 30킬마다 P1한테 파이어뱃 2기를 스타트 위치에 지원군으로 줘 | — | `pipeline` | `research/<id>.md`, `plans/<id>.md` (critic ran), verifier verdict | plan lists the kill counter defect or works around it, names the reinforcement location; changeset touches `kills.eps` or a new module plus `main.eps` import; `build_run` ok; verifier `pass` |
| S6 | 웨이브 시스템 만들어줘. 1분마다 웨이브가 오르고, 웨이브마다 저글링이 2마리씩 늘고, 5웨이브부터는 히드라도 섞여야 해 | — | `pipeline` | research, plan with at least three steps and a test, verifier verdict | new `wave.eps` (or equivalent) imported from `main.eps`, existing spawn logic replaced or coordinated, `src/tests/**` scenario created and `trace_suite_run` attempted; `build_run` ok; verifier `pass` |
| S7 | 밸런스 좀 맞춰줘 | "저글링이 너무 빨리 몰려와. 스폰 수를 줄여줘." | `clarify` → `direct` or `pipeline` | ASK with at least one question about what to balance; after the answer, a changeset on `spawn.eps` | the first response is a question, not a change; the CreateUnit count decreases; `build_run` ok |
| S8 | 10킬 보너스 메시지가 안 떠요 | — | `pipeline` | research naming `kills.eps` and the own-deaths-versus-kills cause, plan, verifier verdict | fix uses `Kills(...)` or the killed unit owner's death counter; `build_run` ok; verifier `pass`; answer cites the named cause before the fix |

Pre-cutover expectation by source: S1/S2 answer; S3–S6 and S8 execute directly with no plan,
research, critic, or verifier; S7 either asks through the `ask` tool or guesses. The run confirms
or corrects this.

## Record format

One row per scenario per provider, appended to `verify.md`:

```text
| run | provider/model | id | route observed | clarify | artifacts | tool calls | build | changeset | verdict | wall time | tokens | notes |
```

- `run`: `pre` or `post` plus the source SHA-256 prefix.
- `route observed`: `answer`, `direct`, `pipeline`, `clarify→…`, or `blocked`.
- `clarify`: the questions asked, or `—`.
- `artifacts`: research/plan/verdict paths that exist after the row.
- `tool calls`: total tool calls across all stages as shown in the panel.
- `build`: `ok`, `fail(n errors)`, or `—`.
- `changeset`: item count and paths.
- `verdict`: verifier verdict, or `—` before the cutover.
- `tokens`: provider-reported total only; never estimated.

A row passes when the observed route equals the expected route and every item in "expected end
state" holds. Partial matches are recorded as failures with the failing item in `notes`.
