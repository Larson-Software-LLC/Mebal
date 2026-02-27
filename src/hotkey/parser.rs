// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Hotkey string parser
//!
//! Parses hotkey strings like "Ctrl+Shift+F9" into global_hotkey::HotKey objects.

use anyhow::Result;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use tracing::debug;

/// Parse a hotkey string into a HotKey
///
/// Supported formats:
/// - Single keys: "F9", "Space", "Escape"
/// - With modifiers: "Ctrl+F9", "Alt+Tab", "Ctrl+Shift+R"
/// - Platform-specific: "Command+S" (macOS), "Super+L" (Linux)
pub fn parse_hotkey(s: &str) -> Result<HotKey> {
    let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();

    if parts.is_empty() {
        return Err(anyhow::anyhow!("Empty hotkey string"));
    }

    // Last part is the key code
    let key_str = parts.last().unwrap();
    let key_code = parse_key_code(key_str)?;

    // All other parts are modifiers
    let mut modifiers = Modifiers::empty();
    for part in &parts[..parts.len() - 1] {
        modifiers |= parse_modifier(part)?;
    }

    debug!(
        "Parsed hotkey '{}': key={:?}, modifiers={:?}",
        s, key_code, modifiers
    );

    Ok(HotKey::new(Some(modifiers), key_code))
}

/// Parse a key code string
fn parse_key_code(s: &str) -> Result<Code> {
    // Handle special key names
    let code = match s.to_lowercase().as_str() {
        // Function keys
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

        // Number keys
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

        // Letter keys
        "a" => Code::KeyA,
        "b" => Code::KeyB,
        "c" => Code::KeyC,
        "d" => Code::KeyD,
        "e" => Code::KeyE,
        "f" => Code::KeyF,
        "g" => Code::KeyG,
        "h" => Code::KeyH,
        "i" => Code::KeyI,
        "j" => Code::KeyJ,
        "k" => Code::KeyK,
        "l" => Code::KeyL,
        "m" => Code::KeyM,
        "n" => Code::KeyN,
        "o" => Code::KeyO,
        "p" => Code::KeyP,
        "q" => Code::KeyQ,
        "r" => Code::KeyR,
        "s" => Code::KeyS,
        "t" => Code::KeyT,
        "u" => Code::KeyU,
        "v" => Code::KeyV,
        "w" => Code::KeyW,
        "x" => Code::KeyX,
        "y" => Code::KeyY,
        "z" => Code::KeyZ,

        // Special keys
        "space" | " " => Code::Space,
        "enter" | "return" => Code::Enter,
        "escape" | "esc" => Code::Escape,
        "tab" => Code::Tab,
        "backspace" => Code::Backspace,
        "delete" | "del" => Code::Delete,
        "insert" | "ins" => Code::Insert,
        "home" => Code::Home,
        "end" => Code::End,
        "pageup" | "page_up" => Code::PageUp,
        "pagedown" | "page_down" => Code::PageDown,

        // Arrow keys
        "up" | "arrowup" => Code::ArrowUp,
        "down" | "arrowdown" => Code::ArrowDown,
        "left" | "arrowleft" => Code::ArrowLeft,
        "right" | "arrowright" => Code::ArrowRight,

        // Numpad
        "num0" | "numpad0" => Code::Numpad0,
        "num1" | "numpad1" => Code::Numpad1,
        "num2" | "numpad2" => Code::Numpad2,
        "num3" | "numpad3" => Code::Numpad3,
        "num4" | "numpad4" => Code::Numpad4,
        "num5" | "numpad5" => Code::Numpad5,
        "num6" | "numpad6" => Code::Numpad6,
        "num7" | "numpad7" => Code::Numpad7,
        "num8" | "numpad8" => Code::Numpad8,
        "num9" | "numpad9" => Code::Numpad9,

        // Punctuation
        "grave" | "`" | "~" => Code::Backquote,
        "minus" | "-" | "_" => Code::Minus,
        "equal" | "=" | "+" => Code::Equal,
        "bracketleft" | "[" | "{" => Code::BracketLeft,
        "bracketright" | "]" | "}" => Code::BracketRight,
        "backslash" | "\\" | "|" => Code::Backslash,
        "semicolon" | ";" | ":" => Code::Semicolon,
        "quote" | "'" | "\"" => Code::Quote,
        "comma" | "," | "<" => Code::Comma,
        "period" | "." | ">" => Code::Period,
        "slash" | "/" | "?" => Code::Slash,

        _ => return Err(anyhow::anyhow!("Unknown key code: {}", s)),
    };

    Ok(code)
}

