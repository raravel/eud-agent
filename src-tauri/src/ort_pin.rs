// The official ONNX Runtime the app ships as a bundle resource. Included by
// `build.rs` (which fetches and verifies it into `resources/onnxruntime/`) and
// by `ort_runtime.rs` (which re-verifies the library before loading it), so
// the two can never disagree. Bump all values together.
//
// 1.24.2 is the ONNX Runtime line `ci/build_rag_index` embeds the shipped RAG
// index with (fastembed 5.x, pyke 1.24.2): query vectors are bit-identical to
// it. 1.31 moved bge-m3's INT8 outputs (mean cosine 0.982 against the index),
// so a newer pin also needs a RAG index generation built on that version.
//
// Source: Microsoft's own NuGet package `Microsoft.ML.OnnxRuntime` (the
// Windows DLL is signed "Microsoft Corporation"). pyke's prebuilt static
// library (the `ort` default) assumes AVX2/BMI2 and kills every pre-Haswell
// CPU at startup with STATUS_ILLEGAL_INSTRUCTION; the official library
// dispatches by CPU at run time. The macOS dylib needs macOS 14
// (`LC_BUILD_VERSION minos 14.0`), the app's minimum.

#[allow(dead_code)]
pub const ORT_VERSION: &str = "1.24.2";
#[allow(dead_code)]
pub const ORT_NUPKG_URL: &str = "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/1.24.2/microsoft.ml.onnxruntime.1.24.2.nupkg";
#[allow(dead_code)]
pub const ORT_NUPKG_SIZE: u64 = 124_756_536;
#[allow(dead_code)]
pub const ORT_NUPKG_SHA256: &str =
    "782564d3d68e87269ea1812cc94710ad06fe014faf093f01a6f89d0bd58719d3";
/// Licence files shipped with the library (MIT, and the third-party notices).
#[allow(dead_code)]
pub const ORT_LICENSE_ENTRIES: [&str; 2] = ["LICENSE", "ThirdPartyNotices.txt"];
/// Folder (under the resources root) holding the library.
#[allow(dead_code)]
pub const ORT_RESOURCE_DIR: &str = "onnxruntime";

/// One platform's library inside the package.
#[allow(dead_code)]
pub struct OrtLibrary {
    /// Package entry.
    pub entry: &'static str,
    /// File name it is shipped under, in `ORT_RESOURCE_DIR`.
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

#[allow(dead_code)]
pub const ORT_WINDOWS_X64: OrtLibrary = OrtLibrary {
    entry: "runtimes/win-x64/native/onnxruntime.dll",
    name: "onnxruntime.dll",
    size: 14_148_680,
    sha256: "114947d633e6844ce3c4b51ef6678f776628571d08a5763859c61642c8dcca9c",
};

#[allow(dead_code)]
pub const ORT_MACOS_ARM64: OrtLibrary = OrtLibrary {
    entry: "runtimes/osx-arm64/native/libonnxruntime.dylib",
    name: "libonnxruntime.dylib",
    size: 35_394_544,
    sha256: "8e88284d3b4395b1e532831ac05eb0ffa3673a62df32349e1b0fd64542cb827d",
};

/// The library a target ships, by Cargo's `target_os`/`target_arch` names.
#[allow(dead_code)]
pub fn ort_library_for(os: &str, arch: &str) -> Option<OrtLibrary> {
    match (os, arch) {
        ("windows", "x86_64") => Some(ORT_WINDOWS_X64),
        ("macos", "aarch64") => Some(ORT_MACOS_ARM64),
        _ => None,
    }
}
