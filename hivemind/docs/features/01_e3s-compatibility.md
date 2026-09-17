# E3S compatibility

## Purpose

`src-tauri/src/nrbf.rs` and `src-tauri/src/e3s_nrbf.rs` implement the legacy `.e3s` boundary without loading EUD Editor assemblies or starting an Editor process.

The native project remains authoritative. E3S is import/export compatibility only.

## Reader/writer

- Independent MS-NRBF parser/writer; no `BinaryFormatter` execution in the product.
- Supported record families: stream header/end, libraries, class metadata and `ClassWithId`, references/null runs, strings, primitive/object/string arrays, and rectangular `BinaryArray` records.
- Unknown executable behavior is never invoked.
- Parse limits: 64 MiB E3S, 32 MiB native extension, bounded strings/collections.
- A parse followed by write preserves a supported input byte-for-byte.
- New object ids are greater than every positive or negative id magnitude. This avoids .NET `ObjectManager` collisions with negative metadata ids.

## Semantic import

`import_e3s(source, destination, compat_root)` requires an empty destination and projects:

- `OpenMapName`/relative map reference into `maps/`;
- CUI epScript tree and exact MainFile into `src/`;
- standard DAT sparse overrides;
- wireframe, ButtonSet, and status XDAT overrides;
- TBL, requirements, and button-set overrides;
- supported main settings and EDS user plugins.

EDS blocks are read as one ordered stream, not as independent plugins. Headerless user blocks extend the active section (including generated `[chatEvent]`), and multiple headers in one block become separate native plugins. Saved chat-event addresses and MSQC mouse configuration are materialized into plugin text; disabled generated blocks do not change the active section. Duplicate sections and unprojectable continuations are rejected rather than misclassified as `[main]` settings.

The referenced source map must exist. Absolute legacy paths may fall back to `mRelativeOpenMapName` beside the E3S.

GUIEps, GUIPy, RawText, and ClassicTrigger-as-MainFile are rejected explicitly. Built-in non-main ClassicTrigger and Setting nodes are retained as opaque records in the compatibility base; no unsupported structure is silently projected.

## Local harness migration

The setup import transaction also restores this machine's legacy harness before activating the native project. It finds a unique trusted workspace by the original E3S's canonical full path (Windows case, separator, and verbatim-prefix normalization), never by basename alone.

- Copy accepted workspace text documents and approved `plans/`, preserving approval/revision metadata and validating approved-plan hashes. Historical root-level and non-Markdown document paths use the legacy workspace confinement rules, not the narrower policy for newly authored harness documents. Preserve deletion tombstones without recreating deleted files.
- Bind the copied workspace to the native project's `.eud-agent/workspace` and trusted `.eud-agent/state/workspace.json`. The persisted workspace ID survives root moves. Unaccepted drafts, session working roots, and jobs are not copied into the project; the provider CLI cwd is the project root, so there is no `source/` mirror to regenerate.
- Copy the four project-memory files, `meta.json`, and `wiki/ledger.json` from the legacy full-path memory key to `<project>/.eud-agent/memory`. Recognize quoted/unquoted historical keys and Windows path aliases. Report multiple matching stores instead of guessing; continue with independent stores. Keep the old source-list hash so structure is not falsely marked current.
- Copy EPS and Map conversation histories under fresh session IDs, preserving names, provider/model/reasoning settings, timestamps, and opaque panel logs. EPS rows match the quoted/unquoted full E3S path; Map rows match the historical SHA-256 of its lowercase canonical full path. Rebind to the native project name or native Map identity respectively. Reset provider connections, pending review IDs, usage/context/task state; do not copy map candidates or journals. Validate the index and matching records before publication, and roll back only unchanged imported records/index entries so unrelated concurrent session saves survive.
- Preserve original stores and existing destination files/metadata. Missing accepted documents or approved-plan bodies, corrupt data, size violations, unsafe/unavailable paths, ambiguous ownership, and per-item collisions are optional omissions. Recover valid siblings and independent workspace/memory/session stores. An occupied EPS history does not block an importable Map history. Preserve tombstones and never retain imported approval metadata without its validated body.
- After safe rollback/cleanup, setup returns `importIssues: [{id, scope, path, reason}]` with no `error` and `projectOpened: false`. Selection and recency remain unchanged during review. `excludedImportItems` must explicitly contain every current issue ID before activation; IDs bind scope/path/reason, not just a filename. Re-evaluate sources on every attempt: newly available files are imported, changed issues require renewed consent, and recheck does not authorize exclusions. Only core project/map/destination failures or cleanup/rollback failures remain fatal.
- Failed setup rolls back imported stores and clears generated destination contents without deleting the selected folder itself, so an open Windows folder handle cannot replace the original error with a sharing violation. Cleanup failures retain the original error and identify the affected output path. Selection and recency remain unchanged.

