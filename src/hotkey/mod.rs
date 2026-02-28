// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Global hotkey handling for Mebal
//!
//! This module provides cross-platform global hotkey support,
//! allowing users to trigger replay saves from anywhere.

use anyhow::{Context, Result};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info};

mod parser;

pub use parser::parse_hotkey;

/// Hotkey manager that handles global keyboard shortcuts
pub struct HotkeyManager {
    /// The global hotkey manager
    manager: GlobalHotKeyManager,
    /// Registered hotkey
    hotkey: HotKey,
    /// Callback function when hotkey is triggered
    callback: Option<Arc<Mutex<Box<dyn FnMut() + Send + 'static>>>>,
}

impl HotkeyManager {
    /// Create a new hotkey manager with the specified hotkey string
    ///
    /// Hotkey format examples:
    /// - "F9" - Single key
    /// - "Ctrl+F9" - Modifier + key
    /// - "Ctrl+Shift+R" - Multiple modifiers
    pub fn new(hotkey_str: &str) -> Result<Self> {
        let manager = GlobalHotKeyManager::new().context("Failed to create hotkey manager")?;

        // Parse hotkey string
        let hotkey = parse_hotkey(hotkey_str)?;

        // Register the hotkey
        manager
            .register(hotkey)
            .context("Failed to register hotkey")?;

        info!("Registered hotkey '{}'", hotkey_str);

        Ok(Self {
            manager,
            hotkey,
            callback: None,
        })
    }

    /// Set the callback function to be called when hotkey is triggered
    pub fn on_trigger<F>(&mut self, callback: F)
    where
        F: FnMut() + Send + 'static,
    {
        self.callback = Some(Arc::new(Mutex::new(Box::new(callback))));
    }

    /// Run the hotkey event loop
    ///
    /// This blocks and listens for hotkey events.
    /// On Windows, a Win32 message pump is required for `global-hotkey`
    /// to receive `WM_HOTKEY` messages.
    pub async fn run(&self) -> Result<()> {
        let receiver = GlobalHotKeyEvent::receiver();

        info!("Hotkey listener started");

        loop {
            // Pump Win32 messages so WM_HOTKEY events are dispatched
            #[cfg(windows)]
            {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
                };
                unsafe {
                    let mut msg: MSG = std::mem::zeroed();
                    while PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) != 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            }

            // Check for hotkey events
            match receiver.try_recv() {
                Ok(event) => {
                    if event.id == self.hotkey.id() && event.state == HotKeyState::Pressed {
                        info!("Hotkey triggered!");

                        if let Some(ref callback) = self.callback {
                            let mut cb = callback.lock().await;
                            cb();
                        }
                    }
                }
                Err(crossbeam::channel::TryRecvError::Empty) => {
                    // No events, sleep briefly
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }
                Err(crossbeam::channel::TryRecvError::Disconnected) => {
                    error!("Hotkey event channel disconnected");
                    return Err(anyhow::anyhow!("Hotkey channel disconnected"));
                }
            }
        }
    }

    /// Unregister the hotkey
    pub fn unregister(&self) -> Result<()> {
        self.manager
            .unregister(self.hotkey)
            .context("Failed to unregister hotkey")?;
        debug!("Unregistered hotkey");
        Ok(())
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        // Unregister hotkey on drop
        let _ = self.unregister();
    }
}

/// Check if a hotkey string is valid
pub fn validate_hotkey(hotkey_str: &str) -> bool {
    parse_hotkey(hotkey_str).is_ok()
}

/// List of common hotkey suggestions
pub fn suggested_hotkeys() -> Vec<&'static str> {
    vec![
        "F9",
        "F10",
        "F11",
        "F12",
        "Ctrl+F9",
        "Ctrl+Shift+R",
        "Alt+`",
        "Ctrl+Alt+S",
    ]
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

    #[test]
    fn test_suggested_hotkeys() {
        let suggestions = suggested_hotkeys();
        assert!(!suggestions.is_empty());
        assert!(suggestions.contains(&"F9"));
    }
}
