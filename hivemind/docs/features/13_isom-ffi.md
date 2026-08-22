# Feature 13: isom C++ engine — vendored static lib + C ABI + Rust FFI

Brings the isom-poc map engine in-process as a statically linked library for CHK extraction,
legacy location/player/switch edits, installed-asset rendering, catalog queries, digesting,
strict existing-map candidate edits, and deterministic image-to-terrain quantization. Replaces
the v1 `IsomTerrain.exe` sidecar + `chk_info.py`; no native executable ships with the application.

> Decision: see [[decisions/09_cpp-static-lib-ffi]].

## Vendoring
Copy the needed isom-poc projects into `native/isom/` (our repo = source of truth):
IsomTerrain (lib) + CrossCutLib + IcuLib (vendored ICU) + CascLib. The editor's own C++ is
never touched. Keep upstream layout; add only the shim.

## C ABI shim (`native/isom/isom_capi.{h,cpp}`)
`extern "C"`, no STL/exceptions across the boundary. C++ exceptions caught at the shim ->
error code.
```c
int isom_chk_extract(const char* map_path, uint8_t** out, size_t* out_len);
int isom_locedit(const char* map_path, const uint8_t* ops, size_t ops_len);
int isom_playeredit(const char* map_path, const uint8_t* ops, size_t ops_len);
int isom_switchedit(const char* map_path, const uint8_t* ops, size_t ops_len);
int isom_render_map(const char* map_path, const char* starcraft_path,
                    uint32_t scale, uint8_t** out, size_t* out_len);
int isom_mapedit(const char* input_path, const char* output_path,
                 const char* starcraft_path,
                 const uint8_t* request, size_t request_len,
                 uint8_t** report, size_t* report_len);
int isom_render_region(const char* map_path, const char* starcraft_path,
                       const uint8_t* request, size_t request_len,
                       uint8_t** rgba, size_t* rgba_len,
                       uint32_t* width, uint32_t* height);
int isom_catalog_query(const char* starcraft_path,
                       const uint8_t* request, size_t request_len,
                       uint8_t** result, size_t* result_len);
int isom_map_digest(const char* map_path, uint8_t** result, size_t* result_len);
int isom_image_quantize(const char* starcraft_path, uint16_t tileset,
                        const uint8_t* rgba, size_t rgba_len,
                        uint16_t width, uint16_t height,
                        const uint16_t* before_tiles, size_t before_tile_count,
                        uint8_t** result, size_t* result_len);
void isom_free(uint8_t* p);
int isom_abi_version(void); // ABI v5
```
- Legacy save paths keep `autoDefragmentLocations=false`, `lockAnywhere=true` (rules.md).
- Location/switch NAME bytes pass through operation buffers as raw map-encoding bytes.
- ABI v5 retains strict v4 map-edit JSON validation and adds one packed bounded
  `eud-map-image-quantize/1` result: dimensions/counts, exact MTXM values, and one preview RGB
  triplet per output tile.
- `isom_mapedit` loads one existing SCX and saves one output SCX. It supports exact/semantic
  terrain, unit/building, doodad+overlay, sprite, and location operations; it never calls rawgen
  or creates a new map.
- `unit.set` patches only fields present in the request and preserves the remaining 36-byte UNIT
  state. Doodad set/move/delete requires exact `replacementTiles` for the old footprint so the
  semantic terrain+overlay operation cannot leave stale doodad graphics.
- The safe Rust wrapper serializes native calls because the vendored engine's temporary-map
  machinery is process-global; buffers are still freed before releasing the call guard.
- Render validation failures return an `eud-map-error/1` detail buffer; the Rust wrapper
  frees it and preserves its actionable message in `NativeCallError`.
- `isom_image_quantize` reuses the cached installed CV5/VX4/VR4/WPE snapshot. It scans only
  graphics-valid exact terrain tiles, computes deterministic SD representative RGB plus
  walkability/height classes, keeps the earliest tile for duplicate RGB, preserves alpha-zero
  candidate tiles, composites partial alpha, applies fixed Bayer 8x8 dithering, and resolves
  nearest-color ties by original scan order. Width/height and every buffer are capped at 256 and
  65,536 cells; all C++ failures remain contained as C ABI status/detail.

