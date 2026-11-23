//! The ONNX Runtime the RAG embedder runs on.
//!
//! The app ships Microsoft's official ONNX Runtime library as a bundle
//! resource (`onnxruntime/` beside the Windows exe, `Contents/Resources/
//! onnxruntime/` in the macOS app), staged by `build.rs` from the pin in
//! `ort_pin.rs`. It is checked against that pin and loaded by absolute path
//! only: never by bare name, which Windows would resolve to the older WinML
//! copy in System32. On Windows the file is held open against writers from the
//! check until the loader has mapped it. When the library is missing, altered
//! or does not load, the embedder is unavailable and RAG keeps answering with
//! keyword matches; nothing else in the app depends on it.

mod pin {
    include!("ort_pin.rs");
}

use pin::*;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

type Loaded = Result<ort::environment::Environment, String>;

static RUNTIME: OnceLock<Loaded> = OnceLock::new();

/// Hands the bge-m3 options the shipped runtime's environment. An error names
/// why the runtime is unavailable; callers must not create a session then,
/// because fastembed would otherwise load a library by bare name.
pub fn bgem3_options(
    options: fastembed::Bgem3InitOptions,
) -> Result<fastembed::Bgem3InitOptions, String> {
    let options = options.with_environment(environment()?);
    // The official Windows build runs bge-m3 2-3x slower when its intra-op
    // pool also takes the SMT siblings (measured on 8 cores / 16 threads:
    // 32 ms vs 13 ms per query); one thread per physical core is its best.
    Ok(match physical_cores() {
        Some(cores) => options.with_intra_threads(cores),
        None => options,
    })
}

/// Why the runtime could not be loaded, once a load was tried and failed.
/// `None` before the first attempt and whenever it loaded.
pub fn unavailable_reason() -> Option<String> {
    RUNTIME
        .get()
        .and_then(|loaded| loaded.as_ref().err().cloned())
}

fn environment() -> Loaded {
    RUNTIME.get_or_init(load).clone()
}

fn library() -> Result<OrtLibrary, String> {
    ort_library_for(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
        format!(
            "no ONNX Runtime ships for {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })
}

fn load() -> Loaded {
    let loaded = library().and_then(|library| {
        let path = locate(&library)?;
        load_verified(&path, &library)
    });
    if let Err(error) = &loaded {
        eprintln!("eud-agent: ONNX Runtime unavailable, RAG uses keyword matches only: {error}");
    }
    loaded
}

/// The resource folder holding the library: beside the exe (Windows, and
/// `cargo` builds), one folder up for test binaries in `target/<profile>/deps/`,
/// or `Contents/Resources/` of a macOS app bundle.
fn locate(library: &OrtLibrary) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| format!("no exe path: {error}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| format!("exe {} has no folder", exe.display()))?;
    let parent = dir.parent();
    let candidates = [
        Some(dir.to_path_buf()),
        parent.map(Path::to_path_buf),
        parent.map(|parent| parent.join("Resources")),
    ];
    for base in candidates.into_iter().flatten() {
        let path = base.join(ORT_RESOURCE_DIR).join(library.name);
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(format!(
        "{} is missing beside {}; reinstall eud-agent",
        library.name,
        exe.display()
    ))
}

fn load_verified(path: &Path, library: &OrtLibrary) -> Loaded {
    let mut file =
        open_exclusive(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut bytes = Vec::with_capacity(library.size as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 != library.size || hex(&Sha256::digest(&bytes)) != library.sha256 {
        return Err(format!(
            "{} is not the ONNX Runtime {ORT_VERSION} this build ships; reinstall eud-agent",
            path.display()
        ));
    }
    drop(bytes);
    let environment = ort::init_from(path)
        .map_err(|error| format!("cannot load {}: {error}", path.display()))?
        .with_name("eud-agent")
        .with_telemetry(false)
        .build()
        .map_err(|error| format!("ONNX Runtime {ORT_VERSION} did not start: {error}"))?;
    drop(file);
    Ok(environment)
}

/// Readers only, until the loader has mapped the file: on Windows the bytes
/// checked are the bytes loaded.
#[cfg(windows)]
fn open_exclusive(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x1;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
}

#[cfg(not(windows))]
fn open_exclusive(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Physical processor cores (one `RelationProcessorCore` record each).
#[cfg(windows)]
fn physical_cores() -> Option<usize> {
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, RelationProcessorCore,
        SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };
    let mut len = 0u32;
    // SAFETY: a null buffer only asks for the required length.
    unsafe {
        GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), &mut len)
    };
    if len == 0 {
        return None;
    }
    // u64 storage: the records are 8-byte aligned structures.
    let mut words = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: the buffer holds at least `len` bytes, the length the call asked for.
    let ok = unsafe {
        GetLogicalProcessorInformationEx(
            RelationProcessorCore,
            words
                .as_mut_ptr()
                .cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>(),
            &mut len,
        )
    };
    if ok == 0 {
        return None;
    }
    let buffer: Vec<u8> = words.iter().flat_map(|word| word.to_ne_bytes()).collect();
    // Records are variable-sized: `Relationship: u32` then `Size: u32`.
    let (mut offset, mut cores) = (0usize, 0usize);
    while offset + 8 <= len as usize {
        let size = u32::from_ne_bytes(buffer[offset + 4..offset + 8].try_into().ok()?) as usize;
        if size == 0 {
            break;
        }
        cores += 1;
        offset += size;
    }
    (cores > 0).then_some(cores)
}

/// Apple Silicon has no SMT siblings: fastembed's default (one thread per
/// logical CPU) is already one per core.
#[cfg(not(windows))]
fn physical_cores() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_runtime_loads_from_the_resource_folder() {
        let library = library().expect("this target ships a library");
        let path = locate(&library).expect("build.rs stages the library beside the test binary");
        assert!(path.ends_with(Path::new(ORT_RESOURCE_DIR).join(library.name)));
        environment().expect("the pinned ONNX Runtime loads and starts");
        assert_eq!(unavailable_reason(), None);
    }

    #[cfg(windows)]
    #[test]
    fn physical_cores_are_at_most_the_logical_ones() {
        let cores = physical_cores().expect("Windows reports its cores");
        let logical = std::thread::available_parallelism().unwrap().get();
        assert!(
            cores >= 1 && cores <= logical,
            "{cores} cores, {logical} logical"
        );
    }

    #[test]
    fn altered_library_is_never_loaded() {
        let library = library().unwrap();
        let shipped = std::fs::read(locate(&library).unwrap()).unwrap();
        let dir = std::env::temp_dir().join(format!("eud-ort-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let altered = dir.join(library.name);
        let mut bytes = shipped;
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&altered, &bytes).unwrap();

        let error = load_verified(&altered, &library).expect_err("a changed byte is refused");
        assert!(error.contains("is not the ONNX Runtime"), "got: {error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_release_target_has_a_pinned_library() {
        assert!(ort_library_for("windows", "x86_64").is_some());
        assert!(ort_library_for("macos", "aarch64").is_some());
        assert!(ort_library_for("macos", "x86_64").is_none());
    }
}
