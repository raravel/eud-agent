# Decision 14: Rebuild native/isom with the dynamic CRT (/MD) to coexist with ort_sys

- Date: 2026-06-09
- Status: Accepted
- Context: EUD-128 first made `isom` a dependency of `src-tauri` (eud-agent). The final
  link then needs `isom_capi.lib` AND `ort_sys` (the prebuilt ONNX runtime pulled in by
  `fastembed`, the RAG engine) in one MSVC binary. They demand opposite C runtimes:
  `isom_capi.lib` (+ folded IsomTerrain/CascLib/StormLib/ICU/MappingCore) is built `/MT`
  (static CRT) + `/GL` (WPO) per `native/isom/**` vcxproj, while `ort_sys 2.0.0-rc.12`
  ships `/MD` (dynamic CRT). A single MSVC binary allows only one CRT — measured both
  directions: default `/MD` link → unresolved `isom_*` (LNK2019); `-C target-feature=
  +crt-static` → unresolved `ort_sys __imp_*` imports (LNK2019, 67 symbols). This blocked
  EUD-128's build/test criteria; it was blocked pending this decision.
- Considered:
  - Rebuild isom as /MD — Pros: single in-process binary (keeps Decision 09), matches the
    Rust default /MD. Cons: rebuilds the vendored C++ under a different CRT; link must be
    re-verified. Recommendation: ★★★.
  - Force ort to static CRT (/MT) — Pros: keeps isom /MT/GL. Cons: ort-sys 2.0.0-rc.12 is a
    prebuilt /MD binary from pyke; switching it to /MT is effectively infeasible without a
    from-source ORT build. Recommendation: ★☆☆ (infeasible).
  - Isolate isom in a separate process — Pros: full CRT isolation, keep /MT/GL. Cons:
    reverses Decision 09 (in-process static link was chosen specifically to remove the
    sidecar); re-introduces a process + IPC. Recommendation: ★☆☆ (contradicts architecture).
- Chosen: Rebuild native/isom with the dynamic CRT (/MD)
- Rationale: The only approach that keeps the single in-process static-link topology of
  Decision 09 while letting isom and ort share one CRT. ort_sys cannot be moved off /MD
  (prebuilt), and a sidecar would undo the very decision that vendored isom in-process.
- Impact:
  - `native/isom/**` vcxproj: `RuntimeLibrary` → `MultiThreadedDLL` (Release) /
    `MultiThreadedDebugDLL` (Debug); remove `WholeProgramOptimization` (/GL).
  - `crates/isom-sys/build.rs`: drop the static-CRT forcing (`/NODEFAULTLIB:msvcrt.lib`,
    `/DEFAULTLIB:libcmt.lib`); keep the `isom_capi` + system-lib link directives. Update the
    CRT note in the module doc comment.
  - feature 13 (isom-ffi) Implementation map gains the CRT requirement; rules.md Rust/C++
    FFI section notes the single-CRT (/MD) constraint.

## Addendum (EUD-133, 2026-06-09): /GL is RETAINED, not removed

The original Impact bullet "remove WholeProgramOptimization (/GL)" was a measured error and
is superseded: **/GL (LTCG) is load-bearing for the vendored MappingCoreLib and must be kept.**
`Chk::Action::stringUsed` / `briefingStringUsed` are declared `inline` in `Chk.h` and DEFINED
`inline` in `Chk.cpp`, yet ODR-used cross-TU from `Scenario.cpp` (`appendTriggerStrUsage`). No
out-of-line definition is emitted, so a non-LTCG build leaves them unresolved (`LNK2019`); only
/GL's whole-program cross-TU inlining resolves them. Removing /GL would require editing vendored
C++ (forbidden by rules.md "keep verified code paths intact"). /GL is orthogonal to the CRT, so
the CRT-coexistence goal is met by the RuntimeLibrary `/MD` switch alone. Measured (EUD-133):
`cargo test -p isom` (incl. `ffi_smoke`) and the isom+ort `eud-agent` coexistence link both pass
with `/MD` + `/GL` on the MSVC `link.exe` toolchain (the only toolchain rules.md supports). A
codex review [P2] re-raised /GL removal on toolchain-fragility grounds (lld-link / mismatched
toolset); overridden as not applicable to the mandated MSVC toolchain and infeasible without
vendor edits. Net change in EUD-133: RuntimeLibrary `/MT`->`/MD` (+ drop static-CRT forcing in
`isom-sys`/`isom` build.rs); WholeProgramOptimization left `true`.
