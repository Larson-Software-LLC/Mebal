// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

mod commands;
mod state;
mod tray;

use state::TauriAppState;
use tauri::Manager;
use tracing::info;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    info!("Starting Mebal GUI v{}", mebal::VERSION);

    let config = mebal::Config::load().expect("Failed to load config");
    let tauri_state = TauriAppState::new(config);

    tauri::Builder::default()
        .manage(tauri_state)
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_config,
            commands::save_replay,
            commands::get_status,
            commands::start_capture,
            commands::stop_capture,
            commands::get_encoder_info,
        ])
        .setup(|app| {
            tray::create_tray(app)?;

            // Auto-start capture
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let ts = handle.state::<TauriAppState>();
                if let Err(e) = ts.start_capture() {
                    tracing::error!("Failed to auto-start capture: {}", e);
                }
            });

            // Register global hotkey
            if let Err(e) = tray::register_hotkey(app) {
                tracing::warn!("Failed to register hotkey: {} — hotkey disabled", e);
            }

            // Start status polling
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                state::status_poll_loop(&handle).await;
            });

            // Open devtools in debug mode
            #[cfg(debug_assertions)]
            if let Some(window) = app.get_webview_window("settings") {
                let _ = window.show();
                window.open_devtools();
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "settings" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("Failed to build Tauri app")
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { ref api, .. } = event {
                // Keep alive in tray when window is hidden
                api.prevent_exit();
            }
        });
}
