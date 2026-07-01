// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Shared application state for Mebal

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::task::TaskTracker;
use tracing::{error, info, warn};

use crate::buffer::PacketBuffer;
use crate::config::Config;
use crate::writer::VideoWriter;

/// Cheap-to-clone handle to the shared application state.
pub type App = Arc<AppState>;

pub struct AppState {
    pub packet_buffer: Arc<PacketBuffer>,
    config: ArcSwap<Config>,
    saving: AtomicBool,
    tracker: TaskTracker,
}

impl AppState {
    pub fn new(config: Config) -> App {
        let total_bitrate = config.total_bitrate_kbps();

        Arc::new(Self {
            packet_buffer: Arc::new(PacketBuffer::new(
                config.buffer_duration_secs,
                config.fps,
                total_bitrate,
                crate::config::GOP_INTERVAL_SECS,
            )),
            config: ArcSwap::from_pointee(config),
            saving: AtomicBool::new(false),
            tracker: TaskTracker::new(),
        })
    }

    pub fn config(&self) -> Arc<Config> {
        self.config.load_full()
    }

    pub fn tracker(&self) -> &TaskTracker {
        &self.tracker
    }

    /// Returns `Ok(true)` when a capture restart is needed.
    pub fn update_config(&self, new: Config) -> Result<bool> {
        new.validate()?;
        new.save()?;

        let old = self.config.load();
        let needs_restart = old.resolution != new.resolution
            || old.fps != new.fps
            || old.encoder != new.encoder
            || old.bitrate_kbps != new.bitrate_kbps
            || old.capture_source != new.capture_source
            || old.audio_enabled != new.audio_enabled
            || old.buffer_duration_secs != new.buffer_duration_secs;

        self.config.store(Arc::new(new));

        Ok(needs_restart)
    }

    pub fn reconfigure_buffer(&self) {
        let config = self.config();
        self.packet_buffer.reconfigure(
            config.buffer_duration_secs,
            config.fps,
            config.total_bitrate_kbps(),
            crate::config::GOP_INTERVAL_SECS,
        );
    }

    pub fn is_saving(&self) -> bool {
        self.saving.load(Ordering::SeqCst)
    }

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
            let writer = VideoWriter::new(&config, extradata, audio_params);
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
