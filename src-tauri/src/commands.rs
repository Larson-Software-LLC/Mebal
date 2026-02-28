use crate::state::{StatusResponse, TauriAppState};
use mebal::Config;
use serde::Serialize;
use tauri::{Emitter, State};

#[derive(Clone, Serialize)]
pub struct EncoderInfo {
    pub name: String,
    pub available: bool,
}

#[tauri::command]
pub fn get_config(state: State<'_, TauriAppState>) -> Config {
    state.inner.config()
}

#[tauri::command]
pub fn set_config(
    state: State<'_, TauriAppState>,
    config: Config,
    window: tauri::Window,
) -> Result<bool, String> {
    let needs_restart = state
        .inner
        .update_config(config)
        .map_err(|e| e.to_string())?;

    if needs_restart {
        let config = state.inner.config();
        state.restart_with_config(config);
        let _ = window.emit("capture-state-changed", true);
    }

    Ok(needs_restart)
}

#[tauri::command]
pub async fn save_replay(
    state: State<'_, TauriAppState>,
    window: tauri::Window,
) -> Result<(), String> {
    let _ = window.emit("save-started", ());

    state.inner.save_replay().await.map_err(|e| e.to_string())?;

    let _ = window.emit("save-completed", ());
    Ok(())
}

#[tauri::command]
pub fn get_status(state: State<'_, TauriAppState>) -> StatusResponse {
    state.status()
}

#[tauri::command]
pub fn start_capture(state: State<'_, TauriAppState>, window: tauri::Window) -> Result<(), String> {
    state.start_capture().map_err(|e| e.to_string())?;
    let _ = window.emit("capture-state-changed", true);
    Ok(())
}

#[tauri::command]
pub fn stop_capture(state: State<'_, TauriAppState>, window: tauri::Window) {
    state.stop_capture();
    let _ = window.emit("capture-state-changed", false);
}

#[tauri::command]
pub fn get_encoder_info() -> Vec<EncoderInfo> {
    mebal::init_ffmpeg();
    mebal::capture::encoder_setup::get_encoder_info()
        .into_iter()
        .map(|(name, available)| EncoderInfo { name, available })
        .collect()
}
