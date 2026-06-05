# EUD Editor 3 Capability Survey (2026-06-05)

Goal: the agent must be able to do everything the editor UI can do — data editing,
trigger files, settings, main, plugins, build. This survey maps every user-facing
editor capability to its VB object model and judges luanet reachability from the
bridge (multi-agent source read of `EUD-Editor-3`, 6 readers + adversarial critic;
5 load-bearing claims spot-checked against source, 1 refuted and corrected below).

## Headline conclusion

The editor is a thin WPF view over a small set of Public, serializable model
objects rooted at `GlobalObj.pgData/pjData/scData`. Nearly every capability is
**model-reachable from the bridge without touching UI code-behind**. The agent
vision is feasible. The genuinely blocked/risky areas are: modal dialogs
(project New/Open, SCA login), structured build-error retrieval, and direct
authoring of GUI/Classic structured trigger models (workaround exists).

## Capability matrix

| # | Capability | Model path | Feasibility | Bridge today | Gap |
|---|---|---|---|---|---|
| 1 | Standard dat fields (units/weapons/flingy/sprites/images/upgrades/techdata/orders) | `pj.BindingManager:get_DatBinding(enum,param,objId).Value` | High | GETDAT/SETDAT | covered |
| 2 | portdata/sfxdata | same store; `GetDatFileE` whitelist excludes them (SCDatFiles.vb:16-25, **confirmed**) | High | — | bridge name→enum table must bypass `GetDatFileE` |
| 3 | Unit/string names (stat_txt/tbl) | `get_StatTxtBinding(index).Value`; reset via `StatNullString` | High | — | new command; non-ASCII must travel via .cmd body (UTF-8), never Lua literals |
| 4 | statusinfor (Status/Display/Joint) | `get_ExtraDatBinding(statusinfor, name, objId)` | High | — | new command |
| 5 | wireframe/grpwire/tranwire remaps | `get_ExtraDatBinding(wireframe, 'wire'\|'grp'\|'tran', objId)` (null for out-of-range) | High | — | new command |
| 6 | Button set assign + button tables | assign via ExtraDatBinding; tables via `ExtraDat.ButtonData:GetButtonSet(id):PasteFromString(csv)` etc. (manual `SetDirty`) | High | — | new command |
| 7 | Requirements (require.dat) | `get_RequireDataBinding(objId, datEnum)` use-modes; custom via `PasteCopyData` string round-trip (capacity-limited; techdata vs Stechdata duality) | Medium | — | new command; prefer PasteCopyData over raw RequireBlock lists |
| 8 | Field reset / changed-state | `binding:DataReset()`, `IsDefault`, `scData.DefaultDat` baseline | High | — | enables safe diff/undo |
| 9 | Read any trigger file as eps (incl. GUI/Classic projection) | `f.Scripter:GetStringText()` | High | GET | covered |
| 10 | Write CUIEps/CUIPy/RawText | `Scripter.StringText` setter | High | SET | covered. **Side effect (confirmed)**: connected nodes write the external disk file immediately. SCAScript also has the setter but is DEFUNCT (see below) — drop from settable set |
| 11 | Create file of ANY type + folders, in any folder | `TEFile(name, EFileType.X)` + `parent:FileAdd/FolderAdd` | High | NEWEPS (CUIEps@root only) | generalize: type arg + target folder + folder creation |
| 12 | Rename / delete / move tree nodes | `FileName` setter; `parent:FileRemove/FolderRemove`; remove+add = move | High/Med | — | new commands; guards: top/Setting nodes, dangling `MainFile`, manual `SetDirty` on remove |
| 13 | **Set main (build entry)** | `pj.TEData.MainFile = <TEFile>` (writable property, SetDirty fires) | High | — | new SETMAIN command — any CUIEps can be main |
| 14 | Author ClassicTrigger structured model | `TriggerListCollection` + Trigger/TriggerCodeBlock/ArgValue ctors | Low | — | possible but KopiLua-hostile; **recommended path: author eps in CUIEps instead** (build consumes `GetFileText` either way) |
| 15 | Author GUI block model (GUIEps/GUIPy) | `GUIScriptEditor.items` ScriptBlock tree | Low | — | do not author directly; `TEFile:ChagneType()` converts valid CUIEps→GUIEps (parse failure pops modal — guard) |
| 16 | SET on GUI/Classic | **no `StringText` member — assignment THROWS** (critic refuted "silent no-op") | Blocked | pcall-guarded | structural guard: pre-check FileType before set; never rely on pcall |
| 17 | Program settings | `pgData:get_Setting/set_Setting(TSetting enum)` + `SaveSetting()` flush | High | — | new command; side effects (Graphic needs `LoadGRPData()`) |
| 18 | Project settings (OpenMapName/SaveMapName/AutoBuild/UseCustomtbl/ViewLog/TempFileLoc/TextEncoding) | plain `pjData` properties | High | — | new command |
| 19 | Plugins (= euddraft eds blocks; **no DLL concept, confirmed**) | `pjData.EdsBlock.Blocks` List CRUD; `EdsBlockItem(UserPlugin)` + `.Texts`; discovery = `<euddraft>\plugins` filenames | High | — | new commands; built-ins auto-reinsert at build (cannot delete) |
| 20 | Build / EDD build | `pj.EudplibData:Build(false/true)`; completion = poll `pgData.IsCompilng` | High | BUILD | add completion polling + result |
| 21 | Structured build errors | stdout/stderr captured in **Private** fields; parsed errors live only on TriggerEditor UI list; `macro.macroErrorList` reachable | Low | — | best path: Python runs euddraft itself (path from settings) and parses with the editor's documented regexes |
| 22 | Project Save to disk | `pjData:Save(false)` silent IF path exists; else modal dialog | Policy | — | contract change (rules.md says memory-only/user saves) — expose only behind explicit opt-in, never Save-As |
| 23 | Project New/Open | `ProjectData.Load` modal + ByRef global; `LoadWithFile(path)` unverified | Blocked (pending) | — | leave user-driven; follow-up: verify LoadWithFile non-modal path |
| 24 | SCA settings/publish | **DEFUNCT** — the scarchive.kr service no longer exists (user, 2026-06-05) | Excluded | — | never expose; ensure `SCArchive.IsUsed=False` so agent builds can't block on the dead login modal |