Durable harness data is now project-local, but still separate from the E3S stream/native extension. E3S alone cannot restore another computer's harness; renamed or moved legacy E3S files are not associated by guessed names. Missing legacy stores require no migration. A fresh E3S import writes its one-time AppData cutover receipt so future native opens cannot attach unrelated old same-name data. Conversations, runtime jobs, credentials, and caches remain machine-local.

## Semantic export

`export_e3s(project, destination, compat_root)`:

1. requires an imported compatibility base;
2. copies the source map beside the destination E3S;
3. rewrites map paths, CUI source tree, MainFile, settings/plugins, and every supported DAT family in the NRBF graph;
4. retains unsupported opaque base objects;
5. embeds a versioned native manifest/DAT/source payload in an unreferenced string record for exact native re-import;
6. writes atomically.

Export disables Editor's automatic chatEvent/MSQC emission because the imported settings now live in native plugin text. This prevents duplicate sections when Editor builds the exported project.

The output is validated by reparsing before placement. Compatibility verification additionally deserializes a real exported fixture with the original .NET `BinaryFormatter` and EUD Editor assembly.

Native-only projects can create/open/save/build without E3S. Export intentionally refuses when no compatibility base exists rather than synthesizing a lossy legacy graph.

## Product surface

- Setup: existing Native project, new project from SCX/SCM, or semantic E3S import.
- Settings → Project: open, create, import, export.
- Long-running picker/import/export buttons expose disabled and busy states.
- Stable import/create failures are mapped to Korean recovery text. The import dialog exposes the original backend or command error under keyboard-accessible **오류 상세**, preserves selections after failure, and clears stale details on retry. Unknown failures never assume that the source map is missing.
- The third import step, **가져올 부가 문서 확인**, shows every unavailable optional item's scope, path, Korean summary, and expandable original reason. It asks **이 항목들을 제외하고 가져올까요?** with fixed-footer **검토한 N개를 제외하고 가져오기**, **다시 확인**, and cancel actions. Pending review has no error alert and cannot activate the project. Recheck and changed source/destination clear consent; new issue identities require renewed consent. Review/error rendering scrolls only dialog content. Focus goes to the neutral review region, never directly to consent. Successful native opens with partial AppData migration show a persistent omission notice, including on the remaining setup screen.

## Verification

- `nrbf::tests::parses_and_writes_a_minimal_stream_exactly`
- `e3s_nrbf::tests::native_extension_is_parseable_and_removable`
- EDS regression coverage: generated chat-event continuations, split MSQC settings, disabled generated blocks, main-setting boundaries, and duplicate-section refusal
- ignored real-fixture contract: `real_fixture_import_export_import_is_semantically_stable`, including semantic equality of the exported Editor graph without the native extension
- ignored setup regression: `real_e3s_import_restores_legacy_harness_without_mutating_source`, covering mixed omissions, exact and renewed consent, held destination-folder handles, quoted memory/wiki, conversation copies, occupied legacy destinations, and unchanged originals/config/recency during review
- workspace regressions: full-path association, healthy documents beside missing ordinary/approved-plan bodies, local collisions, rollback ownership, tombstones, and portable trusted identity
- memory/session regressions: quoted path aliases, same-name project isolation, ambiguous stores, bad/healthy siblings, EPS/Map history separation, and rollback preserving unrelated/concurrent saves
- native cutover regressions: one-time migration, local-file precedence, folder relocation, no replay after local deletion, and nonblocking reporting of unbound name-only memory
- filesystem smoke: NTFS AppData source → exFAT project destination, atomic no-replace publication, readable imported documents/memory, relocation, and unchanged originals
- Windows rollback regressions: keep an open destination folder and retain both the import cause and locked output path when cleanup fails
- external acceptance: real E3S import → export → import equality plus .NET `BinaryFormatter.Deserialize` success
