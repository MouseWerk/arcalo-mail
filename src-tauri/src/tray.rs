//! The tray icon (desktop): the unread count in its tooltip (and as its title on macOS and
//! Linux panels that show one), and quick actions: write a message, sync all accounts, show
//! the window, quit. A click on the icon brings the window to the front. Texts follow the
//! app's language; `refresh` rebuilds the menu after a change.

use std::sync::Mutex;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

static TRAY: Mutex<Option<TrayIcon>> = Mutex::new(None);

fn menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let compose =
        MenuItem::with_id(app, "compose", mail_core::tr!("Neue Nachricht", "New message"), true, None::<&str>)?;
    let sync = MenuItem::with_id(
        app,
        "sync",
        mail_core::tr!("Alle Konten abrufen", "Check all accounts"),
        true,
        None::<&str>,
    )?;
    let show =
        MenuItem::with_id(app, "show", mail_core::tr!("Arcalo Mail zeigen", "Show Arcalo Mail"), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", mail_core::tr!("Beenden", "Quit"), true, None::<&str>)?;
    Menu::with_items(app, &[&compose, &sync, &PredefinedMenuItem::separator(app)?, &show, &quit])
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("Arcalo Mail")
        .menu(&menu(app)?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "compose" => {
                show_main(app);
                let _ = app.emit("tray-action", "compose");
            }
            "sync" => {
                if let Some(state) = app.try_state::<crate::AppState>() {
                    for w in state.all_workers() {
                        w.send(mail_core::engine::Command::SyncNow);
                    }
                }
            }
            "show" => show_main(app),
            // A downloaded update is installed first (`updates::quit`).
            "quit" => crate::updates::quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let tray = builder.build(app)?;
    *TRAY.lock().unwrap_or_else(|e| e.into_inner()) = Some(tray);
    Ok(())
}

/// The menu in the current language.
pub fn refresh(app: &AppHandle) {
    let tray = TRAY.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let (Some(t), Ok(m)) = (tray, menu(app)) {
        let _ = t.set_menu(Some(m));
    }
}

/// The unread count in the tooltip and, where panels show one, as the title.
pub fn show_unread(unread: u32) {
    let tray = TRAY.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(t) = tray else { return };
    let tip = if unread == 0 {
        "Arcalo Mail".to_string()
    } else {
        mail_core::trf!("Arcalo Mail – {unread} ungelesen", "Arcalo Mail – {unread} unread")
    };
    let _ = t.set_tooltip(Some(&tip));
    #[cfg(not(windows))]
    let _ = t.set_title(if unread == 0 { None } else { Some(unread.to_string()) });
}
