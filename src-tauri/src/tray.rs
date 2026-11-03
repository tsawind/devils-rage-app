//! The app's icon by the clock: click it to bring the app forward; right-click for previews,
//! putting EVE windows back, and quitting.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_window(crate::MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Devil's Rage", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open-previews", "Open previews", true, None::<&str>)?;
    let close = MenuItem::with_id(app, "close-previews", "Close previews", true, None::<&str>)?;
    let place = MenuItem::with_id(app, "place-clients", "Put EVE windows back", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let line = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &open, &close, &place, &line, &quit])?;
    let mut builder = TrayIconBuilder::with_id("devils-rage")
        .tooltip("Devil's Rage")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "open-previews" => {
                let _ = crate::previews::open_previews(app.clone());
            }
            "close-previews" => crate::previews::close_previews(app.clone()),
            "place-clients" => {
                let _ = crate::clientwindows::client_windows_place(app.clone());
            }
            "quit" => app.exit(0),
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
    builder.build(app)?;
    Ok(())
}
