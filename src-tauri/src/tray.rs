// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

use crate::state::TauriAppState;
use mebal::HotkeyManager;
use tauri::{
    AppHandle, Manager,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tracing::{error, info};

pub fn create_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let save_i = MenuItem::with_id(app, "save", "Save Replay", true, None::<&str>)?;
    let settings_i = MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit Mebal", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&save_i, &settings_i, &quit_i])?;

    TrayIconBuilder::new()
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "save" => {
                let handle = app.clone();
                let ts = handle.state::<TauriAppState>();
                let inner = ts.inner.clone();
                // ponytail: async_runtime::spawn works off-runtime (this runs on
                // Tauri's main thread); track_future keeps TaskTracker draining.
                tauri::async_runtime::spawn(ts.task_tracker.track_future(async move {
                    if let Err(e) = inner.save_replay().await {
                        error!("Failed to save replay: {}", e);
                    }
                }));
            }
            "settings" => {
                show_settings_window(app);
            }
            "quit" => {
                let ts = app.state::<TauriAppState>();
                ts.stop_capture();
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click { .. } = event {
                show_settings_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn show_settings_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn register_hotkey(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let ts = app.state::<TauriAppState>();
    let config = ts.inner.config();
    let hotkey_str = config.hotkey.clone();

    let handle = app.handle().clone();
    let manager = HotkeyManager::new(&hotkey_str, move || {
        let handle = handle.clone();
        let ts = handle.state::<TauriAppState>();
        let inner = ts.inner.clone();
        // ponytail: hotkey callback runs on livesplit's hook thread with no Tokio
        // runtime; async_runtime::spawn works from any thread (the bug fix).
        tauri::async_runtime::spawn(ts.task_tracker.track_future(async move {
            if let Err(e) = inner.save_replay().await {
                error!("Failed to save replay via hotkey: {}", e);
            }
        }));
    })?;

    ts.set_hotkey_manager(manager);

    info!("Registered global hotkey: {}", hotkey_str);
    Ok(())
}