## Explicitly out of scope (editor chrome)

Theme/Donate/Update tabs, font/topmost/mute prefs, embedded command console
(redundant with LUA channel), dat clipboard plumbing, modal chooser/viewer
windows (drive their underlying data instead), docking/TreeView visual state,
**everything SCA** (SCArchive/scarchive.kr publish, login, SCA settings, **and
the SCAScript file type itself** — user confirmed 2026-06-05 that ALL of SCA is
a dead feature; the code remains in the editor but must never be targeted).
Pending cleanup: drop SCA from `_SETTABLE_FAMILIES` (server bridge_io) and from
the CUI/SCA/RawText wording in rules.md / architecture.md / feature docs.

## Not yet surveyed (follow-up pass needed)

BGM list model, DotPainter data, project Macro function definitions
(MacroFuncSetting), `LoadWithFile` modality — all serializable project state
flagged by the critic as unassessed. (`SCAImageDatas` was also flagged but is
excluded: SCA is defunct.)

## Cross-cutting safety facts (from source, feed into rules.md when implemented)

- Three Lua engines exist; ours is MacroManager's (loads `Data\Lua\TriggerEditor\*.lua` alphabetically; shared with builds — `IsCompilng` guard stands). LuaManager (DataEditor console) is a separate state; its targets are Public and reachable from ours directly.
- The editor injects NO globals into our state; everything flows from `import_type("EUD_Editor_3.GlobalObj")`.
- `WindowMenu.WindowMenus` module is the canonical UI action hub; its methods no-op (not crash) when guards fail.
- Dat value setters clamp to `[0, 256^Size-1]`; ExtraDat setters are Byte-typed Try/Catch (silently swallow bad values — read back to confirm).
- Enum args ALWAYS as imported enum objects. Parameterized properties ALWAYS `get_X/set_X`.
- Build pops modal dialogs for missing map paths, and for SCA login when `SCArchive.IsUsed=True` — SCA is defunct, so that login can never succeed: pre-validate map paths AND force `IsUsed=False` via model before BUILD.
