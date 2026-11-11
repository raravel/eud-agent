//! In-process DevTools Protocol calls on a tab webview (Windows).
//!
//! WebView2 is a single-threaded COM object: every call is made inside
//! `Webview::with_webview`, i.e. on the UI thread, and its completed handler
//! fires there too. The handler only sends the JSON text through a channel;
//! no COM pointer ever leaves the UI thread. Event receivers must stay alive
//! for their handlers to keep firing, so they live in a UI-thread
//! `thread_local!` keyed by the tab's label until the tab closes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;
use tauri::Webview;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2DevToolsProtocolEventReceivedEventArgs2,
    ICoreWebView2DevToolsProtocolEventReceiver, ICoreWebView2_11,
};
use webview2_com::{
    take_pwstr, CallDevToolsProtocolMethodCompletedHandler,
    DevToolsProtocolEventReceivedEventHandler,
};
use windows_core::{Interface, HSTRING, PWSTR};

/// Default bound of one protocol call.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(15);

thread_local! {
    /// Live event receivers and their registration tokens, by tab label.
    static RECEIVERS: RefCell<HashMap<String, Vec<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>>> =
        RefCell::new(HashMap::new());
}

/// Call `method` with `params` on `webview` and wait at most `timeout`.
pub async fn call_with(
    webview: &Webview,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, String> {
    call_in_with(webview, None, method, params, timeout).await
}

/// [`call_with`] in the DevTools `session` of an attached target (an
/// out-of-process iframe, `Target.setAutoAttach` with `flatten`), or in the
/// tab's own page target when `None`.
pub async fn call_in_with(
    webview: &Webview,
    session: Option<&str>,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<String, String>>();
    let sender = Arc::new(Mutex::new(Some(tx)));
    let (name, body) = (method.to_string(), params.to_string());
    let session = session.map(str::to_string);
    let failed = Arc::clone(&sender);
    webview
        .with_webview(move |platform| {
            let fail = |message: String| {
                if let Some(tx) = failed.lock().take() {
                    let _ = tx.send(Err(message));
                }
            };
            // SAFETY: the controller is used on the UI thread it belongs to,
            // inside the closure `with_webview` runs there.
            let core = match unsafe { platform.controller().CoreWebView2() } {
                Ok(core) => core,
                Err(error) => return fail(format!("CoreWebView2: {error}")),
            };
            let done = Arc::clone(&failed);
            let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                move |result: windows_core::Result<()>, json: String| {
                    if let Some(tx) = done.lock().take() {
                        let _ = tx.send(match result {
                            Ok(()) => Ok(json),
                            Err(error) => Err(format!("{error}; {json}")),
                        });
                    }
                    Ok(())
                },
            ));
            // SAFETY: as above, on the UI thread.
            let called = unsafe {
                match &session {
                    None => core.CallDevToolsProtocolMethod(
                        &HSTRING::from(name),
                        &HSTRING::from(body),
                        &handler,
                    ),
                    Some(session) => core.cast::<ICoreWebView2_11>().and_then(|core| {
                        core.CallDevToolsProtocolMethodForSession(
                            &HSTRING::from(session.as_str()),
                            &HSTRING::from(name),
                            &HSTRING::from(body),
                            &handler,
                        )
                    }),
                }
            };
            if let Err(error) = called {
                fail(format!("CallDevToolsProtocolMethod: {error}"));
            }
        })
        .map_err(|error| {
            format!(
                "탭에 접근할 수 없습니다({error}). 탭이 닫혔다면 browser_open 으로 다시 여세요."
            )
        })?;
    let raw = match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(result)) => result.map_err(|error| format!("{method} 실패: {error}"))?,
        Ok(Err(_)) => {
            return Err(format!(
                "{method}: 탭이 응답 전에 닫혔습니다. 필요하면 browser_open 으로 다시 여세요."
            ))
        }
        Err(_) => {
            drop(sender);
            return Err(format!(
                "{method}: {}초 안에 응답이 없습니다. 페이지가 멈췄거나 창이 최소화되어 있을 수 있습니다; 브라우저 창을 복원한 뒤 다시 시도하세요.",
                timeout.as_secs()
            ));
        }
    };
    serde_json::from_str(&raw)
        .map_err(|error| format!("{method}: 응답을 해석할 수 없습니다({error})"))
}

pub async fn call(webview: &Webview, method: &str, params: Value) -> Result<Value, String> {
    call_with(webview, method, params, CALL_TIMEOUT).await
}

