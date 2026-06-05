# Bridge v2 Surface (full editor-model command set)

Expands the Lua bridge from the v1 instruct/apply set (LIST/GET/SET/NEWEPS/GETDAT/SETDAT/BUILD/...) to the full editor-model surface the v2 agent tools need. Grounded in [[capability-survey]] `../capability-survey.md` (multi-agent source read of EUD Editor 3, 2026-06-05): every command below maps to a verified Public model path; none touch UI code-behind. Import-then-extend: v6 paths stay intact; new commands are added to the same dispatcher.

## Scope decisions (single path)

- SCA is fully defunct (incl. the SCAScript file type): settable/creatable text types are **CUIEps, CUIPy, RawText** only. `_SETTABLE_FAMILIES` on the server drops `"SCA"`.
- GUI/GUIPy/ClassicTrigger remain read-only via GET (eps projection). SET/NEWFILE on them is structurally rejected by FileType pre-check in the bridge (assignment would THROW — `StringText` does not exist on those classes; never rely on pcall).
- The agent authors epScript text only; `main` is whatever `pj.TEData.MainFile` references — SETMAIN points it at any CUIEps file.
- Editor BUILD stays the build path (server-side euddraft re-run is only the error-capture fallback, see [[features/05_agent-core|05_agent-core]] `05_agent-core.md`).

## New / changed commands

All commands keep the v6 file-IPC transport (`inbox/srv-*.cmd` → UI-thread Tick → `outbox/*.result`, UTF-8 no BOM). Multi-line or non-ASCII values travel in the body (from 2nd line), never in the pipe-separated arg line. Enum args are built bridge-side from `import_type` enum objects — never raw ints.

### DAT surface (B1)

| Command | Args / body | Model path | Notes |
|---|---|---|---|
| GETDAT / SETDAT | dat\|param\|objId(\|value) | `pj.BindingManager:get_DatBinding(enum,param,objId).Value` | resolver REPLACED: bridge-local name→enum table over `SCDatFiles+DatFiles` covering units, weapons, flingy, sprites, images, upgrades, techdata, orders, **portdata, sfxdata** (bypasses `GetDatFileE`'s 8-name whitelist). Numeric values validated server-side before send. |
| GETXDAT / SETXDAT | dat\|name\|objId(\|value) | `get_ExtraDatBinding(enum,name,objId).Value` | dat ∈ {statusinfor, wireframe, ButtonSet}; name per survey (Status/Display/Joint, wire/grp/tran, ButtonSet). Byte-backed setters swallow bad values — bridge reads back `.Value` and returns it so the server can verify. Null binding (out-of-range grp/tran) → ERROR. |
| GETTBL / SETTBL | index (+ value in body) | `get_StatTxtBinding(index).Value` | unit/string names. Value in BODY (UTF-8 .NET read — Korean safe). `NULLSTRING` body resets to default. Server bound-checks index. |
| RESETDAT | kind\|dat\|param-or-name\|objId | `binding:DataReset()` | kind ∈ {dat, xdat, tbl}; restores stock value, used by changeset reject of a previously-default field. |
| GETREQ / SETREQ | dat\|objId (+ payload in body) | `get_RequireDataBinding(objId,enum)` + `CRequireData GetCopyString/PasteCopyData` | dat ∈ {units, upgrades, techdata, Stechdata, orders}. SETREQ body = the editor's own copy-string format (`CustomUse.op,val.…`) or a use-mode keyword (Default/Dont/Always/AlwaysCurrent) — stays inside the validated PasteCopyData path. |
| GETBTN / SETBTN | setId (+ csv body) | `pj.ExtraDat.ButtonData:GetButtonSet(id)` `GetCopyString/PasteFromString` | full button-table round-trip in the editor's own CSV format; bridge calls `pjData:SetDirty(true)` after SETBTN (direct mutations don't auto-dirty). Malformed CSV → ERROR (8-field check bridge-side before Paste). |

### File tree (B2)

