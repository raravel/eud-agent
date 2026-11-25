//! Project HTML preview: the custom protocol behind the file viewer's
//! rendered `.html` tabs.
//!
//! `eudpreview://localhost/<project path>` (`http://eudpreview.localhost/...`
//! on Windows) answers only a project's own main window, from that window's
//! project root. A page runs scripts and may use the internet, so what it can
//! read of the project is limited to web asset types outside hidden folders:
//! never `.eps`/`.py` sources (a MapVault SDK holds keys), maps, the manifest,
//! `.git` or `.eud-agent` state.
//!
//! Isolation: the panel's iframe is sandboxed without `allow-same-origin` and
//! every response repeats that as a CSP `sandbox`, so the page's origin is
//! opaque. That origin is what keeps it from the app's commands: `ipc://`
//! requests carry `Origin: null`, which Tauri refuses. Tauri counts this
//! scheme as local and, on Windows, runs its init scripts (invoke key
//! included) in frames, so the other IPC path must stay closed too: WebView2
//! never delivers a frame's `postMessage` to the top-level handler (do not add
//! a frame message handler), and on macOS the key stays in the main frame (do
//! not add an init script for all frames).

use std::path::Path;

use tauri::http::{header, Method, Request, Response, StatusCode};

pub const SCHEME: &str = "eudpreview";

/// Largest file one request serves.
const MAX_PREVIEW_BYTES: u64 = 64 * 1024 * 1024;

/// The only hidden project folder whose files are served: agent documents.
const DOCUMENT_FOLDER: [&str; 2] = [".eud-agent", "workspace"];

/// Same flags as the panel's iframe (`HtmlPreview.tsx`).
const SANDBOX: &str = "sandbox allow-scripts allow-forms";

/// Appended to every HTML page. Links inside the project stay in the frame
/// instead of asking for a new window; other http(s) links are handed to the
/// panel, which opens them in the system browser. A page's own handler that
/// cancels the click wins, except on `_blank` links: Tauri's shell script,
/// which WebView2 runs in frames too, cancels those. It goes after the
/// document's end, so it never changes how the page's doctype is read.
const LINK_SCRIPT: &str = r#"
<script>(function(){function follow(event,blank){
if(event.button!==0||event.defaultPrevented)return;
var link=event.composedPath().find(function(node){return node.matches&&node.matches("a[href],area[href]")});
if(!link||!/^(https?|eudpreview):$/.test(link.protocol)||(link.target==="_blank")!==blank)return;
var inside=link.href.indexOf(location.protocol+"//"+location.host+"/")===0;
if(inside&&!/^_(blank|top|parent)$/.test(link.target))return;
event.preventDefault();event.stopImmediatePropagation();
if(inside)location.href=link.href;
else parent.postMessage({type:"eud-preview-link",href:link.href},"*");
}
addEventListener("click",function(event){follow(event,true)},true);
addEventListener("click",function(event){follow(event,false)});
})();</script>
"#;

#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    NotFound,
    NotAllowed,
    TooLarge,
}

#[derive(Debug)]
struct Asset {
    content_type: &'static str,
    bytes: Vec<u8>,
}

/// Answers one protocol request of the webview `label`. Blocking.
pub fn respond<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    label: &str,
    request: &Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    use tauri::Manager;
    if !admits(request.method(), label) {
        return refuse(Refusal::NotAllowed);
    }
    let Some(handle) = app
        .try_state::<crate::project_registry::ProjectRegistry>()
        .and_then(|registry| registry.for_window(label).ok())
    else {
        return refuse(Refusal::NotFound);
    };
    match asset_for(handle.root(), request.uri().path()) {
        Ok(asset) => serve(asset),
        Err(refusal) => refuse(refusal),
    }
}

/// A browser tab, Map window or importer never loads a preview.
fn admits(method: &Method, label: &str) -> bool {
    method == Method::GET && crate::project_launch::is_project_window(label)
}

