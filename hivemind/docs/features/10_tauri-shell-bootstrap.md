# Tauri shell and bootstrap

## Shell

The Tauri 2 application hosts the built React panel and registers typed native commands/events. Managed state is `ipc::AppManaged`, which owns resolved data directories only; each project operation reloads `config.json`.

`project_launch::PendingProjectLaunch` retains startup paths and second-instance requests until the panel consumes them. The single-instance plugin forwards arguments with the sender's working directory, then restores/focuses the main window. The panel subscribes to `project-open-requested` before draining `project_take_launch_request`; neither callback mutates project config.

## Data directories

- `%APPDATA%/eud-agent`: config, conversations, journals, runtime jobs, turn baselines, and preserved legacy harness sources.
- `%LOCALAPPDATA%/eud-agent`: models, RAG index, logs, analyzer mirrors, audio tools, native compatibility assets, managed euddraft distributions.
- selected project root: canonical manifest/sources/DAT/maps/build and `.eud-agent/{workspace,state,memory}` for durable documents, trusted approvals, memory/wiki, and the one-time migration receipt.

All app-written text/JSON is UTF-8 without BOM.

## config.json

Relevant configured paths:

```json
{
  "project_path": "C:\\Projects\\my-map",
  "euddraft_path": "C:\\Tools\\euddraft.exe",
  "starcraft_path": "C:\\Program Files (x86)\\StarCraft"
}
```

Model/RAG/provider settings retain their versioned/checksummed fields. The merged config schema is version 3. There is no Editor path.

`project_recents` stores up to 20 `{name, path, lastOpenedAt}` entries, keyed by normalized canonical Windows root and ordered newest-first. Only successful explicit open/create/import advances recency. `project_recents_initialized` records one-time migration of the previously configured project, so removing the final entry does not reseed it on restart. Missing/invalid entries remain listed with `available: false`; removal changes history only. A shared project-config transaction serializes selection, history initialization/normalization, and removal, so a concurrent initial list request cannot overwrite a successful direct open.

## Startup and environment setup

Every ordinary startup opens the launcher, regardless of saved setup completeness. It offers recent-project search/removal, file/folder open, create-from-map, and E3S import. A saved project is not accepted for this run until an explicit action succeeds; cancel/failure leaves the launcher visible without starting project polling, restoring sessions, or running project-dependent bootstrap.

The desktop launcher leads with a full-width E3S onboarding card above the project panes. It explicitly addresses EUD Editor 3 users, explains that imported projects can be edited/built without running Editor, and gives E3S import the primary filled action. This entry remains visible with or without recent history; create/file-open/folder-open stay secondary on the right. Searchable recent projects with last-opened times occupy the left pane. Each pane scrolls independently in short windows; the onboarding card, recent search field, and project actions do not move when the history list scrolls. Busy/error/pending-request status sits above both panes. Narrow layouts stack the panes.

Filesystem paths use `formatPathForDisplay` only at the presentation boundary: `\\?\E:\project` is shown as `E:\project`, and `\\?\UNC\server\share` as `\\server\share`. Recent rows, tooltips, pending requests, and selected import paths share this rule. Search accepts the displayed network path and either slash direction. Config, recent-entry identity, and native open/remove/import arguments retain the original canonical path; other device namespaces are not rewritten.

The Windows installer associates only `.eap`. New/imported projects write the canonical JSON manifest directly as `project.eap`; no descriptor is generated. A sole renamed `.eap` can also be opened and saved. Legacy `project.json`/`.eudproj` inputs migrate only through an explicit open after validation. Ordinary `.json` associations remain unchanged. Launching through `.eap` takes the same validated open path and bypasses manual selection on success.

The Windows ProgID is `EudAgent.Project`, with a Korean description and dedicated `$INSTDIR\icons\project.ico` DefaultIcon. NSIS hooks are the sole registry authority; do not add `bundle.fileAssociations`, whose generated macros overwrite backups on reinstall and restore them unconditionally on uninstall. The hooks quote both the executable and `%1`, preserve the prior `.eap` owner across reinstalls, retire only the app's old `.eudproj` ownership, and restore/clear owned associations after successful uninstall without overriding a changed owner. `SHChangeNotify` refreshes Explorer's association cache. The application icon is unchanged.

1. Native project
   - open a directory or `.eap` file; explicitly open an old `project.json`/`.eudproj` to migrate;
   - create from a selected SCX/SCM into an empty destination;
   - import a selected E3S and referenced map into an empty destination.
