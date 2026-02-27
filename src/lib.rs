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
//! use mebal::{Config, CaptureManager, PacketBuffer};
//! use std::sync::Arc;
//! use tokio_util::sync::CancellationToken;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = Config::load()?;
//!     let buffer = Arc::new(PacketBuffer::new(300, 60));
//!     let cancel = CancellationToken::new();
//!
//!     let capture = CaptureManager::new(&config)?;
//!     // Run capture in a blocking task
//!     tokio::task::spawn_blocking(move || {
//!         capture.run_blocking(buffer, cancel).unwrap();
//!     });
//!
//!     Ok(())
//! }
//! ```

pub mod buffer;
pub mod capture;
pub mod config;
pub mod error;
pub mod hotkey;
pub mod writer;

pub use buffer::PacketBuffer;
pub use capture::CaptureManager;
pub use config::Config;
pub use error::{MebalError, MebalResult};
pub use hotkey::HotkeyManager;
pub use writer::VideoWriter;

/// Version of the Mebal library
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