/// The file one request path names, read under the policy above.
fn asset_for(root: &Path, uri_path: &str) -> Result<Asset, Refusal> {
    let relative = percent_encoding::percent_decode_str(uri_path.trim_start_matches('/'))
        .decode_utf8()
        .map_err(|_| Refusal::NotFound)?;
    let root = std::fs::canonicalize(root).map_err(|_| Refusal::NotFound)?;
    let path =
        crate::workspace::confined_project_path(&root, &relative).map_err(|_| Refusal::NotFound)?;
    // The policy is decided on the file on disk, so another case, an 8.3 name
    // or a link spells nothing new.
    let path = std::fs::canonicalize(path).map_err(|_| Refusal::NotFound)?;
    let on_disk = path.strip_prefix(&root).map_err(|_| Refusal::NotAllowed)?;
    let content_type = served_type(on_disk).ok_or(Refusal::NotAllowed)?;
    let metadata = std::fs::metadata(&path).map_err(|_| Refusal::NotFound)?;
    if !metadata.is_file() {
        return Err(Refusal::NotFound);
    }
    if metadata.len() > MAX_PREVIEW_BYTES {
        return Err(Refusal::TooLarge);
    }
    let bytes = crate::workspace::read_bounded_regular_file(&path, MAX_PREVIEW_BYTES)
        .map_err(|_| Refusal::NotFound)?;
    Ok(Asset {
        content_type,
        bytes,
    })
}

/// The content type of a project-relative path a page may read, else `None`.
fn served_type(relative: &Path) -> Option<&'static str> {
    let components = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    let visible = components.starts_with(&DOCUMENT_FOLDER)
        || components.iter().all(|name| !name.starts_with('.'));
    if !visible {
        return None;
    }
    let extension = relative.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" | "map" => "application/json",
        "csv" => "text/csv",
        "txt" | "md" => "text/plain",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => return None,
    })
}

fn serve(asset: Asset) -> Response<Vec<u8>> {
    let Asset {
        content_type,
        mut bytes,
    } = asset;
    let text = content_type.starts_with("text/") || content_type == "application/json";
    // A page in another encoding keeps its own `<meta charset>`.
    let header = if text && std::str::from_utf8(&bytes).is_ok() {
        format!("{content_type}; charset=utf-8")
    } else {
        content_type.to_string()
    };
    // ASCII appended to UTF-16 would show as garbage text.
    let utf16 = bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]);
    if content_type == "text/html" && !utf16 {
        bytes.extend_from_slice(LINK_SCRIPT.as_bytes());
    }
    response(StatusCode::OK, &header, bytes)
}

fn refuse(refusal: Refusal) -> Response<Vec<u8>> {
    let (status, message) = match refusal {
        Refusal::NotFound => (
            StatusCode::NOT_FOUND,
            crate::tr!("project.preview_not_found"),
        ),
        Refusal::NotAllowed => (
            StatusCode::FORBIDDEN,
            crate::tr!("project.preview_not_allowed"),
        ),
        Refusal::TooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE,
            crate::tr!("project.preview_too_large"),
        ),
    };
    response(status, "text/plain; charset=utf-8", message.into_bytes())
}

