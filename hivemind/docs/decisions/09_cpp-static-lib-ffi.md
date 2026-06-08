# Decision 09: C++ map engine as vendored static library via C ABI + Rust FFI

- Date: 2026-06-08
- Status: Accepted
- Context: The map tooling (chk extract / locedit / playeredit, used by map_info,
  location_write, player_setup) lives in isom-poc as an **MSBuild solution**
  (IsomTerrain library + CrossCutLib + IcuLib (vendored ICU) + CascLib), with the
  CLI entry `mapGenMain` in `MapGenCli.cpp` and a `IsomApi.h` surface. The user
  wants the C++ code managed inside this repository and consumed as a library
  (not an exe), statically linked into the single distributable.
- Considered:
  - MSBuild static `.lib` + C ABI shim, linked by Rust (Recommended) — Pros:
    reuses the already-working MSBuild build of ICU/CascLib; MSVC ABI is shared
    with the Rust MSVC toolchain so linking is direct; yields a single exe. Cons:
    `build.rs` must invoke msbuild; VS Build Tools required to build. ★★★.
  - Port the solution to CMake + `cc`/`cmake` crate — Pros: cargo-native build.
    Cons: large effort to re-express the ICU + CascLib builds under CMake. ★★☆.
  - Keep as an exe sidecar — Pros: zero integration cost. Cons: user explicitly
    wants a library, not an exe. ★☆☆.
- Chosen: Vendor the isom-poc sources under `native/isom/` (our repo is the source
  of truth). Add a C ABI shim (`isom_capi.h/.cpp`, `extern "C"`, no STL/exceptions
  across the boundary) that exposes chk/locedit/playeredit over plain buffers +
  status codes. Build it to a static `.lib` via MSBuild; an `isom-sys` crate links
  it (bindgen generates the FFI); a safe `isom` wrapper crate sits on top. The
  map-write safety rails (backup, no-share lock probe, compiling guard,
  journal/rollback) live in the Rust service layer (`mapsafe`), not in C++ — the
  same split the Python service had.
- Rationale: Static link satisfies both "single exe" and "library not exe".
  Reusing MSBuild avoids rebuilding ICU/CascLib under a new build system.
- Impact: architecture.md, tech-stack.md, rules.md, feature 13.