pub async fn call_in(
    webview: &Webview,
    session: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    call_in_with(webview, session, method, params, CALL_TIMEOUT).await
}

/// Forward every `event` of `webview`'s page target into the returned
/// channel until the tab closes ([`unsubscribe_all`]). Events of attached
/// targets (flattened sessions) are left out; see [`subscribe_sessions`].
pub async fn subscribe(
    webview: &Webview,
    event: &str,
) -> Result<tokio::sync::mpsc::UnboundedReceiver<Value>, String> {
    let (events, received) = tokio::sync::mpsc::unbounded_channel::<Value>();
    register(webview, event, move |session, text| {
        if session.is_none() {
            let _ = events.send(parse_event(text));
        }
    })
    .await?;
    Ok(received)
}

/// Forward every `event` of `webview`, from its page target (`None`) and
/// from every attached target, with that target's session id.
pub async fn subscribe_sessions(
    webview: &Webview,
    event: &str,
) -> Result<tokio::sync::mpsc::UnboundedReceiver<(Option<String>, Value)>, String> {
    subscribe_sessions_matching(webview, event, &[]).await
}

/// [`subscribe_sessions`], keeping only events whose JSON text contains one
/// of `needles` (all events when empty). The text is checked on the UI
/// thread before it is parsed or sent, so a frequent event (every network
/// response) costs a substring search unless it is wanted.
pub async fn subscribe_sessions_matching(
    webview: &Webview,
    event: &str,
    needles: &'static [&'static str],
) -> Result<tokio::sync::mpsc::UnboundedReceiver<(Option<String>, Value)>, String> {
    let (events, received) = tokio::sync::mpsc::unbounded_channel();
    register(webview, event, move |session, text| {
        if needles.is_empty() || needles.iter().any(|needle| text.contains(needle)) {
            let _ = events.send((session, parse_event(text)));
        }
    })
    .await?;
    Ok(received)
}

/// Like [`subscribe_sessions_matching`], with a filter decided per event on
/// the UI thread (it must be cheap), so an event stream the app needs only
/// now and then is not parsed the rest of the time.
pub async fn subscribe_sessions_filtered(
    webview: &Webview,
    event: &str,
    keep: impl Fn(&str) -> bool + Send + 'static,
) -> Result<tokio::sync::mpsc::UnboundedReceiver<(Option<String>, Value)>, String> {
    let (events, received) = tokio::sync::mpsc::unbounded_channel();
    register(webview, event, move |session, text| {
        if keep(&text) {
            let _ = events.send((session, parse_event(text)));
        }
    })
    .await?;
    Ok(received)
}

fn parse_event(text: String) -> Value {
    serde_json::from_str(&text).unwrap_or(Value::String(text))
}

async fn register(
    webview: &Webview,
    event: &str,
    sink: impl Fn(Option<String>, String) + Send + 'static,
) -> Result<(), String> {
    let (ack, acked) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let label = webview.label().to_string();
    let name = event.to_string();
    webview
        .with_webview(move |platform| {
            let registered = (|| -> windows_core::Result<()> {
                // SAFETY: on the UI thread, inside `with_webview`.
                unsafe {
                    let core = platform.controller().CoreWebView2()?;
                    let receiver = core.GetDevToolsProtocolEventReceiver(&HSTRING::from(name))?;
                    let mut token = 0i64;
                    receiver.add_DevToolsProtocolEventReceived(
                        &DevToolsProtocolEventReceivedEventHandler::create(Box::new(
                            move |_, args| {
                                if let Some(args) = args {
                                    let mut json = PWSTR::null();
                                    args.ParameterObjectAsJson(&mut json)?;
                                    let text = take_pwstr(json);
                                    // Events of a flattened child session
                                    // name it (empty for the page target).
                                    let session = args
                                        .cast::<ICoreWebView2DevToolsProtocolEventReceivedEventArgs2>()
                                        .ok()
                                        .and_then(|args| {
                                            let mut id = PWSTR::null();
                                            args.SessionId(&mut id).ok()?;
                                            Some(take_pwstr(id))
                                        })
                                        .filter(|id| !id.is_empty());
                                    sink(session, text);
                                }
                                Ok(())
                            },
                        )),
                        &mut token,
                    )?;
                    RECEIVERS.with(|receivers| {
                        receivers
                            .borrow_mut()
                            .entry(label)
                            .or_default()
                            .push((receiver, token));
                    });
                }
                Ok(())
            })();
            let _ = ack.send(registered.map_err(|error| error.to_string()));
        })
        .map_err(|error| format!("탭에 접근할 수 없습니다({error})"))?;
    match tokio::time::timeout(CALL_TIMEOUT, acked).await {
        Ok(Ok(result)) => result.map_err(|error| format!("{event} 구독 실패: {error}")),
        _ => Err(format!("{event} 구독이 응답하지 않습니다")),
    }
}

