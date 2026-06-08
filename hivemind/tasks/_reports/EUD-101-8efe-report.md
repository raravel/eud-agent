---
task_id: EUD-101-8efe
completed_at: 2026-06-08T11:51:11
duration_minutes: 28
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 10
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 216293
  output: 38170
cost_usd: 6.11
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
Vendored the isom-poc map engine under `native/isom/` (our repo = source of truth) and added
a C ABI shim built to a single static `.lib` for the future Rust `isom-sys` crate. The shim
exposes chk-extract / locedit / playeredit / abi-version / free over plain buffers + status
codes, driving the verified `mapGenMain` engine entry (import-then-extend) rather than
re-implementing the save logic.

## Changes
- Vendored closure (910 files, 14.6 MB; down from 287 MB upstream by excluding the 142 MB
  CascLib `listfile/`, test dirs, and build outputs): IsomTerrain + CrossCutLib + IcuLib +
  CascLib + MappingCoreLib + StormLib + header-only RareCpp. Closure derived from
  `IsomTerrain.sln` `ProjectDependencies` + `IsomTerrain.vcxproj` `<AdditionalDependencies>`
  + `#include` chains.
- `native/isom/isom_capi.h` — C ABI: `extern "C"`, stddef/stdint only, include guard,
  `ISOM_ABI_VERSION` macro, `IsomStatus` enum, the 5 signatures (exactly as feature 13).
- `native/isom/isom_capi.cpp` — shim impl: two-layer exception safety (C++ `try/catch`
  outside, SEH `__try/__except` inside, C2712-safe); temp-file buffer marshalling; raw-byte
  `ops` pass-through; malloc/`isom_free` CRT-paired in-lib.
- `native/isom/isom_capi.vcxproj` + `isom_capi.sln` — `StaticLibrary` target compiling the
  shim + `MapGenCli.cpp` + `IsomTests.cpp` (globals), excluding `Main.cpp` (`int main`);
  folds the 5 dep `.lib`s via `<Lib>`; `/utf-8`, stdcpp17, auto-link suppression.
- `native/isom/.gitignore` — ignores MSBuild intermediates/outputs (built `.lib` not committed).

## Verification
Run by the orchestrator directly (the Rust cargo stages in verify.md are downstream — the
`isom-sys`/`isom` crates do not exist yet; the relevant gate here is the MSBuild static-lib):
- **Independent rebuild**: forced shim recompile → `msbuild isom_capi.vcxproj
  /p:Configuration=ReleaseUS /p:Platform=x64 /p:PlatformToolset=v143
  /p:PostBuildEventUseInBuild=false` → exit 0; `isom_capi.lib` regenerated (80.7 MB).
- **`dumpbin /LINKERMEMBER`**: the 5 `isom_*` symbols exported UNDECORATED (clean C ABI);
  `?mapGenMain@@YAHHQEAPEAD@Z` present in the archive; 83 engine/CASC symbols present.
- **Mangling check**: the shim's `int mapGenMain(int,char*[])` declaration is textually
  identical to the upstream def (`MapGenCli.cpp:1734`) → decoration matches → resolves at the
  downstream link.
- Scope: all 910 changed files under `native/isom/**`; no build artifacts committed.

## Review
Reviewer (opus) returned no blocking findings; rubric 9/10/9/9. Confirmed safe: no
exception/SEH escape from any `extern "C"` function; allocator pairing correct; raw-byte
fidelity preserved; map-safety rails correctly absent (delegated to the Rust `mapsafe` layer
per Decision 09); save flags + #64 protection delegated to the verified engine. Reviewer
"confirm" items all resolved by the orchestrator: the real header has the stddef/stdint
includes + guard + `extern "C"` + `ISOM_ABI_VERSION` macro; `IsomTests.cpp` defines no stray
`main`; `native/isom/.gitignore` ignores all MSBuild artifacts.

## Harness Sync
- Contract-drift guard: PASS — the 5 shim signatures match feature 13's spec exactly (no
  drift); comments affirm (not contradict) rules.md (autoDefragmentLocations=false,
  lockAnywhere=true, raw NAME bytes); diff is purely additive.
- Binding: feature 13 `## Implementation` already names `native/isom/isom_capi.h`,
  `native/isom/isom_capi.cpp`, and `native/isom/* — vendored ... + lib build target`; the new
  files are already documented (no append needed — idempotent).
- Dep binding: none — the vendored C++ is not a package-manager dependency; tech-stack.md
  already lists it under `## Legacy / Vendored`.

## Notes — downstream guidance for the isom-sys crate task (EUD-102)
The `isom-sys/build.rs` that invokes MSBuild + links this lib must account for:
1. **Toolset**: pass `/p:PlatformToolset=v143` — only v143 (14.40) is installed; the vendored
   vcxproj files hardcode v142 (kept verbatim, import-then-extend). The shim vcxproj defaults
   to v143 but inherited subprojects need the command-line override.
2. **PostBuild**: pass `/p:PostBuildEventUseInBuild=false` — CascLib/StormLib `PostBuild.bat`
   errors 9009 headless (benign copy step).
3. **Duplicate zlib (LNK4006)**: StormLib + CascLib each embed their own zlib; the librarian
   keeps the first definition. The downstream final link may re-emit LNK4006 and could hit
   LNK2005 if Rust pulls another zlib transitively — may need dedup or `/FORCE:MULTIPLE`.
4. **`/GL` + `/MT`**: ReleaseUS uses WholeProgramOptimization (`/GL`) and `RuntimeLibrary=
   MultiThreaded` (static CRT). The Rust link must tolerate LTCG; and since the lib is `/MT`
   while Rust MSVC defaults to `/MD`, Rust must NEVER `libc::free` an isom buffer — only
   `isom_free` (both malloc and free are compiled into this lib, so the alloc/free pair stays
   inside one CRT; this is why the free-via-isom_free contract is safety-critical).
5. **Consume config**: ReleaseUS|x64. The built `.lib` is regenerable (gitignored) — build.rs
   rebuilds it; `bindgen` consumes `native/isom/isom_capi.h`.
