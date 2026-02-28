use crate::state::TauriAppState;
use tauri::{
    AppHandle, Manager,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};
use tracing::{error, info};

/// Create the system tray icon and menu.
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
                tauri::async_runtime::spawn(async move {
                    let ts = handle.state::<TauriAppState>();
                    if let Err(e) = ts.inner.save_replay().await {
                        error!("Failed to save replay: {}", e);
                    }
                });
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

/// Register the global hotkey from config.
pub fn register_hotkey(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let ts = app.state::<TauriAppState>();
    let config = ts.inner.config();
    let hotkey_str = &config.hotkey;

    let shortcut = parse_mebal_hotkey(hotkey_str)?;

    let handle = app.handle().clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_app, _shortcut, event| {
            if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    let ts = handle.state::<TauriAppState>();
                    if let Err(e) = ts.inner.save_replay().await {
                        error!("Failed to save replay via hotkey: {}", e);
                    }
                });
            }
        })?;

    info!("Registered global hotkey: {}", hotkey_str);
    Ok(())
}

/// Convert a mebal-format hotkey string ("Ctrl+Shift+F9") to a Tauri Shortcut.
fn parse_mebal_hotkey(s: &str) -> Result<Shortcut, Box<dyn std::error::Error>> {
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    if parts.is_empty() {
        return Err("Empty hotkey string".into());
    }

    let key_str = parts.last().unwrap();
    let mod_parts = &parts[..parts.len() - 1];

    let mut modifiers = Modifiers::empty();
    for m in mod_parts {
        match m.to_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= Modifiers::CONTROL,
            "alt" | "option" => modifiers |= Modifiers::ALT,
            "shift" => modifiers |= Modifiers::SHIFT,
            "super" | "win" | "windows" | "command" | "cmd" => modifiers |= Modifiers::SUPER,
            other => return Err(format!("Unknown modifier: {}", other).into()),
        }
    }

    let code = parse_key_code(key_str)?;

    if modifiers.is_empty() {
        Ok(Shortcut::new(None, code))
    } else {
        Ok(Shortcut::new(Some(modifiers), code))
    }
}

fn parse_key_code(s: &str) -> Result<Code, Box<dyn std::error::Error>> {
    let code = match s.to_lowercase().as_str() {
        "f1" => Code::F1,
        "f2" => Code::F2,
        "f3" => Code::F3,
        "f4" => Code::F4,
        "f5" => Code::F5,
        "f6" => Code::F6,
        "f7" => Code::F7,
        "f8" => Code::F8,
        "f9" => Code::F9,
        "f10" => Code::F10,
        "f11" => Code::F11,
        "f12" => Code::F12,
        "f13" => Code::F13,
        "f14" => Code::F14,
        "f15" => Code::F15,
        "f16" => Code::F16,
        "f17" => Code::F17,
        "f18" => Code::F18,
        "f19" => Code::F19,
        "f20" => Code::F20,
        "f21" => Code::F21,
        "f22" => Code::F22,
        "f23" => Code::F23,
        "f24" => Code::F24,
        "a" | "keya" => Code::KeyA,
        "b" | "keyb" => Code::KeyB,
        "c" | "keyc" => Code::KeyC,
        "d" | "keyd" => Code::KeyD,
        "e" | "keye" => Code::KeyE,
        "f" | "keyf" => Code::KeyF,
        "g" | "keyg" => Code::KeyG,
        "h" | "keyh" => Code::KeyH,
        "i" | "keyi" => Code::KeyI,
        "j" | "keyj" => Code::KeyJ,
        "k" | "keyk" => Code::KeyK,
        "l" | "keyl" => Code::KeyL,
        "m" | "keym" => Code::KeyM,
        "n" | "keyn" => Code::KeyN,
        "o" | "keyo" => Code::KeyO,
        "p" | "keyp" => Code::KeyP,
        "q" | "keyq" => Code::KeyQ,
        "r" | "keyr" => Code::KeyR,
        "s" | "keys" => Code::KeyS,
        "t" | "keyt" => Code::KeyT,
        "u" | "keyu" => Code::KeyU,
        "v" | "keyv" => Code::KeyV,
        "w" | "keyw" => Code::KeyW,
        "x" | "keyx" => Code::KeyX,
        "y" | "keyy" => Code::KeyY,
        "z" | "keyz" => Code::KeyZ,
        "0" | "digit0" => Code::Digit0,
        "1" | "digit1" => Code::Digit1,
        "2" | "digit2" => Code::Digit2,
        "3" | "digit3" => Code::Digit3,
        "4" | "digit4" => Code::Digit4,
        "5" | "digit5" => Code::Digit5,
        "6" | "digit6" => Code::Digit6,
        "7" | "digit7" => Code::Digit7,
        "8" | "digit8" => Code::Digit8,
        "9" | "digit9" => Code::Digit9,
        "space" => Code::Space,
        "enter" => Code::Enter,
        "escape" | "esc" => Code::Escape,
        "tab" => Code::Tab,
        "backspace" => Code::Backspace,
        "delete" => Code::Delete,
        "insert" => Code::Insert,
        "home" => Code::Home,
        "end" => Code::End,
        "pageup" => Code::PageUp,
        "pagedown" => Code::PageDown,
        "up" | "arrowup" => Code::ArrowUp,
        "down" | "arrowdown" => Code::ArrowDown,
        "left" | "arrowleft" => Code::ArrowLeft,
        "right" | "arrowright" => Code::ArrowRight,
        other => return Err(format!("Unknown key: {}", other).into()),
    };
    Ok(code)
}
