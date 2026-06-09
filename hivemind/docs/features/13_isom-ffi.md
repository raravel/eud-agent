# Feature 13: isom C++ engine — vendored static lib + C ABI + Rust FFI

Brings the isom-poc map engine (chk extract / locedit / playeredit) in-process as a
statically linked library, with the map-write safety rails in Rust. Replaces the v1
`IsomTerrain.exe` sidecar + `chk_info.py`.

> Decision: see [[decisions/09_cpp-static-lib-ffi]].

## Vendoring
Copy the needed isom-poc projects into `native/isom/` (our repo = source of truth):
IsomTerrain (lib) + CrossCutLib + IcuLib (vendored ICU) + CascLib. The editor's own C++ is
never touched. Keep upstream layout; add only the shim.

## C ABI shim (`native/isom/isom_capi.{h,cpp}`)
`extern "C"`, no STL/exceptions across the boundary. C++ exceptions caught at the shim ->
error code.
```c
// returns 0 on success, nonzero error code otherwise
int  isom_chk_extract(const char* map_path, uint8_t** out, size_t* out_len);
int  isom_locedit   (const char* map_path, const uint8_t* ops, size_t ops_len);
int  isom_playeredit(const char* map_path, const uint8_t* ops, size_t ops_len);
void isom_free(uint8_t* p);          // frees buffers isom_* allocated
int  isom_abi_version(void);          // sanity check from Rust
```
- Save path keeps `autoDefragmentLocations=false`, `lockAnywhere=true` (rules.md).
- Location NAME bytes pass through `ops` as RAW bytes (no re-encode in C++/Rust).
- ops buffers reuse the existing locedit/playeredit op encoding from MapGenCli.

## Build & link
- `native/isom/` builds to a static `.lib` via MSBuild (a new lib target that compiles the
  shim + links IsomTerrain/ICU/CascLib).
- `crates/isom-sys/build.rs`: invoke msbuild for the lib target, emit
  `cargo:rustc-link-search` + `cargo:rustc-link-lib=static=...`; `bindgen` generates Rust
  bindings from `isom_capi.h`. Requires the MSVC toolchain.
- `crates/isom/`: safe wrapper returning `Result`, owning/freeing C buffers via `isom_free`.

## chk parsing (Rust port of chk_info.py)
The raw CHK from `isom_chk_extract` is parsed in Rust into the structured digest
(locations/units/forces/players) — Rust does the binary section parsing; the C ABI only
extracts. Used by `map_info`.

## mapsafe (Rust service rails)
Wraps every mutating call (location_write/player_setup):
1. STATUS compiling guard. 2. No-share lock probe (CreateFileW). 3. Full-file backup to
`%appdata%\eud-agent\map_backups`. 4. All-or-nothing C ABI apply. 5. Re-digest verify.
6. Journal entry; reject -> restore backup (temp + atomic replace). #64 protected.

## Edge cases
- Invalid op in a batch -> C ABI aborts before save; mapsafe reports, no backup restore
  needed (nothing written).
- Map open in SCMDraft -> lock probe refuses with a clear message.
- msbuild/MSVC absent in dev -> build.rs fails fast with a setup hint.

## Implementation
- `native/isom/isom_capi.h`, `native/isom/isom_capi.cpp` — C ABI shim
- `native/isom/*` — vendored IsomTerrain/CrossCutLib/IcuLib/CascLib + lib build target
- `crates/isom-sys/build.rs`, `crates/isom-sys/src/lib.rs` — bindgen + link
- `crates/isom/src/lib.rs` — safe wrapper
- `src-tauri/src/chk.rs` — CHK parse (port of chk_info.py)
- `src-tauri/src/mapsafe.rs` — rails + journal (ports journal.py)
- external: vendored ICU + CascLib (static), `bindgen`
- [BOUND 2026-06-09 from EUD-133-f076] `crates/isom/build.rs` — re-supplies the engine-archive link directives (search path + raw `isom_capi.lib` link-arg) that rustc dedups away for the isom crate's own test binaries; built on /MD (no static-CRT forcing) per Decision 14