fn response(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_SECURITY_POLICY, SANDBOX)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        // Modules, fonts and fetches of the sandboxed page: its origin is "null".
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "null")
        .body(body)
        .expect("static preview response headers")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct TestDir(PathBuf);

    impl TestDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn project(files: &[(&str, &[u8])]) -> TestDir {
        let dir = TestDir(
            std::env::temp_dir().join(format!("eud-html-preview-{}", uuid::Uuid::new_v4())),
        );
        for (path, bytes) in files {
            let path = dir.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        dir
    }

    fn refusal(root: &Path, path: &str) -> Option<Refusal> {
        asset_for(root, path).err()
    }

    #[test]
    fn serves_pages_and_their_relative_assets() {
        let dir = project(&[
            ("docs/가이드 1.html", b"<!doctype html><p>hi"),
            ("docs/style.css", b"p{}"),
            (".eud-agent/workspace/specs/report.html", b"<p>spec"),
        ]);
        let page = asset_for(dir.path(), "/docs/%EA%B0%80%EC%9D%B4%EB%93%9C%201.html").unwrap();
        assert_eq!(page.content_type, "text/html");
        assert_eq!(page.bytes, b"<!doctype html><p>hi");
        assert_eq!(
            asset_for(dir.path(), "/docs/style.css")
                .unwrap()
                .content_type,
            "text/css"
        );
        assert!(asset_for(dir.path(), "/.eud-agent/workspace/specs/report.html").is_ok());
    }

    #[test]
    fn never_serves_sources_hidden_folders_or_paths_outside_the_project() {
        let dir = project(&[
            ("project/src/mapvault.eps", b"var rke0 = 1;"),
            ("project/src/tool.py", b"x"),
            ("project/project.eap", b"{}"),
            ("project/maps/source.scx", b"x"),
            ("project/.git/config.json", b"{}"),
            ("project/.mcp.json", b"{}"),
            ("project/.eud-agent/state/workspace.json", b"{}"),
            ("project/.eud-agent/workspace.json", b"{}"),
            ("project/docs/page.html", b"x"),
            ("outside.html", b"x"),
        ]);
        let root = dir.path().join("project");
        for path in [
            "/src/mapvault.eps",
            "/src/tool.py",
            "/project.eap",
            "/maps/source.scx",
            "/.git/config.json",
            "/.mcp.json",
            "/.eud-agent/state/workspace.json",
            "/.eud-agent/workspace.json",
        ] {
            assert_eq!(refusal(&root, path), Some(Refusal::NotAllowed), "{path}");
        }
        for path in [
            "/",
            "/missing.html",
            "/docs",
            "/../outside.html",
            "/%2e%2e/outside.html",
            "/docs%5Cpage.html",
            "/C:/Windows/win.ini",
            "/docs/page.html::$DATA",
            "/%FF.html",
        ] {
            assert!(refusal(&root, path).is_some(), "{path}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn decides_the_policy_on_the_name_on_disk() {
        let dir = project(&[(".git/config.json", b"{}"), ("docs/page.html", b"x")]);
        assert_eq!(
            refusal(dir.path(), "/.GIT/config.json"),
            Some(Refusal::NotAllowed)
        );
        // Windows drops a trailing dot, so this spelling names the same file.
        assert!(refusal(dir.path(), "/.git./config.json").is_some());
        assert!(asset_for(dir.path(), "/DOCS/PAGE.HTML").is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_out_of_the_project_serves_nothing() {
        let dir = project(&[("project/a.txt", b"x"), ("outside/page.html", b"x")]);
        let root = dir.path().join("project");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(root.join("docs"))
            .arg(dir.path().join("outside"))
            .output()
            .unwrap();
        assert!(made.status.success());
        assert_eq!(refusal(&root, "/docs/page.html"), Some(Refusal::NotAllowed));
        fs::remove_dir(root.join("docs")).unwrap();
    }

    #[test]
    fn every_answer_keeps_the_page_in_an_opaque_origin() {
        let page = serve(Asset {
            content_type: "text/html",
            bytes: b"<!doctype html><p>hi".to_vec(),
        });
        for answer in [&page, &refuse(Refusal::NotFound)] {
            let policy = answer.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap();
            assert!(policy.starts_with("sandbox "));
            assert!(!policy.contains("allow-same-origin"));
            assert!(!policy.contains("allow-top-navigation"));
        }
        assert_eq!(
            page.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert!(page.body().starts_with(b"<!doctype html>"));
        assert!(page.body().ends_with(LINK_SCRIPT.as_bytes()));
        let legacy = serve(Asset {
            content_type: "text/html",
            bytes: vec![0xb0, 0xa1],
        });
        assert_eq!(legacy.headers()[header::CONTENT_TYPE], "text/html");
        let utf16 = vec![0xff, 0xfe, b'h', 0];
        let wide = serve(Asset {
            content_type: "text/html",
            bytes: utf16.clone(),
        });
        assert_eq!(wide.body(), &utf16);
    }

    #[test]
    fn answers_only_reads_of_a_project_window() {
        assert!(admits(&Method::GET, "main"));
        assert!(admits(&Method::GET, "main-0123456789ab"));
        assert!(!admits(&Method::POST, "main"));
        for label in [
            "browser",
            "browser-tab-1",
            "map-agent-0123",
            "map-import-0123",
        ] {
            assert!(!admits(&Method::GET, label), "{label}");
        }
    }
}
