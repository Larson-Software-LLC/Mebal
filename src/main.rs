// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Mebal - A Replay Buffer for Continuous Video Recording
//!
//! Mebal continuously records video into a circular buffer. When triggered by a hotkey,
//! it saves the last N seconds of video to a file.
//!
//! Configuration is stored in `%APPDATA%\mebal\config.toml`.

use anyhow::Result;
use clap::Parser;
use mebal::capture::audio::run_audio_capture;
use mebal::capture::run_video_capture;
use mebal::{AppState, Config, HotkeyManager};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    info!("Starting Mebal v{}", env!("CARGO_PKG_VERSION"));

    if args.list_encoders {
        mebal::init_ffmpeg();
        println!("Available encoders:");
        for (name, available) in mebal::capture::encoder_setup::get_encoder_info() {
            let status = if available { "Y" } else { "N" };
            println!("  [{}] {}", status, name);
        }
        return Ok(());
    }

    let mut config = Config::load()?;

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

    let app = AppState::new(config.clone());
    let cancel_token = CancellationToken::new();
    let capture_start = std::time::Instant::now();

    // Video capture
    let capture_buffer = Arc::clone(&app.packet_buffer);
    let capture_cancel = cancel_token.clone();
    let capture_config = config.clone();
    let capture_handle = tokio::task::spawn_blocking(move || {
        run_video_capture(
            &capture_config,
            capture_buffer,
            capture_cancel,
            capture_start,
        )
    });

    // Audio capture
    let audio_enabled = config.audio_enabled;
    let audio_handle = if audio_enabled {
        let audio_buffer = Arc::clone(&app.packet_buffer);
        let audio_cancel = cancel_token.clone();
        let audio_config = config.clone();
        Some(tokio::task::spawn_blocking(move || {
            run_audio_capture(&audio_config, audio_buffer, audio_cancel, capture_start)
        }))
    } else {
        info!("Audio capture disabled");
        None
    };

    // Hotkey handler. The callback fires on livesplit-hotkey's keyboard-hook
    // thread, which is outside the tokio runtime — enter it so `tracker.spawn`
    // (and the save's `spawn_blocking`) have a reactor.
    let hotkey_app = app.clone();
    let tracker = app.tracker().clone();
    let rt_handle = tokio::runtime::Handle::current();
    let _hotkey_manager = HotkeyManager::new(&config.hotkey, move || {
        let app = hotkey_app.clone();
        let _enter = rt_handle.enter();
        tracker.spawn(async move {
            if let Err(e) = app.save_replay().await {
                error!("Failed to save replay: {}", e);
            }
        });
    })?;

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

    let audio_future = async {
        if let Some(handle) = audio_handle {
            let _ = handle.await;
        } else {
            std::future::pending::<()>().await;
        }
    };

    tokio::select! {
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

    cancel_token.cancel();

    // Drain in-flight saves before exit
    app.tracker().close();
    app.tracker().wait().await;

    info!("Mebal shutting down...");
    Ok(())
}
