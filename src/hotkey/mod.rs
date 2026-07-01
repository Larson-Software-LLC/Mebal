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
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        if let Err(e) = self.hook.unregister(self.hotkey) {
            debug!("Failed to unregister hotkey: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hotkey_ok() {
        assert!(parse_hotkey("F9").is_ok());
        assert!(parse_hotkey("Ctrl+F9").is_ok());
        assert!(parse_hotkey("Ctrl+Shift+R").is_ok());
    }
}
