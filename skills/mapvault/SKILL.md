---
name: mapvault
title_en: MapVault Integration
description: "Connect the map to MapVault, the player-data save/load site: create the map and a version through the maker API, define the saved fields, install the generated SDK, wire save/load into the map and build; later change fields, manage player data and release, with risky steps confirmed by the user."
---

# MapVault 연동

Activate when the user selects this skill. MapVault keeps players' data between games: the map shows a save code, the player's helper (or the site) turns it into stored data, and a later game loads it back through a restore key typed into chat. The maker defines a schema of saved fields per map version on the site; each version has its own secret key, built into the map through a generated SDK.

## Tools and hard rules

- `mapvault_request {method, path, body?, file?}` calls the maker API under `/api/v1` of the site set in Settings → MapVault, with the key stored there. `path` starts with `/`; `body` is the JSON body as text. `GET /` returns the full API reference (the site needs no key for it, but this tool still asks for one to be set): read it whenever a route, field or limit is unclear instead of guessing.
- `mapvault_sdk_install {map, version}` (map slug, version id) writes `src/mapvault.eps` and returns the accessors, usage and plugin text. It is the only way to get the SDK; `mapvault_request` refuses the `…/sdk/…` paths.
- No key set: tell the user to create an API key on the MapVault site (site settings) and enter it in Settings → MapVault. Never ask for the key in chat, never print one. A read-only key can only GET and cannot install the SDK (403 otherwise); say so if it happens.
- `src/mapvault.eps` holds the version's secret key. The app masks its key lines (`var rke…`/`var rkm…` round keys, the `T_SAVE`/`T_CH`/`T_KEY` tables and the self-test probe values) as `[masked]` in every read, search, diff and build output, so a masked line is expected, not damage; never try to recover them, never print or quote the file, never edit it. It is generated: to change it, change the version on the site and reinstall. `import mapvault;` loads it by this name; never rename it.
- The app shows the user a MapVault 확인 card before: releasing or blocking a version, uploading, linking or unlinking a map file, deleting a draft version, editing the map page, every player-data change (edit, reset, block/unblock, CSV import, deleting held data), and any unknown change. Do not ask again in chat first; one card is the confirmation. A refusal or expiry means do not retry unless the user asks again. When the user turned the card off (Settings → MapVault), such a request is sent at once and its result carries `autoApproved: true`: nobody saw it before it went out, so send only what the user asked for. Never release or upload unless the user asked for it.
- After a change that may have gone through but returned no answer (timeout, connection lost, or a `[mapvault requests interrupted]` note at the start of a turn), GET the resource before sending it again.
- In a read-only turn (plan feedback, an autonomous check) only GETs run; any other call returns `WriteWorkspaceTransition` and the turn resumes where it can write.

## Schema fields (`PATCH /maps/<slug>/versions/<id>` with `{"schema": [...]}`)

- Each field: `key` (`^[a-z][a-z0-9_]{0,31}$`, unique), `label` (1–40 characters), and either `max` (1..4294967295; values above it are saved as `max`) or `bits` (1–32; range 0..2^bits−1). `max` wins and sets the width. Optional `count` (1–256) makes an array; `rankable: true` (scalars only) makes the field a column of the public ranking table on the map's page, titled with its `label`; `rankSort: "desc" | "asc"` on one rankable field sets the order the table opens with (default: the first rankable field, largest first).
- At most 128 fields, and all fields together at most 4420 bits after packing; the answer's `stats` gives `bits`, `saveChars` (save code length, at most one screen of 500 symbols), `restoreKeys` and `restoreSeconds`. Prefer `max` that matches the game's real cap so codes stay short.
- The PATCH replaces the whole schema; send every field. Schema and `keyAlphabet` change only while the version is a draft, except `label`, `rankable` and `rankSort`, which a released version accepts too (send every other property unchanged). Leave `keyAlphabet` alone unless the user asks.
- In the map: `mapvault.d_<key>[p]` for a scalar, `mapvault.d_<key>[p * count + i]` for an array, `p` = player 0..7. The install result's `fields` lists the exact accessors.

## Version settings (same PATCH, on a version of any status, released ones too, with no card; reinstall and build afterwards)

- `screenLang`: `"ko"` (default) or `"en"`, the language of the save/load screens, notices and example menu inside the game. Codes and the `MapVault SAVE`/`MapVault LOAD` titles stay the same in both; the helper reads load screens in either language.
- `stream`: `"fast"` (21 channels), `"normal"` (8, default) or `"saving"` (4): how restore key lines travel between players over MSQC. MSQC keeps one unit per channel (plus one for ESC) for every human player, so fewer channels cost fewer units and load slower. It changes the `[MSQC]` lines, so update that plugin section from the new install result.
- `cameraLocation`: the name of a location in the map (up to 32 characters), or `""`. While the load screen is open the local player's camera is held there; use a location nothing else uses.

## First integration

