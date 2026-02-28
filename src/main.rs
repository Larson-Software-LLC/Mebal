// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Mebal - A Replay Buffer for Continuous Video Recording
//!
//! Mebal continuously records video into a circular buffer. When triggered by a hotkey,
//! it saves the last N seconds of video to a file.
//!
//! Configuration is stored in `%APPDATA%\mebal\config.toml`.

use anyhow::{Context, Result};
use clap::Parser;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

mod buffer;
mod capture;
mod config;
mod error;
mod hotkey;
mod writer;

use buffer::PacketBuffer;
use capture::CaptureManager;
use capture::audio::AudioCaptureManager;
use config::Config;
use hotkey::HotkeyManager;
use writer::VideoWriter;

/// Command line arguments
#[derive(Parser, Debug)]
#[command(name = "mebal")]
#[command(about = "A replay buffer for continuous video recording")]
#[command(version)]
struct Args {
    /// Configuration file path
    #[arg(short, long)]
    config: Option<String>,

    /// Hotkey for saving replay
    #[arg(short = 'k', long)]
    hotkey: Option<String>,

    /// Buffer duration in seconds
    #[arg(short, long)]
    buffer_duration: Option<u32>,

    /// Save duration in seconds
    #[arg(short, long)]
    save_duration: Option<u32>,

    /// Output directory
    #[arg(short, long)]
    output: Option<String>,

    /// Disable audio capture
    #[arg(long)]
    no_audio: bool,

    /// List available encoders
    #[arg(long)]
    list_encoders: bool,
}

/// Application state shared across components
pub struct AppState {
    /// The circular buffer storing encoded video packets
    packet_buffer: Arc<PacketBuffer>,
    /// Configuration settings
    config: Config,
    /// Whether a save operation is currently in progress
    saving: std::sync::atomic::AtomicBool,
}

