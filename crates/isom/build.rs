//! Build script for the safe `isom` wrapper.
//!
//! Re-supplies the native link directives that do NOT reach this crate's
//! final-link targets (its `cargo test -p isom` binaries) automatically:
//!
//! 1. The four `cargo:rustc-link-arg` directives `isom-sys/build.rs` emits to
//!    put the link on the STATIC CRT (`isom_capi.lib` is built `/MT`).
//!    `rustc-link-arg` applies ONLY to the final-link target it is emitted for
//!    (isom-sys's own rlib never links an exe), never transitively — so every
//!    consumer must re-supply them or the link fails on a CRT mismatch
//!    (LNK2005 multiply-defined CRT symbols / LNK4098 default-lib conflict).
//!
//! 2. The `rustc-link-search` path to, and `rustc-link-lib=static=isom_capi`
//!    for, the engine archive. MEASURED: the `static=isom_capi` directive from
//!    isom-sys's build script does NOT reach the `isom` integration-test link
//!    (the `#[used]` anchor keeps `isom_abi_version` referenced, so its absence
//!    surfaces as `LNK2001: unresolved external symbol isom_abi_version`). The
//!    Win32 system `dylib`s DO propagate, so only the static archive + its
//!    search path need re-supplying here. Re-emitting is safe: this crate sets
//!    no `links` key, and a duplicate static-lib request is deduplicated by the
//!    linker.
//!
//! Keep these in lockstep with `crates/isom-sys/build.rs`.

use std::path::PathBuf;

/// Link args that put the Rust side on the static CRT to match `isom_capi.lib`
/// (`/MT`). Mirror of `isom-sys/build.rs::CRT_STATIC_LINK_ARGS`.
const CRT_STATIC_LINK_ARGS: &[&str] = &[
    "/NODEFAULTLIB:msvcrt.lib",
    "/NODEFAULTLIB:msvcprt.lib",
    "/DEFAULTLIB:libcmt.lib",
    "/DEFAULTLIB:libcpmt.lib",
];

/// MSBuild Platform / Configuration the engine archive lands under. Mirror of
/// `isom-sys/build.rs::{MSBUILD_PLATFORM, MSBUILD_CONFIG}`.
const MSBUILD_PLATFORM: &str = "x64";
const MSBUILD_CONFIG: &str = "ReleaseUS";

/// Strip the Windows `\\?\` verbatim prefix `canonicalize()` adds.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Only the MSVC toolchain builds & links the C engine; on any other target
    // there is nothing to re-supply (the FFI is Windows/MSVC-only).
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }

    // crates/isom -> ../../native/isom/<platform>/<config> : the same OutDir
    // isom-sys/build.rs links from (isom_capi.lib lives there after MSBuild).
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let lib_dir = strip_verbatim(
        manifest_dir
            .join("..")
            .join("..")
            .join("native")
            .join("isom")
            .join(MSBUILD_PLATFORM)
            .join(MSBUILD_CONFIG)
            .canonicalize()
            .expect(
                "isom_capi.lib output dir not found — build isom-sys first \
                 (it runs the MSBuild that produces native/isom/x64/ReleaseUS)",
            ),
    );
    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    // Pass the engine archive as a RAW link-arg (full path) rather than
    // `rustc-link-lib=static=isom_capi`. MEASURED: a `rustc-link-lib=static`
    // for a name already owned by the `links = "isom_capi"` crate (isom-sys) is
    // DEDUPLICATED away by rustc and never reaches the integration-test link
    // line — `isom_abi_version` then comes up `LNK2001`. A `rustc-link-arg` is
    // never deduplicated and always lands on the final-binary link command,
    // and (unlike `rustc-link-lib`) it applies to THIS final-link target.
    let lib_file = lib_dir.join("isom_capi.lib");
    println!("cargo:rustc-link-arg={}", lib_file.display());

    for arg in CRT_STATIC_LINK_ARGS {
        println!("cargo:rustc-link-arg={arg}");
    }
}
