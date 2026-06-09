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
    tauri_build::build();
}