impl AppState {
    fn new(config: Config) -> Self {
        // Include audio bitrate in buffer sizing when audio is enabled
        let total_bitrate = if config.audio_enabled {
            config.bitrate_kbps + config.audio_bitrate_kbps
        } else {
            config.bitrate_kbps
        };

        Self {
            packet_buffer: Arc::new(PacketBuffer::new(
                config.buffer_duration_secs,
                config.fps,
                total_bitrate,
                2, // GOP interval in seconds (matches encoder_setup: fps * 2)
            )),
            config,
            saving: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Save the current replay buffer to a file
    async fn save_replay(&self) -> Result<()> {
        // Check if already saving
        if self.saving.swap(true, std::sync::atomic::Ordering::SeqCst) {
            warn!("Save already in progress, ignoring trigger");
            return Ok(());
        }

        info!("Saving replay...");

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let filename = format!("{}_{}.mp4", self.config.output_prefix, timestamp);
        let output_path = std::path::Path::new(&self.config.output_directory).join(&filename);

        // Ensure output directory exists
        if let Err(e) = std::fs::create_dir_all(&self.config.output_directory) {
            self.saving
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return Err(e).with_context(|| {
                format!(
                    "Failed to create output directory: {}",
                    self.config.output_directory
                )
            });
        }

        // Get packets from buffer
        let packets = self
            .packet_buffer
            .get_packets_for_duration(self.config.save_duration_secs);

        if packets.is_empty() {
            warn!("No packets in buffer to save");
            self.saving
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return Ok(());
        }

        // Get codec extradata
        let extradata = self.packet_buffer.get_codec_extradata().unwrap_or_default();

        // Get audio params (None if audio capture never started)
        let audio_params = self.packet_buffer.get_audio_params();

        info!("Writing {} packets to {:?}", packets.len(), output_path);

        // Write video file in a blocking task
        let config = self.config.clone();
        let path = output_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            let writer = VideoWriter::new(&config, extradata, audio_params);
            writer.write_packets_blocking(packets, &path)
        })
        .await;

        match result {
            Ok(Ok(_)) => {
                info!("Replay saved to {:?}", output_path);
            }
            Ok(Err(e)) => {
                error!("Failed to write replay: {}", e);
            }
            Err(e) => {
                error!("Write task panicked: {}", e);
            }
        }

        self.saving
            .store(false, std::sync::atomic::Ordering::SeqCst);

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    info!("Starting Mebal v{}", env!("CARGO_PKG_VERSION"));

    // Handle list commands
    if args.list_encoders {
        // Initialize FFmpeg so we can probe encoders
        ffmpeg_next::init().ok();
        println!("Available encoders:");
        for (name, available) in capture::encoder_setup::get_encoder_info() {
            let status = if available { "Y" } else { "N" };
            println!("  [{}] {}", status, name);
        }
        return Ok(());
    }

    // Load configuration
    let mut config = Config::load()?;

    // Apply command line overrides
    if let Some(hotkey) = args.hotkey {
        config.hotkey = hotkey;
    }
    if let Some(duration) = args.buffer_duration {
        config.buffer_duration_secs = duration;
    }
    if let Some(duration) = args.save_duration {
        config.save_duration_secs = duration;
    }
    if let Some(output) = args.output {
        config.output_directory = output;
    }
    if args.no_audio {
        config.audio_enabled = false;
    }

    // Validate configuration
    config.validate()?;

    info!("Configuration:");
    info!("  Buffer duration: {}s", config.buffer_duration_secs);
    info!("  Save duration: {}s", config.save_duration_secs);
    info!(
        "  Resolution: {}x{}",
        config.resolution.0, config.resolution.1
    );
    info!("  FPS: {}", config.fps);
    info!("  Bitrate: {} kbps", config.bitrate_kbps);
    info!(
        "  Audio: {}",
        if config.audio_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    if config.audio_enabled {
        info!("  Audio bitrate: {} kbps", config.audio_bitrate_kbps);
    }
    info!("  Output directory: {}", config.output_directory);
    info!("  Hotkey: {}", config.hotkey);

    // Create application state
    let app_state = Arc::new(AppState::new(config.clone()));

    // Create cancellation token for clean shutdown
    let cancel_token = CancellationToken::new();

    // Shared epoch for A/V sync
    let capture_start = std::time::Instant::now();

    // Start video capture in a blocking task
    let capture_buffer = Arc::clone(&app_state.packet_buffer);
    let capture_cancel = cancel_token.clone();
    let capture_config = config.clone();
    let capture_handle =
        tokio::task::spawn_blocking(move || match CaptureManager::new(&capture_config) {
            Ok(capture) => {
                if let Err(e) = capture.run_blocking(capture_buffer, capture_cancel, capture_start)
                {
                    error!("Capture error: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to create capture manager: {}", e);
            }
        });

    // Start audio capture in a blocking task (if enabled)
    let audio_enabled = config.audio_enabled;
    let audio_handle = if audio_enabled {
        let audio_buffer = Arc::clone(&app_state.packet_buffer);
        let audio_cancel = cancel_token.clone();
        let audio_config = config.clone();
        Some(tokio::task::spawn_blocking(move || {
            let audio = AudioCaptureManager::new(&audio_config);
            if let Err(e) = audio.run_blocking(audio_buffer, audio_cancel, capture_start) {
                warn!("Audio capture failed: {} — continuing video-only", e);
            }
        }))
    } else {
        info!("Audio capture disabled");
        None
    };

    // Set up hotkey handler
    let hotkey_state = Arc::clone(&app_state);
    let mut hotkey_manager = HotkeyManager::new(&config.hotkey)?;

    hotkey_manager.on_trigger(move || {
        let state = Arc::clone(&hotkey_state);
        tokio::spawn(async move {
            if let Err(e) = state.save_replay().await {
                error!("Failed to save replay: {}", e);
            }
        });
    });

    info!("");
    info!("========================================");
    info!("  Mebal is running!");
    info!(
        "  Press {} to save the last {}s",
        config.hotkey, config.save_duration_secs
    );
    info!("  Press Ctrl+C to exit");
    info!("========================================");
    info!("");

    // Future that resolves when audio task ends, or pends forever if audio is disabled
    let audio_future = async {
        if let Some(handle) = audio_handle {
            let _ = handle.await;
        } else {
            std::future::pending::<()>().await;
        }
    };

    // Run hotkey manager (blocks until error or shutdown)
    tokio::select! {
        result = hotkey_manager.run() => {
            if let Err(e) = result {
                error!("Hotkey manager error: {}", e);
            }
        }
        _ = capture_handle => {
            warn!("Capture task ended");
        }
        _ = audio_future => {
            warn!("Audio task ended");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Shutdown signal received");
        }
    }

    // Signal capture to stop
    cancel_token.cancel();

    // Cleanup
    let _ = hotkey_manager.unregister();

    info!("Mebal shutting down...");
    Ok(())
}