/// Parse a modifier string
fn parse_modifier(s: &str) -> Result<Modifiers> {
    let modifier = match s.to_lowercase().as_str() {
        "ctrl" | "control" => Modifiers::CONTROL,
        "alt" | "option" => Modifiers::ALT,
        "shift" => Modifiers::SHIFT,
        "super" | "win" | "windows" | "command" | "cmd" => Modifiers::SUPER,
        _ => return Err(anyhow::anyhow!("Unknown modifier: {}", s)),
    };

    Ok(modifier)
}

/// Format a HotKey back to string representation
pub fn format_hotkey(hotkey: &HotKey) -> String {
    let mut parts = Vec::new();

    let modifiers = hotkey.mods;
    if modifiers.contains(Modifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if modifiers.contains(Modifiers::ALT) {
        parts.push("Alt");
    }
    if modifiers.contains(Modifiers::SHIFT) {
        parts.push("Shift");
    }
    if modifiers.contains(Modifiers::SUPER) {
        parts.push("Super");
    }

    let key_str = format_code(hotkey.key);
    parts.push(&key_str);

    parts.join("+")
}

/// Format a key code to string
fn format_code(code: Code) -> String {
    match code {
        Code::F1 => "F1".to_string(),
        Code::F2 => "F2".to_string(),
        Code::F3 => "F3".to_string(),
        Code::F4 => "F4".to_string(),
        Code::F5 => "F5".to_string(),
        Code::F6 => "F6".to_string(),
        Code::F7 => "F7".to_string(),
        Code::F8 => "F8".to_string(),
        Code::F9 => "F9".to_string(),
        Code::F10 => "F10".to_string(),
        Code::F11 => "F11".to_string(),
        Code::F12 => "F12".to_string(),
        Code::Space => "Space".to_string(),
        Code::Enter => "Enter".to_string(),
        Code::Escape => "Escape".to_string(),
        Code::Tab => "Tab".to_string(),
        Code::Backspace => "Backspace".to_string(),
        _ => format!("{:?}", code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_hotkey() {
        let hotkey = parse_hotkey("F9").unwrap();
        assert_eq!(hotkey.key, Code::F9);
        assert!(hotkey.mods.is_empty());
    }

    #[test]
    fn test_parse_with_modifier() {
        let hotkey = parse_hotkey("Ctrl+F9").unwrap();
        assert_eq!(hotkey.key, Code::F9);
        assert!(hotkey.mods.contains(Modifiers::CONTROL));
    }

    #[test]
    fn test_parse_multiple_modifiers() {
        let hotkey = parse_hotkey("Ctrl+Shift+R").unwrap();
        assert_eq!(hotkey.key, Code::KeyR);
        assert!(hotkey.mods.contains(Modifiers::CONTROL));
        assert!(hotkey.mods.contains(Modifiers::SHIFT));
    }

    #[test]
    fn test_parse_special_keys() {
        assert!(parse_hotkey("Space").is_ok());
        assert!(parse_hotkey("Escape").is_ok());
        assert!(parse_hotkey("Tab").is_ok());
    }

    #[test]
    fn test_invalid_key() {
        assert!(parse_hotkey("InvalidKey").is_err());
    }
}
