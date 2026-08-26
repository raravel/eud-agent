fn main() {
    // Re-supply the isom_capi.lib static-archive link directive that rustc dedups
    // away from this final binary (isom-sys declares links="isom_capi"). Mirror of
    // crates/isom/build.rs; required so the isom_* C ABI symbols resolve in the
    // eud-agent link. isom_capi.lib is built /MD (Decision 14) -- no CRT-forcing args.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
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
    tauri_build::build();
}