2. euddraft
   - an empty or omitted `euddraft_path` downloads the latest official `armoha/euddraft` release ZIP by default;
   - users may instead select an existing folder containing `euddraft.exe`/`euddraft.py`, or select either entrypoint directly;
   - verify the ZIP against GitHub's SHA-256 release digest, extract the complete distribution into staging, then publish under `euddraft/sha256-<digest>` in LocalAppData;
   - persist the installed executable path only after validation; a local selection made during automatic download wins;
   - existing configured paths are reused without a latest-release lookup on healthy startup; invalid explicit paths require reselection or the explicit latest-install action;
   - download errors retain retry and local-selection actions, and post-save progress advances to asset setup.
3. Assets
   - verify model, RAG index, native compatibility assets, and managed media tools;
   - download missing checksummed assets atomically.
4. AI provider
   - select Codex, Claude Code, Antigravity, OpenCode Go, or Ollama;
   - install/import credentials, authenticate, or configure local/base-URL access as supported.

`setup_status` returns project/euddraft/assets state, the closed five-provider status list, the selected default provider, and one stable optional error. Every setup snapshot includes `projectOpened`: only successful explicit project actions return `true`; status probes, cancel, and errors return `false`. An accepted project plus completed setup arms native project refresh, session restoration, and provider model loading.

## Project commands

- `setup_pick_project_path`
- `project_open`
- `project_recent_list`
- `project_recent_remove`
- `project_take_launch_request`
- `setup_create_project`
- `setup_import_e3s`
- `setup_pick_euddraft_path`
- `setup_install_euddraft`
- `project_export_e3s`
- `bootstrap_run`

Create/import destinations must be empty. Cancellation returns the unchanged setup snapshot. A failed action does not replace the configured valid project.

The E3S import dialog explains the migration boundary before selection: original E3S/map files are unchanged, imported work is saved in a separate empty folder, and subsequent edits do not automatically update the original E3S. It names the CUI epScript MainFile requirement and unsupported GUI/RawText formats rather than promising universal compatibility. The explicit source → destination → import flow is unchanged.

`setup_pick_project_path` selects `.eap` files by default, offers a labeled legacy `project.json`/`.eudproj` migration filter, or picks a folder with `directory: true`. `project_open` accepts a path without a dialog. `project_recent_list` and `project_recent_remove` return `{name, path, lastOpenedAt, available}` records; `project_take_launch_request` returns one `{path}` or `null`.

The workspace header exposes project switching. Native admission is serialized against session workers and rejects active agent/review/harness work, a build marker, or open map-agent/map-import windows. Idle worker caches and panel project/session views are invalidated only on a successful switch; persisted sessions are retained. A forwarded request blocked by active work or an open picker/import dialog stays pending with retry/dismiss controls, also visible after returning to the current workspace.

`setup_pick_euddraft_path` takes optional `directory: true` for the folder dialog; omission selects a file. `setup_install_euddraft` explicitly switches to the latest managed release and returns the setup snapshot. Automatic euddraft installation runs before the other assets under the shared bootstrap lock. Asset completion reloads config before updating only model/RAG fields, preserving project/provider/path selections made during downloads.

Settings includes a **Compile** category backed by dedicated typed commands. It
shows the configured path and the release tag from a matching managed install
marker without launching euddraft. **Check latest version** reads GitHub's
official latest release metadata; when the managed tag differs, **Update to
latest** runs the same checksum-verified atomic installer and switches the
configured path only after validation. Its progress uses the dedicated
`euddraft_update` stage and remains inside Compile settings; it never activates
the first-run bootstrap screen or writes progress into a chat session. Manually
selected distributions keep their path and report that their version cannot be
inferred; switching one to the latest managed release remains an explicit user
action.

## Resource sync

Tauri resource `native/eud-editor-compat` is copied to LocalAppData on startup. It contains data-format metadata only. EUD Editor binaries, DLLs, Lua, and install scripts are not resources.

## Verification

- setup status/config tests;
- create-from-map copy/open and nonempty-destination refusal;
- protocol/type-guard tests for every setup command;
- SetupScreen busy/action/accessibility tests;
- ordinary startup gating, canceled picker, direct startup, forwarded/busy request recovery, and stale same-name project refresh regression coverage;
- Settings Compile path/version rendering, explicit latest-release check, managed update, and manual-path messaging;
- canonical/extended Windows path MRU deduplication and remove-last migration coverage;
- canonical manifest validation/rename persistence, legacy migration rollback, unchanged-config failure, and real-map creation/open smoke;
- Windows NSIS association generation and isolated installed Explorer acceptance;
- production Tauri/browser interaction acceptance.
