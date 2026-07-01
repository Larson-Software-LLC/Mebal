// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Configuration management for Mebal
//!
//! Handles loading and validation of application configuration.
//! Config file location: `%APPDATA%\mebal\config.toml`

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const DEFAULT_BUFFER_DURATION: u32 = 300;
const DEFAULT_SAVE_DURATION: u32 = 30;
const DEFAULT_BITRATE_KBPS: usize = 8000;
const DEFAULT_AUDIO_BITRATE_KBPS: usize = 192;
const DEFAULT_FPS: u32 = 60;
const DEFAULT_RESOLUTION: (u32, u32) = (2560, 1440);

/// GOP (Group of Pictures) interval in seconds.
///
/// Controls keyframe frequency: the encoder produces a keyframe every
/// `GOP_INTERVAL_SECS * fps` frames. Also used as overfetch padding
/// when retrieving packets for save, so `trim_to_keyframe` can find
/// a keyframe without shortening the clip.
pub const GOP_INTERVAL_SECS: u32 = 1;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Duration of the circular buffer in seconds
    pub buffer_duration_secs: u32,

    /// Duration of saved clips in seconds
    pub save_duration_secs: u32,

    /// Video bitrate in kbps
    pub bitrate_kbps: usize,

    /// Frames per second
    pub fps: u32,

    /// Output directory for saved replays
    pub output_directory: String,

    /// Output filename prefix
    pub output_prefix: String,

    /// Hotkey combination for triggering save
    pub hotkey: String,

    /// Video resolution (width, height)
    pub resolution: (u32, u32),

    /// Capture source (monitor index, or None for primary)
    pub capture_source: Option<String>,

    /// Encoder to use: "h264_nvenc", "h264_amf", or None for auto-detect
    pub encoder: Option<String>,

    /// Whether audio capture is enabled
    pub audio_enabled: bool,

    /// Audio bitrate in kbps
    pub audio_bitrate_kbps: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            buffer_duration_secs: DEFAULT_BUFFER_DURATION,
            save_duration_secs: DEFAULT_SAVE_DURATION,
            bitrate_kbps: DEFAULT_BITRATE_KBPS,
            fps: DEFAULT_FPS,
            output_directory: dirs::video_dir()
                .map(|p: PathBuf| p.join("mebal").to_string_lossy().to_string())
                .unwrap_or_else(|| "./recordings".to_string()),
            output_prefix: "replay".to_string(),
            hotkey: "F9".to_string(),
            resolution: DEFAULT_RESOLUTION,
            capture_source: None,
            encoder: None,
            audio_enabled: true,
            audio_bitrate_kbps: DEFAULT_AUDIO_BITRATE_KBPS,
        }
    }
}

impl Config {
    /// Load configuration from file or create default
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config from {:?}", config_path))?;

            let config: Config = toml::from_str(&content)
                .with_context(|| format!("Failed to parse config from {:?}", config_path))?;

            config.validate()?;
            Ok(config)
        } else {
            let config = Config::default();
            config.save()?;
            Ok(config)
        }
    }

    /// Save configuration to file
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;

        // Ensure config directory exists
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, content)
            .with_context(|| format!("Failed to write config to {:?}", config_path))?;

        Ok(())
    }

    /// Get the configuration file path
    fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Failed to determine config directory")?
            .join("mebal");

        Ok(config_dir.join("config.toml"))
    }

    /// Combined video + audio bitrate when audio is enabled.
    pub fn total_bitrate_kbps(&self) -> usize {
        if self.audio_enabled {
            self.bitrate_kbps + self.audio_bitrate_kbps
        } else {
            self.bitrate_kbps
        }
    }

    /// Validate configuration values
    pub fn validate(&self) -> Result<()> {
        let gop = GOP_INTERVAL_SECS;
        anyhow::ensure!(
            self.buffer_duration_secs > 0,
            "Buffer duration must be greater than 0"
        );
        anyhow::ensure!(
            self.save_duration_secs > 0,
            "Save duration must be greater than 0"
        );
        anyhow::ensure!(
            self.save_duration_secs + gop <= self.buffer_duration_secs,
            "Save duration + GOP compensation ({gop}s) must not exceed buffer duration"
        );
        anyhow::ensure!(self.bitrate_kbps > 0, "Bitrate must be greater than 0");
        anyhow::ensure!(self.fps > 0, "FPS must be greater than 0");
        anyhow::ensure!(
            self.resolution.0 > 0 && self.resolution.1 > 0,
            "Resolution must be valid"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.buffer_duration_secs, 300);
        assert_eq!(config.save_duration_secs, 30);
        assert_eq!(config.bitrate_kbps, 8000);
        assert_eq!(config.fps, 60);
        assert_eq!(config.resolution, (2560, 1440));
        assert!(config.encoder.is_none());
        assert!(config.audio_enabled);
        assert_eq!(config.audio_bitrate_kbps, 192);
    }

    #[test]
    fn test_validate() {
        let mut config = Config::default();
        assert!(config.validate().is_ok());

        config.save_duration_secs = 400;
        assert!(config.validate().is_err());
    }
}
