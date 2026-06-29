// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Mebal - A Replay Buffer for Continuous Video Recording
//!
//! Mebal continuously records video into a circular buffer. When triggered by a hotkey,
//! it saves the last N seconds of video to a file. Uses FFmpeg gdigrab for capture
//! and NVENC/libx264 for encoding on Windows.
//!
//! # Example Usage
//!
//! ```no_run
//! use mebal::{AppState, Config, CaptureManager, PacketBuffer, GOP_INTERVAL_SECS};
//! use std::sync::Arc;
//! use std::time::Instant;
//! use tokio_util::sync::CancellationToken;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = Config::load()?;
//!     let app = AppState::new(config.clone());
//!     let cancel = CancellationToken::new();
//!     let capture_start = Instant::now();
//!
//!     let buffer = Arc::clone(&app.packet_buffer);
//!     let capture = CaptureManager::new(&config)?;
//!     tokio::task::spawn_blocking(move || {
//!         capture.run_blocking(buffer, cancel, capture_start).unwrap();
//!     });
//!
//!     Ok(())
//! }
//! ```

pub mod app;
pub mod buffer;
pub mod capture;
pub mod config;
pub mod hotkey;
pub mod writer;

pub use app::{App, AppState};
pub use buffer::PacketBuffer;
pub use capture::CaptureManager;
pub use capture::audio::AudioCaptureManager;
pub use config::Config;
pub use hotkey::HotkeyManager;
pub use writer::VideoWriter;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub use config::GOP_INTERVAL_SECS;

/// Initialize FFmpeg. Call before using encoder probing functions.
pub fn init_ffmpeg() {
    ffmpeg_next::init().ok();
}
