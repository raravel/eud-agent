fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    // Re-supply the isom_capi.lib static-archive link directive that rustc dedups
    // away from this final binary (isom-sys declares links="isom_capi"). Mirror of
    // crates/isom/build.rs; required so the isom_* C ABI symbols resolve in the
    // eud-agent link. isom_capi.lib is built /MD (Decision 14) -- no CRT-forcing args.
    // Non-MSVC targets link the `cc`-built archive bundled in the isom-sys rlib.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let lib_dir = std::path::Path::new(&manifest_dir)
            .join("..")
            .join("native")
            .join("isom")
            .join("x64")
            .join("ReleaseUS");
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
        println!(
            "cargo:rustc-link-arg={}",
            lib_dir.join("isom_capi.lib").display()
        );
        // Common Controls v6 entry points (TaskDialogIndirect, imported with
        // the tray icon) resolve only under a v6 manifest. The linker embeds
        // it into every binary, test binaries included, in place of
        // tauri-build's app-only manifest resource, or tests fail to start.
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let injector_source = std::path::Path::new(&manifest_dir)
            .join("..")
            .join("native")
            .join("trace_injector.rs");
        let injector_exe =
            std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("eud_trace_injector.exe");
        let status = std::process::Command::new(std::env::var("RUSTC").unwrap())
            .args([
                "--crate-name",
                "eud_trace_injector",
                "--crate-type",
                "bin",
                "--edition",
                "2021",
                "--target",
                "i686-pc-windows-msvc",
                "-C",
                "opt-level=z",
                "-C",
                "panic=abort",
                "-C",
                "strip=symbols",
                "-C",
                "lto=fat",
                "-C",
                "codegen-units=1",
            ])
            .arg(&injector_source)
            .arg("-o")
            .arg(&injector_exe)
            .status()
            .expect("failed to invoke rustc for the x86 trace injector");
        assert!(status.success(), "failed to build the x86 trace injector");
        println!("cargo:rerun-if-changed={}", injector_source.display());
        println!(
            "cargo:rustc-env=EUD_TRACE_INJECTOR_EXE={}",
            injector_exe.display()
        );
    }
    // The app loads the official ONNX Runtime library from its bundle
    // (`src/ort_runtime.rs`); stage it as a resource before tauri-build checks
    // the resources exist.
    onnxruntime::stage(std::path::Path::new(&manifest_dir));
    let mut windows = tauri_build::WindowsAttributes::new();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // The linker embeds the manifest (above).
        windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    }
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
}

/// Fetches the pinned official ONNX Runtime package once, checks it, and puts
/// the target's library and the licence files in `resources/onnxruntime/`.
/// The download is cached under the workspace `target/onnxruntime/<version>/`.
mod onnxruntime {
    include!("src/ort_pin.rs");

    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub fn stage(manifest_dir: &Path) {
        println!("cargo:rerun-if-changed=src/ort_pin.rs");
        let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
        let library = ort_library_for(&os, &arch).unwrap_or_else(|| {
            panic!("no ONNX Runtime is pinned for {os}/{arch} (src/ort_pin.rs)")
        });
        let out = manifest_dir.join("resources").join(ORT_RESOURCE_DIR);
        let staged_library = out.join(library.name);
        println!("cargo:rerun-if-changed={}", staged_library.display());
        let staged = matches(&staged_library, library.size, library.sha256)
            && ORT_LICENSE_ENTRIES
                .iter()
                .all(|name| out.join(name).is_file());
        if staged {
            return;
        }

        let cache = manifest_dir
            .join("..")
            .join("target")
            .join("onnxruntime")
            .join(ORT_VERSION);
        std::fs::create_dir_all(&cache).expect("create ONNX Runtime cache folder");
        // Build scripts of other target dirs (rust-analyzer, a second
        // checkout of the profile) share the cache: every temporary is ours.
        let pid = std::process::id();
        let nupkg = cache.join("onnxruntime.nupkg");
        if !matches(&nupkg, ORT_NUPKG_SIZE, ORT_NUPKG_SHA256) {
            let partial = cache.join(format!("onnxruntime.nupkg.{pid}.part"));
            run(Command::new(host_tool("curl"))
                .args([
                    "--fail",
                    "--location",
                    "--silent",
                    "--show-error",
                    "--output",
                ])
                .arg(&partial)
                .arg(ORT_NUPKG_URL));
            assert!(
                matches(&partial, ORT_NUPKG_SIZE, ORT_NUPKG_SHA256),
                "downloaded {ORT_NUPKG_URL} does not match the pinned size/SHA-256 in src/ort_pin.rs"
            );
            if std::fs::rename(&partial, &nupkg).is_err() {
                // Another build published it first.
                let _ = std::fs::remove_file(&partial);
                assert!(
                    matches(&nupkg, ORT_NUPKG_SIZE, ORT_NUPKG_SHA256),
                    "cannot publish the ONNX Runtime package in {}",
                    cache.display()
                );
            }
        }

        let extract = cache.join(format!("extract-{pid}"));
        let _ = std::fs::remove_dir_all(&extract);
        std::fs::create_dir_all(&extract).expect("create ONNX Runtime extract folder");
        // bsdtar (Windows' own and macOS') reads zip archives; a .nupkg is
        // one. `-m`: the package's entry times cannot be restored on Windows.
        run(Command::new(host_tool("tar"))
            .arg("-xmf")
            .arg(&nupkg)
            .arg("-C")
            .arg(&extract)
            .arg(library.entry)
            .args(ORT_LICENSE_ENTRIES));
        let extracted = extract.join(library.entry);
        assert!(
            matches(&extracted, library.size, library.sha256),
            "{} in the pinned package does not match src/ort_pin.rs",
            library.entry
        );

        std::fs::create_dir_all(&out).expect("create resources/onnxruntime");
        for name in ORT_LICENSE_ENTRIES {
            std::fs::copy(extract.join(name), out.join(name)).expect("stage ONNX Runtime licence");
        }
        let temporary = out.join(format!("{}.{pid}.part", library.name));
        std::fs::copy(&extracted, &temporary).expect("stage ONNX Runtime library");
        if std::fs::rename(&temporary, &staged_library).is_err() {
            let _ = std::fs::remove_file(&temporary);
            assert!(
                matches(&staged_library, library.size, library.sha256),
                "cannot publish {}",
                staged_library.display()
            );
        }
        let _ = std::fs::remove_dir_all(&extract);
    }

    fn matches(path: &Path, size: u64, sha256: &str) -> bool {
        let Ok(metadata) = std::fs::metadata(path) else {
            return false;
        };
        if metadata.len() != size {
            return false;
        }
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        let digest = Sha256::digest(&bytes);
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            == sha256
    }

    /// The build host's own `curl`/`tar` (both ship with Windows 10+ and
    /// macOS), never whatever another toolchain put first on PATH.
    fn host_tool(name: &str) -> PathBuf {
        if cfg!(windows) {
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            PathBuf::from(root)
                .join("System32")
                .join(format!("{name}.exe"))
        } else {
            PathBuf::from("/usr/bin").join(name)
        }
    }

    fn run(command: &mut Command) {
        let status = command
            .status()
            .unwrap_or_else(|error| panic!("cannot run {command:?}: {error}"));
        assert!(status.success(), "{command:?} failed with {status}");
    }
}