## Build & link
- `native/isom/` builds to a static `.lib` via MSBuild (a new lib target that compiles the
  shim + links IsomTerrain/ICU/CascLib).
- `crates/isom-sys/build.rs`: invoke msbuild for the lib target, emit
  `cargo:rustc-link-search` + `cargo:rustc-link-lib=static=...`; `bindgen` generates Rust
  bindings from `isom_capi.h`. Requires the MSVC toolchain.
- `crates/isom/`: safe wrapper returning `Result`, owning/freeing C buffers via `isom_free`.
- `MapAgentCore.{h,cpp}` shares installed CV5/VX4/VR4/WPE/DAT/GRP data for semantic/exact
  catalogs, deterministic thumbnails, player-colored composite crops, one-load/one-save edits,
  and the cached graphics-valid photo palette/quantizer.

## CHK parsing (Rust)
The raw CHK from `isom_chk_extract` is parsed into locations, full unit
placements, forces/players, MTXM terrain tiles, SWNM names, and TRIG switch
condition/action usages. `map_info` returns bounded filtered pages, never the
unbounded raw arrays.

## mapsafe (Rust service rails)
Legacy direct map tools and Map Agent Apply share the same outer safety owner:
1. STATUS compiling guard. 2. CreateFileW no-share lock probe. 3. Source/candidate hashes.
4. Full-file backup under `%appdata%\eud-agent\map_backups`. 5. Durable pending-Apply record.
6. Same-directory atomic replace. 7. Exact post-write verification; failure immediately restores
the backup. 8. Candidate state commit and pending-record removal; explicit undo restores exact
backup bytes through the same rails. Startup restores an interrupted, uncommitted replacement and
recognizes a candidate state that was committed immediately before a crash.
Location #64 remains protected, location ids stay stable, and trigger-used location deletion fails.

## Edge cases
- Invalid op in a batch -> C ABI aborts before save; mapsafe reports, no backup restore
  needed (nothing written).
- Map open in SCMDraft -> lock probe refuses with a clear message.
- msbuild/MSVC absent in dev -> build.rs fails fast with a setup hint.

## Implementation
- `native/isom/isom_capi.h`, `native/isom/isom_capi.cpp` — ABI v5 shim with native error detail
- `native/isom/IsomTerrain/MapAgentJson.{h,cpp}` — strict duplicate-key-rejecting JSON
- `native/isom/IsomTerrain/MapAgentCore.{h,cpp}` — existing-map writer, renderer, catalog,
  digest, photo palette, and deterministic quantizer
- `native/isom/IsomTerrain/MapGenCli.cpp` — retained legacy chk/locedit/playeredit/switchedit/render paths
- `crates/isom-sys/build.rs`, `crates/isom-sys/src/lib.rs` — bindgen + static link
- `crates/isom/src/lib.rs` — ABI v5 assertion and safe owned-buffer/image-envelope wrappers
- `src-tauri/src/chk.rs` — CHK parse plus terrain/unit/doodad/sprite/location digests
- `src-tauri/src/map_candidate.rs`, `map_verify.rs`, `mapsafe.rs` — candidate authority,
  replay, verification, Apply/rollback/undo rails
- external: vendored ICU + CascLib/StormLib (static), `bindgen`
- [BOUND 2026-06-09 from EUD-133-f076] `crates/isom/build.rs` — re-supplies the engine-archive link directives (search path + raw `isom_capi.lib` link-arg) that rustc dedups away for the isom crate's own test binaries; built on /MD (no static-CRT forcing) per Decision 14
- [BOUND 2026-06-09 from EUD-128-daea] `src-tauri/build.rs` — re-supplies the `isom_capi.lib` static-archive link directive (search path + raw `rustc-link-arg`) that rustc dedups away from the eud-agent final link (isom-sys declares links="isom_capi"); mirror of crates/isom/build.rs, alongside tauri_build::build()