// Drop every event receiver of `webview` (before it closes).
pub fn unsubscribe_all(webview: &Webview) {
    let label = webview.label().to_string();
    let _ = webview.with_webview(move |_| forget(&label));
}

/// Drop the receivers of tab `label` on the UI thread (also after its webview
/// is gone, when the window was destroyed with it).
pub fn forget(label: &str) {
    let removed = RECEIVERS.with(|receivers| receivers.borrow_mut().remove(label));
    for (receiver, token) in removed.into_iter().flatten() {
        // SAFETY: on the UI thread that registered the receiver.
        let _ = unsafe { receiver.remove_DevToolsProtocolEventReceived(token) };
    }
}

/// Put tab `webview` above its siblings, so it is the one the user sees.
pub fn bring_to_front(webview: &Webview) -> Result<(), String> {
    webview
        .with_webview(|platform| {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
            };
            let mut hwnd = Default::default();
            // SAFETY: on the UI thread; the container HWND wry created for
            // this child webview belongs to the browser window.
            unsafe {
                if platform.controller().ParentWindow(&mut hwnd).is_ok() {
                    SetWindowPos(
                        hwnd.0,
                        HWND_TOP,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            }
        })
        .map_err(|error| format!("탭에 접근할 수 없습니다({error})"))
}

/// Install the page guards that live on the WebView2 object itself:
/// `on_popup(uri, user_initiated)` hears every window the page asks for
/// (before the webview's own new-window handler decides it, which runs
/// after this handler returns), and every external protocol launch
/// (`mailto:`, `ms-settings:`, an app's own scheme) is cancelled, so no page
/// can start an OS handler. Either guard failing to install (a WebView2
/// runtime too old for `ICoreWebView2_18`) is an error: the tab is not
/// created without them.
pub async fn install_page_guards(
    webview: &Webview,
    on_popup: impl Fn(String, bool) + Send + 'static,
) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_18;
    use webview2_com::{LaunchingExternalUriSchemeEventHandler, NewWindowRequestedEventHandler};
    let (ack, acked) = tokio::sync::oneshot::channel::<Result<(), String>>();
    webview
        .with_webview(move |platform| {
            // SAFETY: on the UI thread, inside `with_webview`.
            let installed = unsafe {
                (|| -> Result<(), String> {
                    let core = platform
                        .controller()
                        .CoreWebView2()
                        .map_err(|error| format!("CoreWebView2: {error}"))?;
                    let mut token = 0i64;
                    let popups = core.add_NewWindowRequested(
                        &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                            if let Some(args) = args {
                                let mut uri = PWSTR::null();
                                args.Uri(&mut uri)?;
                                let mut user = windows_core::BOOL::default();
                                let _ = args.IsUserInitiated(&mut user);
                                on_popup(take_pwstr(uri), user.as_bool());
                            }
                            Ok(())
                        })),
                        &mut token,
                    );
                    popups.map_err(|error| format!("popup guard: {error}"))?;
                    let external = core.cast::<ICoreWebView2_18>().and_then(|core| {
                        core.add_LaunchingExternalUriScheme(
                            &LaunchingExternalUriSchemeEventHandler::create(Box::new(|_, args| {
                                if let Some(args) = args {
                                    args.SetCancel(true)?;
                                    let mut uri = PWSTR::null();
                                    let _ = args.Uri(&mut uri);
                                    eprintln!(
                                    "eud-agent: browser refused an external protocol launch: {}",
                                    take_pwstr(uri)
                                );
                                }
                                Ok(())
                            })),
                            &mut token,
                        )
                    });
                    external.map_err(|error| format!("external-scheme guard: {error}"))
                })()
            };
            let _ = ack.send(installed);
        })
        .map_err(|error| format!("탭에 접근할 수 없습니다({error})"))?;
    match tokio::time::timeout(CALL_TIMEOUT, acked).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(error))) => Err(format!(
            "탭의 보호 장치를 설치할 수 없어 탭을 열지 않았습니다({error}). Microsoft Edge WebView2 런타임을 최신으로 업데이트한 뒤 다시 시도하세요."
        )),
        _ => Err("탭의 보호 장치 설치가 응답하지 않아 탭을 열지 않았습니다. 다시 시도하세요.".to_string()),
    }
}
