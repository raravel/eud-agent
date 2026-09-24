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
//! On non-MSVC targets (macOS, other Unix) step 1 is replaced by `build_with_cc`,
//! which compiles the same ReleaseUS|x64 translation units with the `cc` crate
//! into one `libisom_capi.a` (see its doc comment for the per-project mapping).
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
    let project = native_dir.join("isom_capi.vcxproj");
    let map_core_header = native_dir.join("IsomTerrain").join("MapAgentCore.h");
    let map_core_cpp = native_dir.join("IsomTerrain").join("MapAgentCore.cpp");
    let map_gen_cpp = native_dir.join("IsomTerrain").join("MapGenCli.cpp");
    let map_file_cpp = native_dir.join("MappingCoreLib").join("MapFile.cpp");
    let map_json_header = native_dir.join("IsomTerrain").join("MapAgentJson.h");
    let map_json_cpp = native_dir.join("IsomTerrain").join("MapAgentJson.cpp");

    // Rerun when the C ABI surface or the build target changes.
    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-changed={}", shim_cpp.display());
    println!("cargo:rerun-if-changed={}", solution.display());
    println!("cargo:rerun-if-changed={}", project.display());
    println!("cargo:rerun-if-changed={}", map_core_header.display());
    println!("cargo:rerun-if-changed={}", map_core_cpp.display());
    println!("cargo:rerun-if-changed={}", map_gen_cpp.display());
    println!("cargo:rerun-if-changed={}", map_file_cpp.display());
    println!("cargo:rerun-if-changed={}", map_json_header.display());
    println!("cargo:rerun-if-changed={}", map_json_cpp.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MSBUILD");

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build_static_lib(&native_dir, &solution);
        emit_link_directives(&native_dir);
    } else {
        build_with_cc(&native_dir);
    }
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

    let vswhere =
        PathBuf::from(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe");
    if !vswhere.is_file() {
        panic!(
            "vswhere.exe not found at {} — install Visual Studio 2022 (with the \
             C++ build tools) or set the MSBUILD env var to MSBuild.exe",
            vswhere.display()
        );
    }

    let out = Command::new(&vswhere)
        .args(["-latest", "-find", r"MSBuild\**\Bin\MSBuild.exe"])
        .output()
        .expect("failed to run vswhere.exe");
    if !out.status.success() {
        panic!("vswhere failed: {}", String::from_utf8_lossy(&out.stderr));
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
    let icu_include = native_dir.join("..").join("include").join("unicode");
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
    "advapi32", "user32", "ole32", "oleaut32", "shell32", "version", "ws2_32", "bcrypt", "wininet",
    "comdlg32", // GetOpenFileNameW / GetSaveFileNameW (MappingCoreLib SystemIO)
];

/// Non-MSVC equivalent of the MSBuild solution: compiles the ReleaseUS|x64
/// translation units of every subproject into ONE static archive
/// `libisom_capi.a`, mirroring the folded `isom_capi.lib`.
///
/// Deliberate deviations from the vcxprojs, all following upstream's own Unix
/// build (StormPort.h/CascPort.h and the CMakeLists.txt files):
/// * WIN32/WIN64/UNICODE-style defines are dropped, so SimpleIcu selects its
///   UTF-8 filestring/uistring path.
/// * zlib/bzip2 come from the system (`__SYS_ZLIB`, `__SYS_BZLIB`,
///   `CASC_USE_SYSTEM_ZLIB`) instead of the two bundled zlib copies, which
///   would otherwise collide in one archive.
/// * LZMA is single-threaded (`_7ZIP_ST`); LzFindMt.c/Threads.c are Win32-only.
/// * jenkins/lookup3.c is compiled once (the Casc/Storm copies differ only in
///   whitespace and MSVC pragmas).
fn build_with_cc(native_dir: &Path) {
    let out_dir = PathBuf::from(env_var("OUT_DIR"));
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // Without an explicit deployment target `cc` stamps objects with the SDK
    // version, which is newer than what rustc links for (11.0 by default).
    // Pin it to 11.0 unless the user chose one.
    if target_os == "macos" && std::env::var_os("MACOSX_DEPLOYMENT_TARGET").is_none() {
        std::env::set_var("MACOSX_DEPLOYMENT_TARGET", "11.0");
    }

    // `cc` does not track sources; rebuild when any vendored subproject changes.
    for project in [
        "CascLib",
        "StormLib",
        "IcuLib",
        "CrossCutLib",
        "MappingCoreLib",
        "IsomTerrain",
        "RareCpp",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            native_dir.join(project).display()
        );
    }

    let casc = native_dir.join("CascLib").join("src");
    let storm = native_dir.join("StormLib").join("src");
    let icu = native_dir.join("IcuLib");
    let cross_cut = native_dir.join("CrossCutLib");
    let map_core = native_dir.join("MappingCoreLib");
    let isom_terrain = native_dir.join("IsomTerrain");

    let mut objects = Vec::new();
    let mut compile = |name: &str,
                       cpp: bool,
                       files: Vec<PathBuf>,
                       includes: &[&Path],
                       defines: &[(&str, Option<&str>)]| {
        let mut build = base_build(cpp);
        build.files(files).includes(includes);
        for (key, value) in defines {
            build.define(key, *value);
        }
        objects.extend(
            build
                .out_dir(out_dir.join("obj").join(name))
                .compile_intermediates(),
        );
    };

    let casc_defines: &[(&str, Option<&str>)] = &[
        ("NDEBUG", None),
        ("_LIB", None),
        ("_7ZIP_ST", None),
        ("BZ_STRICT_ANSI", None),
        ("CASC_USE_SYSTEM_ZLIB", None),
    ];
    compile(
        "CascLib",
        true,
        sources(
            &casc,
            &[
                "CascDecrypt.cpp",
                "CascFiles.cpp",
                "CascDecompress.cpp",
                "CascDumpData.cpp",
                "CascFindFile.cpp",
                "CascIndexFiles.cpp",
                "CascOpenFile.cpp",
                "CascOpenStorage.cpp",
                "CascReadFile.cpp",
                "CascRootFile_Diablo3.cpp",
                "CascRootFile_Install.cpp",
                "CascRootFile_MNDX.cpp",
                "CascRootFile_OW.cpp",
                "CascRootFile_Text.cpp",
                "CascRootFile_TVFS.cpp",
                "CascRootFile_WoW.cpp",
                "common/Common.cpp",
                "common/Directory.cpp",
                "common/Csv.cpp",
                "common/FileStream.cpp",
                "common/FileTree.cpp",
                "common/ListFile.cpp",
                "common/RootHandler.cpp",
                "common/Mime.cpp",
                "common/Sockets.cpp",
                "md5/md5.cpp",
            ],
        ),
        &[],
        casc_defines,
    );
    compile(
        "CascLibC",
        false,
        sources(&casc, &["jenkins/lookup3.c"]),
        &[],
        casc_defines,
    );

    let storm_defines: &[(&str, Option<&str>)] = &[
        ("NDEBUG", None),
        ("_LIB", None),
        ("_7ZIP_ST", None),
        ("__SYS_ZLIB", None),
        ("__SYS_BZLIB", None),
    ];
    compile(
        "StormLib",
        true,
        sources(
            &storm,
            &[
                "FileStream.cpp",
                "SBaseCommon.cpp",
                "SBaseFileTable.cpp",
                "SBaseSubTypes.cpp",
                "SCompression.cpp",
                "SFileAddFile.cpp",
                "SFileAttributes.cpp",
                "SFileCompactArchive.cpp",
                "SFileCreateArchive.cpp",
                "SFileExtractFile.cpp",
                "SFileFindFile.cpp",
                "SFileGetFileInfo.cpp",
                "SFileListFile.cpp",
                "SFileOpenArchive.cpp",
                "SFileOpenFileEx.cpp",
                "SFilePatchArchives.cpp",
                "SFileReadFile.cpp",
                "SFileVerify.cpp",
                "adpcm/adpcm.cpp",
                "huffman/huff.cpp",
                "sparse/sparse.cpp",
            ],
        ),
        &[],
        storm_defines,
    );
    compile(
        "StormLibC",
        false,
        sources(
            &storm,
            &[
                "LibTomCrypt.c",
                "LibTomMath.c",
                "LibTomMathDesc.c",
                "lzma/C/LzFind.c",
                "lzma/C/LzmaDec.c",
                "lzma/C/LzmaEnc.c",
                "pklib/explode.c",
                "pklib/implode.c",
            ],
        ),
        &[],
        storm_defines,
    );

    compile(
        "IcuLib",
        true,
        sources(&icu, &ICU_SOURCES),
        &[&icu],
        &[
            ("NDEBUG", None),
            ("U_STATIC_IMPLEMENTATION", None),
            ("U_COMMON_IMPLEMENTATION", None),
            ("U_ATTRIBUTE_DEPRECATED", Some("")),
        ],
    );

    compile(
        "CrossCutLib",
        true,
        sources(
            &cross_cut,
            &[
                "Commander.cpp",
                "ErrorHandler.cpp",
                "GenericCommand.cpp",
                "Logger.cpp",
                "SimpleIcu.cpp",
                "TestCommands.cpp",
                "Updater.cpp",
            ],
        ),
        &[&icu],
        &[("NDEBUG", None), ("_CONSOLE", None)],
    );

    compile(
        "MappingCoreLib",
        true,
        sources(
            &map_core,
            &[
                "Basics.cpp",
                "CascArchive.cpp",
                "Chk.cpp",
                "MpqFile.cpp",
                "Sc.cpp",
                "EscapeStrings.cpp",
                "FileBrowser.cpp",
                "SystemIO.cpp",
                "MapFile.cpp",
                "ArchiveFile.cpp",
                "Scenario.cpp",
                "sha256.cpp",
                "TextTrigCompiler.cpp",
                "TextTrigGenerator.cpp",
                // Itanium-ABI-only companion to EscapeStrings.cpp (see file).
                "ConvertStrItanium.cpp",
            ],
        ),
        &[&cross_cut],
        &[
            ("NDEBUG", None),
            ("STORMLIB_NO_AUTO_LINK", None),
            ("CASCLIB_NO_AUTO_LINK_LIBRARY", None),
        ],
    );

    compile(
        "isom_capi",
        true,
        vec![
            native_dir.join("isom_capi.cpp"),
            isom_terrain.join("MapAgentCore.cpp"),
            isom_terrain.join("MapAgentJson.cpp"),
            isom_terrain.join("MapGenCli.cpp"),
            isom_terrain.join("IsomTests.cpp"),
        ],
        &[&isom_terrain, &icu],
        &[
            ("NDEBUG", None),
            ("STORMLIB_NO_AUTO_LINK", None),
            ("CASCLIB_NO_AUTO_LINK_LIBRARY", None),
        ],
    );

    // Fold everything into one archive, like the MSVC librarian step. `cc`
    // emits the search path + `static=isom_capi` for it.
    base_build(true)
        .cargo_metadata(true)
        .objects(objects)
        .compile("isom_capi");

    // `cc` above also emits the C++ runtime (libc++ on Apple, libstdc++
    // elsewhere). System zlib/bzip2 replace the bundled copies:
    println!("cargo:rustc-link-lib=dylib=z");
    println!("cargo:rustc-link-lib=dylib=bz2");
}

