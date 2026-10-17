//! The tray icon that keeps the app reachable while remote access is on.
//!
//! With remote access on, closing the last window does not end the process:
//! paired devices keep working on open projects. The tray icon is then the
//! way back: it opens a window, shows where remote access listens, and quits
//! for real. It exists only while remote access is on.

use std::sync::Arc;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::Manager;

use super::RemoteServer;

const TRAY_ID: &str = "eud-agent-remote";
const MENU_OPEN: &str = "remote-tray-open";
const MENU_STATUS: &str = "remote-tray-status";
const MENU_QUIT: &str = "remote-tray-quit";

/// Whether an exit request is held back: only one the user caused by closing
/// the last window (`code: None`), and only while remote access is on. An
/// explicit quit (`AppHandle::exit`) always exits.
pub fn should_prevent_exit(code: Option<i32>, remote_enabled: bool) -> bool {
    code.is_none() && remote_enabled
}

/// The tray menu's status line: the MagicDNS name phones reach, or the
/// loopback address when Tailscale does not name this PC.
pub fn status_label(dns_name: Option<&str>, port: u16) -> String {
    match dns_name {
        Some(name) => format!("원격 접속: 켜짐 · {name}"),
        None => format!("원격 접속: 켜짐 · 127.0.0.1:{port}"),
    }
}

/// Show the tray icon while remote access is on and remove it otherwise.
pub async fn sync(app: &tauri::AppHandle) {
    let Some(server) = app.try_state::<Arc<RemoteServer>>() else {
        return;
    };
    let server = server.inner().clone();
    if !server.is_enabled() {
        app.remove_tray_by_id(TRAY_ID);
        return;
    }
    let port = server
        .bound_port()
        .await
        .or_else(|| {
            server
                .dirs()
                .load_config()
                .ok()
                .map(|config| config.remote.port)
        })
        .unwrap_or_default();
    let dns_name = super::tailscale::status(port).await.dns_name;
    let label = status_label(dns_name.as_deref(), port);
    if let Err(error) = show(app, &label) {
        eprintln!("eud-agent: remote tray icon unavailable: {error}");
    }
}

fn menu(app: &tauri::AppHandle, status: &str) -> tauri::Result<Menu<tauri::Wry>> {
    Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, MENU_OPEN, "창 열기", true, None::<&str>)?,
            &MenuItem::with_id(app, MENU_STATUS, status, false, None::<&str>)?,
            &MenuItem::with_id(app, MENU_QUIT, "종료", true, None::<&str>)?,
        ],
    )
}

fn show(app: &tauri::AppHandle, status: &str) -> tauri::Result<()> {
    let menu = menu(app, status)?;
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_menu(Some(menu))?;
        return tray.set_tooltip(Some(status));
    }
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(status)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_OPEN => open_window(app),
            MENU_QUIT => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// Focus a main window, or open one at the launcher when none is left.
fn open_window(app: &tauri::AppHandle) {
    // Off the event loop's thread: creating a webview there can stall it.
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = crate::project_launch::show_main_window(&app) {
            eprintln!("eud-agent: tray could not open a window: {error}");
        }
    });
}

/// Quit for real. Work in progress is handled as closing the last window
/// handles it: while any project has work the user is asked first, and a
/// confirmed quit cancels that work instead of waiting for it.
fn quit(app: &tauri::AppHandle) {
    let Some(engine) = app.try_state::<crate::engine::SessionEngineManager>() else {
        app.exit(0);
        return;
    };
    let Some(blocker) = engine.quit_blocker() else {
        engine.close_all_for_quit();
        app.exit(0);
        return;
    };
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let target = app.clone();
    app.dialog()
        .message(format!(
            "{blocker}

그래도 종료하면 진행 중인 작업을 중단합니다. 대화와 검토 대기 중인 계획은 프로젝트를 다시 열면 그대로 남아 있습니다."
        ))
        .title("종료할까요?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "종료".to_string(),
            "취소".to_string(),
        ))
        .show(move |quit| {
            if quit {
                if let Some(engine) = target.try_state::<crate::engine::SessionEngineManager>() {
                    engine.close_all_for_quit();
                }
                target.exit(0);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_last_window_close_with_remote_on_is_held_back() {
        assert!(should_prevent_exit(None, true));
        assert!(!should_prevent_exit(None, false));
        // The tray's 종료 and the updater's restart exit with a code.
        assert!(!should_prevent_exit(Some(0), true));
        assert!(!should_prevent_exit(Some(i32::MAX), true));
    }

    #[test]
    fn the_status_line_names_where_phones_connect() {
        assert_eq!(
            status_label(Some("pc.tail1234.ts.net"), 47821),
            "원격 접속: 켜짐 · pc.tail1234.ts.net"
        );
        assert_eq!(
            status_label(None, 47821),
            "원격 접속: 켜짐 · 127.0.0.1:47821"
        );
    }
}
