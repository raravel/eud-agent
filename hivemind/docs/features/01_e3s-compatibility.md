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

The referenced source map must exist. Absolute legacy paths may fall back to `mRelativeOpenMapName` beside the E3S.

GUIEps, GUIPy, RawText, and ClassicTrigger-as-MainFile are rejected explicitly. Built-in non-main ClassicTrigger and Setting nodes are retained as opaque records in the compatibility base; no unsupported structure is silently projected.

## Semantic export

`export_e3s(project, destination, compat_root)`:

1. requires an imported compatibility base;
2. copies the source map beside the destination E3S;
3. rewrites map paths, CUI source tree, MainFile, settings/plugins, and every supported DAT family in the NRBF graph;
4. retains unsupported opaque base objects;
5. embeds a versioned native manifest/DAT/source payload in an unreferenced string record for exact native re-import;
6. writes atomically.

The output is validated by reparsing before placement. Compatibility verification additionally deserializes a real exported fixture with the original .NET `BinaryFormatter` and EUD Editor assembly.

Native-only projects can create/open/save/build without E3S. Export intentionally refuses when no compatibility base exists rather than synthesizing a lossy legacy graph.

## Product surface

- Setup: existing Native project, new project from SCX/SCM, or semantic E3S import.
- Settings → Project: open, create, import, export.
- Long-running picker/import/export buttons expose disabled and busy states.
- Stable import/create failures are mapped to Korean recovery text; detailed errors remain in the command failure.

## Verification

- `nrbf::tests::parses_and_writes_a_minimal_stream_exactly`
- `e3s_nrbf::tests::native_extension_is_parseable_and_removable`
- ignored real-fixture contract: `real_fixture_import_export_import_is_semantically_stable`
- external acceptance: real E3S import → export → import equality plus .NET `BinaryFormatter.Deserialize` success