/// Shared ReleaseUS-equivalent flags: optimized regardless of the Cargo
/// profile (the MSVC path always builds ReleaseUS), C++17, and MSVC
/// `__declspec(align(1))` accepted as a no-op. `-Wno-everything` also silences
/// clang's default-error `missing-template-arg-list-after-template-kw` in
/// RareCpp's `template i(...)` calls, which MSVC accepts.
fn base_build(cpp: bool) -> cc::Build {
    let mut build = cc::Build::new();
    build
        .cpp(cpp)
        .opt_level(2)
        .debug(false)
        .warnings(false)
        .cargo_metadata(false)
        .flag_if_supported("-fdeclspec")
        .flag_if_supported("-Wno-everything");
    if cpp {
        build.std("c++17");
    }
    build
}

fn sources(dir: &Path, files: &[&str]) -> Vec<PathBuf> {
    files.iter().map(|f| dir.join(f)).collect()
}

/// IcuLib/common.vcxproj ClCompile items (all 186; none are excluded).
const ICU_SOURCES: [&str; 186] = [
    "filteredbrk.cpp",
    "ubidi.cpp",
    "ubiditransform.cpp",
    "ubidi_props.cpp",
    "ubidiln.cpp",
    "ubidiwrt.cpp",
    "uloc_keytype.cpp",
    "ushape.cpp",
    "brkeng.cpp",
    "brkiter.cpp",
    "dictbe.cpp",
    "pluralmap.cpp",
    "rbbi.cpp",
    "rbbidata.cpp",
    "rbbinode.cpp",
    "rbbirb.cpp",
    "rbbiscan.cpp",
    "rbbisetb.cpp",
    "rbbistbl.cpp",
    "rbbitblb.cpp",
    "rbbi_cache.cpp",
    "dictionarydata.cpp",
    "ubrk.cpp",
    "ucol_swp.cpp",
    "propsvec.cpp",
    "uarrsort.cpp",
    "uenum.cpp",
    "uhash.cpp",
    "uhash_us.cpp",
    "ulist.cpp",
    "ustack.cpp",
    "ustrenum.cpp",
    "utrie.cpp",
    "utrie2.cpp",
    "utrie2_builder.cpp",
    "uvector.cpp",
    "uvectr32.cpp",
    "uvectr64.cpp",
    "errorcode.cpp",
    "icudataver.cpp",
    "locmap.cpp",
    "putil.cpp",
    "umath.cpp",
    "umutex.cpp",
    "utrace.cpp",
    "utypes.cpp",
    "wintz.cpp",
    "ucnv.cpp",
    "ucnv2022.cpp",
    "ucnv_bld.cpp",
    "ucnv_cb.cpp",
    "ucnv_cnv.cpp",
    "ucnv_ct.cpp",
    "ucnv_err.cpp",
    "ucnv_ext.cpp",
    "ucnv_io.cpp",
    "ucnv_lmb.cpp",
    "ucnv_set.cpp",
    "ucnv_u16.cpp",
    "ucnv_u32.cpp",
    "ucnv_u7.cpp",
    "ucnv_u8.cpp",
    "ucnvbocu.cpp",
    "ucnvdisp.cpp",
    "ucnvhz.cpp",
    "ucnvisci.cpp",
    "ucnvlat1.cpp",
    "ucnvmbcs.cpp",
    "ucnvscsu.cpp",
    "ucnvsel.cpp",
    "cmemory.cpp",
    "ucln_cmn.cpp",
    "ucmndata.cpp",
    "udata.cpp",
    "udatamem.cpp",
    "udataswp.cpp",
    "uinit.cpp",
    "umapfile.cpp",
    "uobject.cpp",
    "dtintrv.cpp",
    "parsepos.cpp",
    "ustrfmt.cpp",
    "util.cpp",
    "util_props.cpp",
    "punycode.cpp",
    "uidna.cpp",
    "uts46.cpp",
    "locavailable.cpp",
    "locbased.cpp",
    "locdispnames.cpp",
    "locdspnm.cpp",
    "locid.cpp",
    "loclikely.cpp",
    "locresdata.cpp",
    "locutil.cpp",
    "resbund.cpp",
    "resbund_cnv.cpp",
    "ucat.cpp",
    "uloc.cpp",
    "uloc_tag.cpp",
    "ures_cnv.cpp",
    "uresbund.cpp",
    "uresdata.cpp",
    "resource.cpp",
    "ucurr.cpp",
    "caniter.cpp",
    "filterednormalizer2.cpp",
    "loadednormalizer2impl.cpp",
    "normalizer2.cpp",
    "normalizer2impl.cpp",
    "normlzr.cpp",
    "unorm.cpp",
    "unormcmp.cpp",
    "bmpset.cpp",
    "patternprops.cpp",
    "propname.cpp",
    "ruleiter.cpp",
    "ucase.cpp",
    "uchar.cpp",
    "unames.cpp",
    "unifiedcache.cpp",
    "unifilt.cpp",
    "unifunct.cpp",
    "uniset.cpp",
    "uniset_closure.cpp",
    "uniset_props.cpp",
    "unisetspan.cpp",
    "uprops.cpp",
    "usc_impl.cpp",
    "uscript.cpp",
    "uscript_props.cpp",
    "uset.cpp",
    "uset_props.cpp",
    "usetiter.cpp",
    "icuplug.cpp",
    "serv.cpp",
    "servlk.cpp",
    "servlkf.cpp",
    "servls.cpp",
    "servnotf.cpp",
    "servrbf.cpp",
    "servslkf.cpp",
    "usprep.cpp",
    "appendable.cpp",
    "bytesinkutil.cpp",
    "bytestream.cpp",
    "bytestrie.cpp",
    "bytestriebuilder.cpp",
    "bytestrieiterator.cpp",
    "chariter.cpp",
    "charstr.cpp",
    "cstring.cpp",
    "cstr.cpp",
    "cwchar.cpp",
    "edits.cpp",
    "messagepattern.cpp",
    "schriter.cpp",
    "stringpiece.cpp",
    "stringtriebuilder.cpp",
    "simpleformatter.cpp",
    "ucasemap.cpp",
    "ucasemap_titlecase_brkiter.cpp",
    "ucharstrie.cpp",
    "ucharstriebuilder.cpp",
    "ucharstrieiterator.cpp",
    "uchriter.cpp",
    "uinvchar.cpp",
    "uiter.cpp",
    "unistr.cpp",
    "unistr_case.cpp",
    "unistr_case_locale.cpp",
    "unistr_cnv.cpp",
    "unistr_props.cpp",
    "unistr_titlecase_brkiter.cpp",
    "ustr_cnv.cpp",
    "ustr_titlecase_brkiter.cpp",
    "ustr_wcs.cpp",
    "ustrcase.cpp",
    "ustrcase_locale.cpp",
    "ustring.cpp",
    "ustrtrns.cpp",
    "utext.cpp",
    "utf_impl.cpp",
    "listformatter.cpp",
    "ulistformatter.cpp",
    "sharedobject.cpp",
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
    let mut source = bindings.to_string();
    // A C enum's integer type is implementation-defined: MSVC always uses
    // `int`, while clang on Unix picks `unsigned int` for non-negative values.
    // The `isom_*` functions return `int`, so keep the status consts `c_int`
    // on every target (the MSVC output already is). `IsomStatus` is the only
    // allowlisted type, so its `Type` alias is the only one emitted.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        source = source.replacen(
            "pub type Type = ::std::os::raw::c_uint;",
            "pub type Type = ::std::os::raw::c_int;",
            1,
        );
    }
    std::fs::write(out_dir.join("bindings.rs"), source).expect("failed to write bindings.rs");
}
