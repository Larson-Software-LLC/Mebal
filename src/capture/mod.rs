// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Video capture module using DXGI Desktop Duplication
//!
//! Captures the Windows desktop via DXGI (GPU compositor), scales BGRA -> NV12,
//! encodes with NVENC (or libx264 fallback), and pushes encoded packets into
//! the shared buffer.

use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::buffer::{Packet, PacketBuffer, PacketType};
use crate::config::Config;

pub mod audio;
mod dxgi;
pub mod encoder_setup;

/// Manages continuous video capture
pub struct CaptureManager {
    config: Config,
}

impl CaptureManager {
    /// Create a new capture manager
    pub fn new(config: &Config) -> Result<Self> {
        ffmpeg_next::init().context("Failed to initialize FFmpeg")?;

        info!("FFmpeg initialized");

        Ok(Self {
            config: config.clone(),
        })
    }

    /// Run the capture loop (blocking — call from spawn_blocking)
    pub fn run_blocking(
        &self,
        buffer: Arc<PacketBuffer>,
        cancel: CancellationToken,
        capture_start: Instant,
    ) -> Result<()> {
        let output_index = self
            .config
            .capture_source
            .as_deref()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        let mut dxgi = dxgi::DxgiCapture::new(output_index)
            .context("Failed to initialize DXGI capture")?;

        info!(
            "DXGI capture opened: {}x{} (output {})",
            dxgi.width(),
            dxgi.height(),
            output_index
        );

        // --- Create encoder ---
        let (mut encoder, encoder_name) = encoder_setup::create_encoder(&self.config)?;

        info!("Encoder: {}", encoder_name);

        // Store codec extradata on buffer (SPS/PPS needed for MP4)
        let extradata = unsafe {
            let ctx = encoder.as_ptr();
            let size = (*ctx).extradata_size as usize;
            if size > 0 && !(*ctx).extradata.is_null() {
                std::slice::from_raw_parts((*ctx).extradata, size).to_vec()
            } else {
                Vec::new()
            }
        };
        if !extradata.is_empty() {
            buffer.set_codec_extradata(extradata);
            debug!("Stored codec extradata on buffer");
        }

        // --- Create scaler: BGRA @ capture resolution -> NV12 @ config resolution ---
        let capture_w = dxgi.width();
        let capture_h = dxgi.height();
        let (enc_w, enc_h) = self.config.resolution;

        let sws_flags = if capture_w == enc_w && capture_h == enc_h {
            ffmpeg_next::software::scaling::Flags::POINT
        } else {
            ffmpeg_next::software::scaling::Flags::BILINEAR
        };

        let mut scaler = ffmpeg_next::software::scaling::Context::get(
            ffmpeg_next::format::Pixel::BGRA,
            capture_w,
            capture_h,
            ffmpeg_next::format::Pixel::NV12,
            enc_w,
            enc_h,
            sws_flags,
        )
        .context("Failed to create scaler")?;

        info!(
            "Scaler: BGRA {}x{} -> NV12 {}x{}",
            capture_w, capture_h, enc_w, enc_h
        );

        // --- Allocate frames ---
        let mut bgra_frame =
            ffmpeg_next::frame::Video::new(ffmpeg_next::format::Pixel::BGRA, capture_w, capture_h);
        let mut scaled_frame = ffmpeg_next::frame::Video::empty();

        // --- Frame pacing ---
        let fps = self.config.fps;
        let frame_interval = std::time::Duration::from_secs_f64(1.0 / fps as f64);
        let mut next_frame_time = Instant::now();
        let mut frame_count: u64 = 0;
        let mut last_pts: i64 = -1;
        let mut has_first_frame = false;

        info!("Starting capture loop ({}fps)", fps);

        loop {
            if cancel.is_cancelled() {
                info!("Capture cancelled");
                break;
            }

            // Sleep until next frame time
            let now = Instant::now();
            if next_frame_time > now {
                std::thread::sleep(next_frame_time - now);
            }
            next_frame_time += frame_interval;

            // If we fell behind, reset to prevent burst encoding
            if next_frame_time < Instant::now() {
                next_frame_time = Instant::now() + frame_interval;
            }

            // Acquire frame with 0ms timeout (non-blocking poll)
            let stride = bgra_frame.stride(0);
            match dxgi.acquire_frame_into(bgra_frame.data_mut(0), stride, 0) {
                Ok(true) => {
                    has_first_frame = true;
                }
                Ok(false) => {
                    // Desktop unchanged — re-encode previous frame if we have one
                    if !has_first_frame {
                        continue;
                    }
                }
                Err(e) => {
                    warn!("DXGI capture error: {:#}", e);
                    const MAX_RECONNECT_RETRIES: u32 = 20;
                    let mut reconnected = false;
                    for attempt in 1..=MAX_RECONNECT_RETRIES {
                        if cancel.is_cancelled() {
                            break;
                        }
                        match dxgi.reconnect(output_index) {
                            Ok(()) => {
                                info!("DXGI reconnected on attempt {}", attempt);
                                // Recreate scaler if resolution changed
                                let new_w = dxgi.width();
                                let new_h = dxgi.height();
                                if new_w != capture_w || new_h != capture_h {
                                    let flags = if new_w == enc_w && new_h == enc_h {
                                        ffmpeg_next::software::scaling::Flags::POINT
                                    } else {
                                        ffmpeg_next::software::scaling::Flags::BILINEAR
                                    };
                                    scaler = ffmpeg_next::software::scaling::Context::get(
                                        ffmpeg_next::format::Pixel::BGRA,
                                        new_w,
                                        new_h,
                                        ffmpeg_next::format::Pixel::NV12,
                                        enc_w,
                                        enc_h,
                                        flags,
                                    )
                                    .context("Failed to recreate scaler after reconnect")?;
                                    bgra_frame = ffmpeg_next::frame::Video::new(
                                        ffmpeg_next::format::Pixel::BGRA,
                                        new_w,
                                        new_h,
                                    );
                                    has_first_frame = false;
                                    info!("Scaler recreated: BGRA {}x{} -> NV12 {}x{}", new_w, new_h, enc_w, enc_h);
                                }
                                reconnected = true;
                                break;
                            }
                            Err(re) => {
                                warn!("Reconnect attempt {}/{} failed: {:#}", attempt, MAX_RECONNECT_RETRIES, re);
                                std::thread::sleep(std::time::Duration::from_millis(500));
                            }
                        }
                    }
                    if !reconnected {
                        anyhow::bail!("DXGI reconnect failed after {} attempts", MAX_RECONNECT_RETRIES);
                    }
                    continue;
                }
            }

            // Scale BGRA -> NV12
            scaler.run(&bgra_frame, &mut scaled_frame)?;

            // Set PTS from wall-clock time, enforcing strict monotonicity
            let elapsed = capture_start.elapsed();
            let mut pts = (elapsed.as_secs_f64() * fps as f64) as i64;
            if pts <= last_pts {
                pts = last_pts + 1;
            }
            last_pts = pts;
            scaled_frame.set_pts(Some(pts));
            scaled_frame.set_kind(ffmpeg_next::picture::Type::None);

            // Encode
            encoder.send_frame(&scaled_frame)?;

            let mut encoded_packet = ffmpeg_next::Packet::empty();
            while encoder.receive_packet(&mut encoded_packet).is_ok() {
                let packet = Packet::from_ffmpeg_packet(&encoded_packet, PacketType::Video);
                buffer.push(packet);
            }

            frame_count += 1;

            if frame_count % (fps as u64) == 0 {
                let stats = buffer.stats();
                debug!(
                    "Captured {} frames ({:.1} actual fps), buffer: {} packets ({:.1} MB)",
                    frame_count,
                    frame_count as f64 / capture_start.elapsed().as_secs_f64(),
                    stats.packet_count,
                    stats.total_bytes as f64 / 1024.0 / 1024.0
                );
            }
        }

        // Skip flush on cancel — avoids stale packets racing into a cleared buffer.
        if !cancel.is_cancelled() {
            encoder.send_eof()?;
            let mut encoded_packet = ffmpeg_next::Packet::empty();
            while encoder.receive_packet(&mut encoded_packet).is_ok() {
                let packet = Packet::from_ffmpeg_packet(&encoded_packet, PacketType::Video);
                buffer.push(packet);
            }
        }

        info!("Capture loop ended after {} frames", frame_count);
        Ok(())
    }
}