| Command | Args / body | Model path | Notes |
|---|---|---|---|
| NEWFILE | path\|type (+ body) | `TEFile(name, EFileType.<T>)` + `parent:FileAdd` | generalizes NEWEPS. type ∈ {CUIEps, CUIPy, RawText}. `path` may include folders (`folder/sub/name`); missing folders auto-created via FolderAdd. Duplicate path → ERROR (decision 02 generalized). NEWEPS kept as alias for compat. |
| MKDIR | path | `TEFile(name, EFileType.Folder)` + `FolderAdd` | nested ok; duplicate → ERROR. |
| RENAME | path (+ newname in body) | `f.FileName = newname` then parent `FileSort/FolderSort` | rejects top node, Setting node, duplicate sibling name. |
| DELFILE | path | walk-located parent `:FileRemove/:FolderRemove` + `pjData:SetDirty(true)` | rejects top/Setting nodes; if target IS MainFile, clears `MainFile` first and the result notes `main-cleared`; bridge also closes an open tab via `WindowControl.TECloseTabITem` when present. |
| MOVEFILE | path (+ destFolder in body) | `oldParent:FileRemove` + `dest:FileAdd` (same instance) | preserves MainFile identity; rejects move into Setting/top. |
| SETMAIN | path | `pj.TEData.MainFile = <node>` | node must exist (walk); `GETMAIN` (no args) returns current main path or empty. |

### Settings & plugins (B3)

| Command | Args / body | Model path | Notes |
|---|---|---|---|
| GETSET / SETSET | scope\|key (+ value in body) | project: plain `pjData` props; program: `pgData:get_Setting/set_Setting(TSetting enum)` + `SaveSetting()` flush | whitelists — project: OpenMapName, SaveMapName, AutoBuild, UseCustomtbl, ViewLog, TempFileLoc; program: euddraft, starcraft (read/write), Language (read). Anything else → ERROR (no theme/UX chrome). |
| PLUGLIST | — | `pjData.EdsBlock.Blocks` walk | one line per block: index TAB BType TAB first-line-of-Texts. |
| PLUGADD | index (+ Texts body) | `EdsBlockItem(EdsBlockType.UserPlugin)` + `Blocks:Insert` | index=-1 appends; `SetDirty` after. |
| PLUGSET | index (+ Texts body) | `Blocks:get_Item(i).Texts = body` | UserPlugin only → else ERROR. |
| PLUGDEL | index | `Blocks:RemoveAt(i)` | UserPlugin only (built-ins auto-reinsert at build anyway) → else ERROR. |
| PLUGMOVE | from\|to | RemoveAt + Insert | reorder. |

### Build (B4)

| Command | Args / body | Model path | Notes |
|---|---|---|---|
| BUILD | — | `pj.EudplibData:Build(false)` | CHANGED: before Build, force `pj.TEData.SCArchive.IsUsed = false` (defunct service would block on a dead modal login) and preflight OpenMapName/SaveMapName/euddraft-path existence — missing → ERROR without invoking Build (avoids the editor's modal CheckBuildable dialogs). Returns `OK: started`. Completion = server polls `status.txt` compiling flag. |
| BUILDERR | — | `GlobalObj.macro.macroErrorList` walk | returns macro/eps errors accumulated by the last build (one line per entry); empty result = no macro errors recorded. Secondary euddraft-output capture is server-side (see 05_agent-core). |
| EDSPATH | — | `BulidPaths`-derived temp .eds path + `pjData.SaveMapName` | gives the server the artifact paths for the euddraft re-run fallback and output-map existence check. |

## Verification contract

- Static tests per command group in `server/tests/` (same pattern as `test_bridge_list_static.py`): dispatcher branch present, model API tokens pinned (e.g. `get_ExtraDatBinding`, `StatTxtBinding`, `MainFile`, `EdsBlock`), FileType pre-check guard present for SET/NEWFILE, `SCArchive.IsUsed` forced false inside BUILD branch, no `f\.Filetype\b` style regressions.
- Settable-families regression: server `_settable_for` returns settable only for CUI*/RawText; SCA absent everywhere (grep-able test).
- Live verification via the LUA debug channel during the editor E2E (user-assisted), one command group at a time.

## Implementation

- `bridge/ZZZ_10_agent_bridge.lua` — dispatcher extension (all commands above)
- `server/eud_agent/bridge_io.py` — client wrappers per command + `_SETTABLE_FAMILIES` SCA drop
- `server/tests/test_bridge_datx_static.py` / `test_bridge_tree_static.py` / `test_bridge_plug_static.py` / `test_bridge_build_static.py` — static contracts
- external: EUD Editor 3 model APIs per [[capability-survey]] `../capability-survey.md`
