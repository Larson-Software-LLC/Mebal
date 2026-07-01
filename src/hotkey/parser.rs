// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Hotkey string parser
//!
//! Parses hotkey strings like "Ctrl+Shift+F9" into livesplit_hotkey::Hotkey objects.

use anyhow::Result;
use livesplit_hotkey::{Hotkey, KeyCode, Modifiers};
use tracing::debug;

/// Parse a hotkey string into a Hotkey
///
/// Supported formats:
/// - Single keys: "F9", "Space", "Escape"
/// - With modifiers: "Ctrl+F9", "Alt+Tab", "Ctrl+Shift+R"
/// - Platform-specific: "Command+S" (macOS), "Super+L" (Linux)
pub fn parse_hotkey(s: &str) -> Result<Hotkey> {
    let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();

    if parts.is_empty() {
        return Err(anyhow::anyhow!("Empty hotkey string"));
    }

    // Last part is the key code
    let key_str = parts.last().expect("split yields >= 1 element");
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

    Ok(Hotkey {
        key_code,
        modifiers,
    })
}

/// Parse a key code string
fn parse_key_code(s: &str) -> Result<KeyCode> {
    // Handle special key names
    let code = match s.to_lowercase().as_str() {
        // Function keys
        "f1" => KeyCode::F1,
        "f2" => KeyCode::F2,
        "f3" => KeyCode::F3,
        "f4" => KeyCode::F4,
        "f5" => KeyCode::F5,
        "f6" => KeyCode::F6,
        "f7" => KeyCode::F7,
        "f8" => KeyCode::F8,
        "f9" => KeyCode::F9,
        "f10" => KeyCode::F10,
        "f11" => KeyCode::F11,
        "f12" => KeyCode::F12,
        "f13" => KeyCode::F13,
        "f14" => KeyCode::F14,
        "f15" => KeyCode::F15,
        "f16" => KeyCode::F16,
        "f17" => KeyCode::F17,
        "f18" => KeyCode::F18,
        "f19" => KeyCode::F19,
        "f20" => KeyCode::F20,
        "f21" => KeyCode::F21,
        "f22" => KeyCode::F22,
        "f23" => KeyCode::F23,
        "f24" => KeyCode::F24,

        // Number keys
        "0" | "digit0" => KeyCode::Digit0,
        "1" | "digit1" => KeyCode::Digit1,
        "2" | "digit2" => KeyCode::Digit2,
        "3" | "digit3" => KeyCode::Digit3,
        "4" | "digit4" => KeyCode::Digit4,
        "5" | "digit5" => KeyCode::Digit5,
        "6" | "digit6" => KeyCode::Digit6,
        "7" | "digit7" => KeyCode::Digit7,
        "8" | "digit8" => KeyCode::Digit8,
        "9" | "digit9" => KeyCode::Digit9,

        // Letter keys
        "a" => KeyCode::KeyA,
        "b" => KeyCode::KeyB,
        "c" => KeyCode::KeyC,
        "d" => KeyCode::KeyD,
        "e" => KeyCode::KeyE,
        "f" => KeyCode::KeyF,
        "g" => KeyCode::KeyG,
        "h" => KeyCode::KeyH,
        "i" => KeyCode::KeyI,
        "j" => KeyCode::KeyJ,
        "k" => KeyCode::KeyK,
        "l" => KeyCode::KeyL,
        "m" => KeyCode::KeyM,
        "n" => KeyCode::KeyN,
        "o" => KeyCode::KeyO,
        "p" => KeyCode::KeyP,
        "q" => KeyCode::KeyQ,
        "r" => KeyCode::KeyR,
        "s" => KeyCode::KeyS,
        "t" => KeyCode::KeyT,
        "u" => KeyCode::KeyU,
        "v" => KeyCode::KeyV,
        "w" => KeyCode::KeyW,
        "x" => KeyCode::KeyX,
        "y" => KeyCode::KeyY,
        "z" => KeyCode::KeyZ,

        // Special keys
        "space" | " " => KeyCode::Space,
        "enter" | "return" => KeyCode::Enter,
        "escape" | "esc" => KeyCode::Escape,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "page_up" => KeyCode::PageUp,
        "pagedown" | "page_down" => KeyCode::PageDown,

        // Arrow keys
        "up" | "arrowup" => KeyCode::ArrowUp,
        "down" | "arrowdown" => KeyCode::ArrowDown,
        "left" | "arrowleft" => KeyCode::ArrowLeft,
        "right" | "arrowright" => KeyCode::ArrowRight,

        // Numpad
        "num0" | "numpad0" => KeyCode::Numpad0,
        "num1" | "numpad1" => KeyCode::Numpad1,
        "num2" | "numpad2" => KeyCode::Numpad2,
        "num3" | "numpad3" => KeyCode::Numpad3,
        "num4" | "numpad4" => KeyCode::Numpad4,
        "num5" | "numpad5" => KeyCode::Numpad5,
        "num6" | "numpad6" => KeyCode::Numpad6,
        "num7" | "numpad7" => KeyCode::Numpad7,
        "num8" | "numpad8" => KeyCode::Numpad8,
        "num9" | "numpad9" => KeyCode::Numpad9,

        // Punctuation
        "grave" | "`" | "~" => KeyCode::Backquote,
        "minus" | "-" | "_" => KeyCode::Minus,
        "equal" | "=" | "+" => KeyCode::Equal,
        "bracketleft" | "[" | "{" => KeyCode::BracketLeft,
        "bracketright" | "]" | "}" => KeyCode::BracketRight,
        "backslash" | "\\" | "|" => KeyCode::Backslash,
        "semicolon" | ";" | ":" => KeyCode::Semicolon,
        "quote" | "'" | "\"" => KeyCode::Quote,
        "comma" | "," | "<" => KeyCode::Comma,
        "period" | "." | ">" => KeyCode::Period,
        "slash" | "/" | "?" => KeyCode::Slash,

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
        "super" | "win" | "windows" | "command" | "cmd" => Modifiers::META,
        _ => return Err(anyhow::anyhow!("Unknown modifier: {}", s)),
    };

    Ok(modifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_hotkey() {
        let hotkey = parse_hotkey("F9").unwrap();
        assert_eq!(hotkey.key_code, KeyCode::F9);
        assert!(hotkey.modifiers.is_empty());
    }

    #[test]
    fn test_parse_with_modifier() {
        let hotkey = parse_hotkey("Ctrl+F9").unwrap();
        assert_eq!(hotkey.key_code, KeyCode::F9);
        assert!(hotkey.modifiers.contains(Modifiers::CONTROL));
    }

    #[test]
    fn test_parse_multiple_modifiers() {
        let hotkey = parse_hotkey("Ctrl+Shift+R").unwrap();
        assert_eq!(hotkey.key_code, KeyCode::KeyR);
        assert!(hotkey.modifiers.contains(Modifiers::CONTROL));
        assert!(hotkey.modifiers.contains(Modifiers::SHIFT));
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
