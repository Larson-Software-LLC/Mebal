// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Global hotkey handling for Mebal
//!
//! This module provides cross-platform global hotkey support using a low-level
//! keyboard hook that does not exclusively capture keypresses — other apps
//! continue to receive the key events normally.

use anyhow::{Context, Result};
use livesplit_hotkey::{Hook, Hotkey};
use tracing::{debug, info};

mod parser;

pub use parser::parse_hotkey;

const SUGGESTED_KEYS: [&str; 8] = [
    "F9",
    "F10",
    "F11",
    "F12",
    "Ctrl+F9",
    "Ctrl+Shift+R",
    "Alt+`",
    "Ctrl+Alt+S",
];

/// Hotkey manager that handles global keyboard shortcuts.
///
/// Uses `livesplit-hotkey` which installs a non-exclusive low-level keyboard
/// hook (`WH_KEYBOARD_LL` on Windows). The hook fires on its own thread.
pub struct HotkeyManager {
    hook: Hook,
    hotkey: Hotkey,
}

impl HotkeyManager {
    /// Create a new hotkey manager, parse the hotkey string, and register it
    /// with the given callback immediately.
    pub fn new<F>(hotkey_str: &str, callback: F) -> Result<Self>
    where
        F: FnMut() + Send + 'static,
    {
        let hook = Hook::new().context("Failed to create keyboard hook")?;

        let hotkey = parse_hotkey(hotkey_str)?;

        hook.register(hotkey, callback)
            .context("Failed to register hotkey")?;

        info!("Registered hotkey '{}'", hotkey_str);

        Ok(Self { hook, hotkey })
    }

    /// Unregister the hotkey
    pub fn unregister(&self) -> Result<()> {
        self.hook
            .unregister(self.hotkey)
            .context("Failed to unregister hotkey")?;
        debug!("Unregistered hotkey");
        Ok(())
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        let _ = self.unregister();
    }
}

/// Check if a hotkey string is valid
pub fn validate_hotkey(hotkey_str: &str) -> bool {
    parse_hotkey(hotkey_str).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_hotkey() {
        assert!(validate_hotkey("F9"));
        assert!(validate_hotkey("Ctrl+F9"));
        assert!(validate_hotkey("Ctrl+Shift+R"));
    }
}
