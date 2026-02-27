// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Error types for Mebal

use thiserror::Error;

/// Main error type for Mebal operations
#[derive(Error, Debug)]
pub enum MebalError {
    #[error("FFmpeg error: {0}")]
    Ffmpeg(#[from] ffmpeg_next::Error),

    #[error("Encoder error: {0}")]
    Encoder(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Buffer error: {0}")]
    Buffer(String),

    #[error("Capture error: {0}")]
    Capture(String),

    #[error("Hotkey error: {0}")]
    Hotkey(String),

    #[error("Video write error: {0}")]
    VideoWrite(String),
}

#[allow(unused)]
/// Result type alias for Mebal operations
pub type MebalResult<T> = Result<T, MebalError>;
