# Map properties, 12-slot setup, CHK text encoding, SCMDraft 2 handoff

Status: in progress (2026-09-21). Scope is the four requests from the same session:

1. The blank-map wizard shows all 12 CHK player slots and lets each one be set (human /
   computer / rescuable / neutral / unused / closed, race, force for P1..P8), like SCMDraft 2.
2. Scenario properties (title, description, 12 slots, 4 forces) can be edited again at any time
   from the Map window ("맵 속성"), saved straight to the source map through the existing
   MapSafe rails, and undone with the Map window's Undo.
3. A "SCMDraft 2로 열기" button in the main-window header launches the configured SCMDraft 2
   executable on the source map; the path lives in Settings, and an unset path opens
   설정 → 컴파일 directly.
4. Bug: Korean force names written by the wizard show garbled in SCMDraft 2.

## Root cause of the encoding bug

`MapFile(tileset, w, h)` creates a legacy `STR ` string table (no `STRx`), and the wizard wrote
title/description/force names as raw UTF-8 into it. SCMDraft 2 (0.9.10) decodes every string
with the system code page (CP949 on Korean Windows), so UTF-8 Hangul displays as mojibake. The
app already has the correct rule for location names (`encode_location_name`): ASCII as-is,
`STRx` maps UTF-8, legacy `STR ` maps CP949. The wizard and the new property ops now share that
rule through `chk::encode_chk_text(text, has_strx)`; text CP949 cannot represent falls back to
UTF-8 (SC:R still renders it, SCMDraft 2 cannot).

## Frozen wire contracts

### `eud-map-new/1` (native `mapNew`, `isom::MapNewSpec`)

- `titleBytesHex` (1..1024 bytes) and `descriptionBytesHex` (0..4096 bytes) replace `title` /
  `description`; `forces[].nameBytesHex` (1..256 bytes) replaces `forces[].name`. Bytes are
  already encoded by Rust; native writes them verbatim (`RawString`).
- `players`: 0..12 entries, `slot` 0..11, unique. `force` (0..3, must name a declared force) is
  required for slot < 8 and must be absent for slot >= 8 (FORC has 8 entries). `start` is only
  allowed for slot < 8.
- `race` adds `neutral` (Chk::Race::Neutral = 4) and `inactive` (Chk::Race::Inactive = 7).
- Every slot 0..11 the spec does not mention becomes `Inactive` (was 0..7).
- Report `players` still counts spec entries; `startLocations` counts `start` fields.

### `eud-map-edit/1` new operations (native `mapEdit`, `map_model::MapOperation`)

- `scenario.set { op, titleBytesHex?, descriptionBytesHex? }` — at least one field; title
  1..1024 bytes, description 0..4096 bytes. Effect `{op, layer: "scenario", ordinal: 0}`.
- `player.set { op, slot: 0..11, type?, race?, force? }` — at least one of type/race/force;
  `type` in human|computer|rescuable|neutral|inactive|closed (OWNR + IOWN), `race` in
  zerg|terran|protoss|userSelectable|random|neutral|inactive, `force` 0..3 only for slot < 8.
  Effect `{op, layer: "players", ordinal: slot}`.
- `force.set { op, force: 0..3, nameBytesHex?, allied?, alliedVictory?, sharedVision?,
  randomStart? }` — at least one field; name 1..256 bytes. Flags are read-modify-write.
  Effect `{op, layer: "forces", ordinal: force}`.
- These ops are not advertised in the Map agent's `map_edit` tool schema; only the Map window's
  properties request emits them.

### Rust

- `chk.rs`: `MAP_PROPERTY_SECTIONS = [SPRP, OWNR, IOWN, SIDE, FORC]`, `MapHeader.title` /
  `.description` (SPRP), `Player.controller_id` / `.race_id`, `encode_chk_text`, `chk_has_strx`.
- `map_verify.rs`: `MapRequestAuthority.properties: bool` (serde default false). Changes to
  `MAP_PROPERTY_SECTIONS` are unsupported unless `properties` is true; `MapDiff.properties`
  (serde default 0) counts changed title/description/slot/force fields from the digests.
- `MapAgentService::properties_save(MapPropertiesSaveCommand { session_id, properties })`:
  refuses while the session has a candidate revision, a running request, or the native project is
  building; validates the baseline hash; diffs the request against the current digest and builds
  only changed ops (none → error); runs `isom::mapedit` source → session work file; verifies with
  a properties authority; checks the work digest equals the request; then `MapSafe::apply`
  (lock probe, backup, verify, atomic replace) + `complete_apply` so Undo works. Emits
  `map_apply_result`.
- `config.scmdraft_path` + `settings_pick_scmdraft_path` (exe picker) + `project_open_scmdraft`
  (spawns the exe with the project's source map path; returns `unconfigured` when the path is
  unset or not a file so the panel opens 설정 → 컴파일).

### Panel

- `lib/mapNew.ts`: slot types add `inactive` ("사용 안 함"); races add `neutral`, `inactive`;
  `players` is always 12 entries and `forces` always 4 (the "플레이어 수"/"포스 수" selects go
  away, the layout presets stay). Force column is "—" for P9..P12. Defaults: P1..P8 human /
  userSelectable, P9..P11 inactive / inactive, P12 neutral / neutral.
- Shared `PlayerSlotsEditor` + `ForcesEditor` components used by the wizard and by the Map
  window's `MapPropertiesDialog` (Tabs: 기본 / 플레이어 / 포스). The dialog is disabled while a
  candidate revision exists and explains why.
- `MapToolbar`: "맵 속성" button; `Header`: "SCMDraft 2로 열기" next to "맵 에이전트";
  Settings → 컴파일 gains the SCMDraft 2 path picker.

## Verification

Rust full suite (native map-new with 12 slots and CP949 force-name bytes, verifier property
sections, service save/undo), panel Vitest + TypeScript build, and a real SCMDraft 2 open of a
wizard map with a Korean force name. Results go to [verify.md](../verify.md).
