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
    let mut windows = tauri_build::WindowsAttributes::new();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // The linker embeds the manifest (above).
        windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    }
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
}
