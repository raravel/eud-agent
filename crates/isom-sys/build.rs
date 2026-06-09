//! Build script for `isom-sys`.
//!
//! 1. Builds the vendored C++ static library `isom_capi.lib` by invoking MSBuild
//!    on `native/isom/isom_capi.sln` (ReleaseUS|x64). The SOLUTION is the entry
//!    point (not the bare vcxproj) so `$(SolutionDir)` resolves and the dep .libs
//!    land in the one shared OutDir the librarian folds. MSBuild is located via
//!    vswhere (VS 2022) unless overridden by the `MSBUILD` env var.
//! 2. Emits the link directives so Rust links that single archive plus the Win32
//!    system libraries the folded CascLib/StormLib/ICU code pulls in.
//! 3. Runs bindgen over `native/isom/isom_capi.h` to generate the Rust FFI into
//!    `$OUT_DIR/bindings.rs`.
//!
//! CRT (load-bearing for downstream): `isom_capi.lib` (ReleaseUS) is built `/MD`
//! (dynamic CRT), matching Rust MSVC's default and the prebuilt `ort_sys` library
//! used by `fastembed`. No CRT-forcing link args are emitted here, and downstream
//! final-link targets such as `src-tauri` require no special CRT handling.

use std::path::{Path, PathBuf};
use std::process::Command;

const MSBUILD_CONFIG: &str = "ReleaseUS";
const MSBUILD_PLATFORM: &str = "x64";
const PLATFORM_TOOLSET: &str = "v143";

fn main() {
    let manifest_dir = PathBuf::from(env_var("CARGO_MANIFEST_DIR"));
    // crates/isom-sys -> ../../native/isom. Strip the Windows `\\?\` verbatim
    // prefix `canonicalize()` adds: MSBuild custom-build steps shell out to cmd's
    // `copy`, which does NOT understand verbatim paths (the ICU header-staging
    // step fails with "path not found" otherwise).
    let native_dir = strip_verbatim(
        manifest_dir
            .join("..")
            .join("..")
            .join("native")
            .join("isom")
            .canonicalize()
            .expect("native/isom directory not found relative to crates/isom-sys"),
    );

    // The solution (not the bare vcxproj) is the build entry point: each vendored
    // subproject sets OutDir to `$(SolutionDir)x64\<Config>\`, so building via the
    // .sln makes `$(SolutionDir)` resolve to native\isom\ and all dep .libs land
    // in the ONE shared OutDir the isom_capi librarian folds them from. Building
    // the bare .vcxproj leaves `$(SolutionDir)` undefined and the librarian can't
    // find CascLib.lib (LNK1181).
    let solution = native_dir.join("isom_capi.sln");
    let header = native_dir.join("isom_capi.h");
    let shim_cpp = native_dir.join("isom_capi.cpp");

    // Rerun when the C ABI surface or the build target changes.
    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-changed={}", shim_cpp.display());
    println!("cargo:rerun-if-changed={}", solution.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MSBUILD");

    build_static_lib(&native_dir, &solution);
    emit_link_directives(&native_dir);
    generate_bindings(&header);
}

/// Strip the Windows `\\?\` verbatim/extended-length prefix from a path.
/// `Path::canonicalize` returns verbatim paths on Windows; cmd builtins used by
/// MSBuild custom-build steps choke on them.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p
    }
}

fn env_var(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("env var {key} not set"))
}

/// Locate MSBuild.exe: honor a `MSBUILD` override, else query vswhere.
fn find_msbuild() -> PathBuf {
    if let Ok(p) = std::env::var("MSBUILD") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return p;
        }
        panic!("MSBUILD env var set but not a file: {}", p.display());
    }

    let vswhere = PathBuf::from(
        r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe",
    );
    if !vswhere.is_file() {
        panic!(
            "vswhere.exe not found at {} — install Visual Studio 2022 (with the \
             C++ build tools) or set the MSBUILD env var to MSBuild.exe",
            vswhere.display()
        );
    }

    let out = Command::new(&vswhere)
        .args([
            "-latest",
            "-find",
            r"MSBuild\**\Bin\MSBuild.exe",
        ])
        .output()
        .expect("failed to run vswhere.exe");
    if !out.status.success() {
        panic!(
            "vswhere failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let path = stdout
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_else(|| {
            panic!("vswhere could not locate MSBuild.exe — install the MSVC C++ toolchain")
        });
    PathBuf::from(path)
}

fn build_static_lib(native_dir: &Path, solution: &Path) {
    let msbuild = find_msbuild();
    eprintln!("isom-sys: using MSBuild at {}", msbuild.display());

    // IcuLib\common.vcxproj stages the ICU public headers into native\include\
    // unicode\ via a custom-build `copy` step; cmd's `copy` fails ("path not
    // found") if the destination dir is missing. Create it up front. This dir is
    // build output OUTSIDE native\isom\ and is intentionally not committed.
    let icu_include = native_dir
        .join("..")
        .join("include")
        .join("unicode");
    std::fs::create_dir_all(&icu_include)
        .unwrap_or_else(|e| panic!("could not create {}: {e}", icu_include.display()));

    // PlatformToolset=v143: only v143 (14.40) is installed but the vendored
    // subprojects hardcode v142.
    // PostBuildEventUseInBuild=false: CascLib/StormLib ship a PostBuild.bat that
    // exits 9009 headless (benign copy) — suppressing it keeps the build green.
    let status = Command::new(&msbuild)
        .arg(solution)
        .arg(format!("/p:Configuration={MSBUILD_CONFIG}"))
        .arg(format!("/p:Platform={MSBUILD_PLATFORM}"))
        .arg(format!("/p:PlatformToolset={PLATFORM_TOOLSET}"))
        .arg("/p:PostBuildEventUseInBuild=false")
        .arg("/m")
        .arg("/nologo")
        .arg("/v:minimal")
        .current_dir(native_dir)
        .status()
        .expect("failed to spawn MSBuild");
    if !status.success() {
        panic!(
            "MSBuild failed for {} ({MSBUILD_CONFIG}|{MSBUILD_PLATFORM}); exit {:?}",
            solution.display(),
            status.code()
        );
    }
}

fn emit_link_directives(native_dir: &Path) {
    let lib_dir = strip_verbatim(
        native_dir
            .join(MSBUILD_PLATFORM)
            .join(MSBUILD_CONFIG)
            .canonicalize()
            .expect("isom_capi.lib output dir not found — did MSBuild run?"),
    );

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=isom_capi");

    // Win32 system libraries the folded CascLib/StormLib/ICU/MappingCore code
    // pulls in once consumed as a bare archive (discovered from LNK2019s).
    for lib in SYSTEM_LIBS {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }
}

/// Win32 libs required by the folded engine archive. Add only what the link needs.
const SYSTEM_LIBS: &[&str] = &[
    "advapi32",
    "user32",
    "ole32",
    "oleaut32",
    "shell32",
    "version",
    "ws2_32",
    "bcrypt",
    "wininet",
    "comdlg32", // GetOpenFileNameW / GetSaveFileNameW (MappingCoreLib SystemIO)
];

fn generate_bindings(header: &Path) {
    let out_dir = PathBuf::from(env_var("OUT_DIR"));
    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        // Only the isom_* C ABI surface + the status enum + the abi-version macro.
        .allowlist_function("isom_.*")
        .allowlist_type("IsomStatus")
        .allowlist_var("ISOM_ABI_VERSION")
        // The enum is a plain C status code; map it to a Rust constified enum.
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        .generate()
        .expect("bindgen failed to generate FFI from isom_capi.h");
    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("failed to write bindings.rs");
}
