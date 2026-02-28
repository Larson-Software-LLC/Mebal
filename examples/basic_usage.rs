//! Basic usage example of Mebal as a library
//!
//! This example demonstrates how to:
//! 1. Create a packet buffer
//! 2. Start video capture
//! 3. Set up a hotkey handler
//! 4. Save replays on hotkey trigger

use anyhow::Result;
use mebal::{CaptureManager, Config, HotkeyManager, PacketBuffer, VideoWriter};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    info!("Mebal Basic Usage Example");

    // Create default configuration
    let config = Config::default();

    // Create the packet buffer (5 minutes @ 60fps, 8Mbps bitrate, 2s GOP)
    let buffer = Arc::new(PacketBuffer::new(300, 60, config.bitrate_kbps, 2));
    info!("Packet buffer created");

    // Create cancellation token for clean shutdown
    let cancel = CancellationToken::new();

    // Start capture in a blocking task
    let capture_buffer = buffer.clone();
    let capture_cancel = cancel.clone();
    let capture_config = config.clone();
    let capture_start = std::time::Instant::now();
    let capture_handle =
        tokio::task::spawn_blocking(move || match CaptureManager::new(&capture_config) {
            Ok(capture) => {
                info!("Starting capture...");
                if let Err(e) = capture.run_blocking(capture_buffer, capture_cancel, capture_start)
                {
                    error!("Capture error: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to create capture: {}", e);
            }
        });

    // Set up hotkey handler
    let hotkey_buffer = buffer.clone();
    let hotkey_config = config.clone();

    let mut hotkey = HotkeyManager::new("F9")?;
    hotkey.on_trigger(move || {
        let buffer = hotkey_buffer.clone();
        let config = hotkey_config.clone();

        tokio::spawn(async move {
            info!("Hotkey triggered! Saving replay...");

            // Get packets from buffer
            let packets = buffer.get_packets_for_duration(30);
            info!("Got {} packets", packets.len());

            // Get codec extradata
            let extradata = buffer.get_codec_extradata().unwrap_or_default();

            // Save to file
            let output_path = "replay_example.mp4";
            let writer = VideoWriter::new(&config, extradata, None);

            let path = output_path.to_string();
            match tokio::task::spawn_blocking(move || writer.write_packets_blocking(packets, &path))
                .await
            {
                Ok(Ok(_)) => info!("Replay saved to {}", output_path),
                Ok(Err(e)) => error!("Failed to save replay: {}", e),
                Err(e) => error!("Write task panicked: {}", e),
            }
        });
    });

    info!("Press F9 to save a replay, Ctrl+C to exit");

    // Run hotkey handler
    tokio::select! {
        result = hotkey.run() => {
            if let Err(e) = result {
                error!("Hotkey error: {}", e);
            }
        }
        _ = capture_handle => {
            info!("Capture ended");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Shutting down...");
        }
    }

    cancel.cancel();

    Ok(())
}