1. `GET /me`, then `GET /maps`. Use the user's map if it exists; otherwise `POST /maps {slug, name, description?}` (slug: 3–32 lowercase letters, digits and `-`, not starting or ending with `-`, permanent; name 1–60). A map becomes public only when a version is released.
2. `GET /maps/<slug>` lists versions. Create one with `POST /maps/<slug>/versions {label}` (1–20 ASCII letters, digits, spaces and `. - _`, unique per map). It starts as a draft.
3. Agree the fields with the user (which values, their caps, arrays), then PATCH the schema, and `screenLang` when the map's players read English.
4. `mapvault_sdk_install {map, version}`.
5. In the main file: `import mapvault;`, `mapvault.init();` in `onPluginStart()` and `mapvault.tick();` in `beforeTriggerExec()` (add to existing functions; never define them twice).
6. Plugins: `plugins_list`, then make the map's plugins contain the install result's `plugin` text: `[eudTurbo]` (required: without it triggers and `tick` run only every ~16 frames) and the `[MSQC]` lines. The app emits plugins before the main file. Add each section as its own entry with `plugin_add`; if `eudTurbo` or `MSQC` already exists, keep one section and add the MapVault lines to it with `plugin_edit` instead of a duplicate. Never remove the map's own MSQC lines.
7. Wire the calls the user wants into the map's own UI or triggers (a menu, a key, a unit at a location, a chat command):
   - `mapvault.save(p)` shows player p a save code; `mapvault.request_load(p)` shows the load challenge and the player's restore key lines arrive through chat. ESC closes either screen.
   - `mapvault.just_loaded(p)` is 1 once after a successful load; the restored values are already in the fields then. Apply them (stats, items, unlocks) there.
   - While `mapvault.busy(p)` is 1 (seconds, during a save or load) do not change p's fields.
   - MSQC inputs for a new menu key or click are the map's own; `F7` and `F8` belong to the MapVault helper and must not be used by the map.
   - `mapvault.self_test()` prints OK/FAIL against the server's test vectors; offer it for a first in-game check.
8. `build_run` in the same turn and fix every error. Tell the user how players save and load in the game, and that the version stays a draft until released.

## `mapvault.origin(p)` (after a load)

0 not loaded yet · 1 the map file registered for the version · 2 a different file · 3 loaded on the site without the helper (not checked) · 4 the helper could not check (file not found, or no file registered). Act only on 2, and gently (a notice, not a defeat), unless the user explicitly wants more: 1 can be faked by a modified helper, and players with an older build of a re-uploaded version also get 2.

## Changing fields later

- Draft version: PATCH the full new schema, reinstall the SDK, update the map's uses of `mapvault.d_<key>`, build.
- Released version: its fields are fixed (only `label`, `rankable` and `rankSort` can still be PATCHed). Create a new version with `POST /maps/<slug>/versions {label, copyFrom: <old id>}` (copies schema, key alphabet, stream and screen language; not the camera location), PATCH it, reinstall, build; release it when the user decides. Explain this to the user before doing it.
- Existing players keep their data: it is stored per map, by field key, and fitted to the schema of the version that loads it. Same key: carried over, clamped to the new range (arrays padded with 0 or cut). New key: 0. Renamed key: starts at 0 (the old value stays under the old key). Removed key: kept on the site, not loaded. A save writes the loaded version's fields back, so a clamp or a cut array becomes permanent once the player saves in that version. Point out data that a narrower `max`/`bits` or shorter `count` would cut.

## Releasing (only when the user asks)

1. `build_run` must succeed for the current revision.
2. Upload: `mapvault_request {method: "POST", path: "/maps/<slug>/versions/<id>/file", file: <the project's output map>}`. It goes to scmscx.com, public and permanent (the site takes up to 300 MB, 5 uploads per 10 minutes). One tool call plans for 1 MiB/s and takes files up to about 185 MB; for a larger file the tool refuses before asking, and the user uploads it on the version page. A 502 with `scmscxId` is not always safe to link: link it with `PUT …/file {"address": "<scmscxId>"}` only when the error says the file reached scmscx but could not be verified ("The file reached scmscx (id …) but could not be verified" / "scmscx에 올라갔지만(id …) 확인하지 못했습니다"), or, when it says the version to link it to is gone ("… but the version to link it to is gone" / "… 연결할 버전이 없어졌습니다"), to the version the user names instead; never when it says the map with that id differs from the file sent ("scmscx answered with id …, but that map differs from the file you sent" / "scmscx가 id …로 답했지만 그 맵은 보낸 파일과 다릅니다": it is another map). The site answers in the app's language. A map already on scmscx.com can be linked the same way.
3. `POST /maps/<slug>/versions/<id>/status {"status": "active"}`: the map becomes public (its `publicUrl` from `GET /maps/<slug>`) and the schema is fixed for good.
- A released version is never re-uploaded with a new build: players with the earlier file would get origin 2. Make a new version.
- `{"status": "blocked"}` stops a released version (sending `active` again reopens it); a draft cannot be blocked, only deleted (`DELETE …/versions/<id>`).

## Player data (on the user's request)

- Look up: `GET /maps/<slug>/players?q=<nickname part>&page=<n>` (also lists held CSV data under `waiting`), `GET /maps/<slug>/players/<uid>` (values and the latest 200 history entries).
- Ranking: `GET /maps/<slug>/ranking?sort=<key>&dir=<desc|asc>&page=<n>` returns what the map's page shows (`ranking` is null without a released version or a rankable field; 50 rows a page; players who hid themselves or are blocked are left out). Mark fields `rankable` only when the user asks for a ranking: it shows every player's nickname and those values to everyone.
- Edit: `PATCH /maps/<slug>/players/<uid> {"changes": [{"key", "op": "set" | "add", "amount", "index"?}], "note"?}` (add may be negative; results are clamped). Reset: `POST …/players/<uid>/reset {"note"?}` (cannot be undone). Block or unblock: `POST …/players/<uid>/block {"blocked": true | false, "note"?}`.
- Bulk: `POST /maps/<slug>/import {"text": "<CSV: nickname,key,…; arrays as key[i]>", "overwrite"?, "dryRun"?}`. Run it with `"dryRun": true` first (no card, nothing changes) and show the user the result. Held entries are handed over when that nickname next saves or loads; `DELETE /maps/<slug>/waiting/<id>` drops one.
- Report results in plain terms (nickname, field, old → new); never invent values.
