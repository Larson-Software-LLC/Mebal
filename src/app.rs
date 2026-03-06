// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Shared application state for Mebal
//!
//! `AppState` holds the circular buffer, configuration, and save guard.
//! Both the CLI binary and the Tauri GUI depend on this module.

use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{error, info, warn};

use crate::buffer::PacketBuffer;
use crate::config::Config;
use crate::writer::VideoWriter;

/// Application state shared across components
pub struct AppState {
    /// The circular buffer storing encoded media packets
    pub packet_buffer: Arc<PacketBuffer>,
    /// Current configuration (behind RwLock for runtime updates)
    config: parking_lot::RwLock<Config>,
    /// Guard preventing concurrent save operations
    saving: AtomicBool,
}

impl AppState {
    /// Create a new `AppState` from the given configuration.
    pub fn new(config: Config) -> Self {
        let total_bitrate = config.total_bitrate_kbps();

        Self {
            packet_buffer: Arc::new(PacketBuffer::new(
                config.buffer_duration_secs,
                config.fps,
                total_bitrate,
                crate::config::GOP_INTERVAL_SECS,
            )),
            config: parking_lot::RwLock::new(config),
            saving: AtomicBool::new(false),
        }
    }

    /// Return a clone of the current configuration.
    pub fn config(&self) -> Config {
        self.config.read().clone()
    }

    /// Update the configuration, validate it, and persist to disk.
    ///
    /// Returns `Ok(true)` when capture-affecting settings changed
    /// and a capture restart is needed.
    pub fn update_config(&self, new: Config) -> Result<bool> {
        new.validate()?;
        new.save()?;

        let old = self.config.read().clone();
        let needs_restart = old.resolution != new.resolution
            || old.fps != new.fps
            || old.encoder != new.encoder
            || old.bitrate_kbps != new.bitrate_kbps
            || old.capture_source != new.capture_source
            || old.audio_enabled != new.audio_enabled
            || old.buffer_duration_secs != new.buffer_duration_secs;

        *self.config.write() = new;

        Ok(needs_restart)
    }

    /// Update the packet buffer parameters to match the current config.
    pub fn reconfigure_buffer(&self) {
        let config = self.config();
        self.packet_buffer.reconfigure(
            config.buffer_duration_secs,
            config.fps,
            config.total_bitrate_kbps(),
            crate::config::GOP_INTERVAL_SECS,
        );
    }

    /// Whether a save operation is currently in progress.
    pub fn is_saving(&self) -> bool {
        self.saving.load(Ordering::SeqCst)
    }

    /// Save the current replay buffer to a file.
    pub async fn save_replay(&self) -> Result<()> {
        if self.saving.swap(true, Ordering::SeqCst) {
            warn!("Save already in progress, ignoring trigger");
            return Ok(());
        }

        info!("Saving replay...");

        let config = self.config();
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let filename = format!("{}_{}.mp4", config.output_prefix, timestamp);
        let output_path = std::path::Path::new(&config.output_directory).join(&filename);

        if let Err(e) = std::fs::create_dir_all(&config.output_directory) {
            self.saving.store(false, Ordering::SeqCst);
            return Err(e).with_context(|| {
                format!(
                    "Failed to create output directory: {}",
                    config.output_directory
                )
            });
        }

        let packets = self
            .packet_buffer
            .get_packets_for_duration(config.save_duration_secs);

        if packets.is_empty() {
            warn!("No packets in buffer to save");
            self.saving.store(false, Ordering::SeqCst);
            return Ok(());
        }

        let extradata = self.packet_buffer.get_codec_extradata().unwrap_or_default();
        let audio_params = self.packet_buffer.get_audio_params();

        info!("Writing {} packets to {:?}", packets.len(), output_path);

        let path = output_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            let writer = VideoWriter::new(
                &config,
                extradata,
                audio_params,
                ffmpeg_sys_next::AVCodecID::AV_CODEC_ID_H264,
            );
            writer.write_packets_blocking(packets, &path)
        })
        .await;

        match result {
            Ok(Ok(_)) => info!("Replay saved to {:?}", output_path),
            Ok(Err(e)) => error!("Failed to write replay: {}", e),
            Err(e) => error!("Write task panicked: {}", e),
        }

        self.saving.store(false, Ordering::SeqCst);

        Ok(())
    }
}
