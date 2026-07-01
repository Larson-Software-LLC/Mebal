// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Video capture module using DXGI Desktop Duplication
//!
//! Captures the Windows desktop via DXGI (GPU compositor) as BGRA textures,
//! copies them GPU->GPU into a D3D11 hardware frame pool, and encodes them with
//! NVENC/AMF (which does RGB->NV12 internally) — no CPU readback. Encoded packets
//! are pushed into the shared buffer.

use anyhow::{Context, Result};
use ffmpeg_sys_next as ff;
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::core::Interface;

use crate::buffer::{Packet, PacketBuffer, PacketType};
use crate::config::Config;

pub mod audio;
mod dxgi;
pub mod encoder_setup;
mod hwframe;
mod shader;

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

        let (enc_w, enc_h) = self.config.resolution;

        let mut dxgi =
            dxgi::DxgiCapture::new(output_index).context("Failed to initialize DXGI capture")?;

        info!(
            "DXGI capture opened: {}x{} (output {})",
            dxgi.width(),
            dxgi.height(),
            output_index
        );

        // --- FFmpeg D3D11 hardware frame pool (shares the capture device) ---
        let hw = hwframe::HwFrames::new(dxgi.device(), enc_w, enc_h)
            .context("Failed to create D3D11 hardware frame pool")?;

        // --- Create encoder (consumes D3D11 frames from the pool) ---
        let (mut encoder, encoder_name) = encoder_setup::create_encoder(&self.config, &hw)?;

        info!("Encoder: {}", encoder_name);

        // Pick a GPU conversion pass:
        // - HDR display: always tone-map FP16 scRGB -> SDR sRGB BGRA.
        // - SDR + scaling needed: BGRA passthrough scale.
        // - SDR + native size: none (the captured texture copies straight in).
        let mut converter = if dxgi.is_hdr() {
            Some(
                shader::Converter::hdr(
                    dxgi.device(),
                    enc_w,
                    enc_h,
                    dxgi::capture_format(true),
                    dxgi.sdr_white_scale(),
                )
                .context("Failed to create HDR converter")?,
            )
        } else if dxgi.width() != enc_w || dxgi.height() != enc_h {
            Some(
                shader::Converter::passthrough(dxgi.device(), enc_w, enc_h)
                    .context("Failed to create scaler")?,
            )
        } else {
            None
        };

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

        let path = if dxgi.is_hdr() {
            "HDR tone-map"
        } else if converter.is_some() {
            "scaled"
        } else {
            "direct copy"
        };
        info!(
            "Zero-copy D3D11 encode: capture {}x{} -> encode {}x{} ({})",
            dxgi.width(),
            dxgi.height(),
            enc_w,
            enc_h,
            path
        );

        // --- Frame pacing ---
        let fps = self.config.fps;
        let frame_interval = std::time::Duration::from_secs_f64(1.0 / fps as f64);
        let mut next_frame_time = Instant::now();
        let mut frame_count: u64 = 0;
        let mut last_pts: i64 = -1;
        // The most recent pool frame; reused when the desktop is unchanged.
        let mut last: Option<hwframe::HwFrame> = None;

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

            // Acquire latest desktop frame (0ms = non-blocking poll)
            let acquired = match dxgi.acquire() {
                Ok(opt) => opt,
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
                                last = None;
                                reconnected = true;
                                break;
                            }
                            Err(re) => {
                                warn!(
                                    "Reconnect attempt {}/{} failed: {:#}",
                                    attempt, MAX_RECONNECT_RETRIES, re
                                );
                                std::thread::sleep(std::time::Duration::from_millis(500));
                            }
                        }
                    }
                    if !reconnected {
                        anyhow::bail!(
                            "DXGI reconnect failed after {} attempts",
                            MAX_RECONNECT_RETRIES
                        );
                    }
                    continue;
                }
            };

            match acquired {
                Some(acq) => {
                    // Copy (and scale) the captured BGRA texture into a fresh pool
                    // frame — entirely on the GPU.
                    let frame = hw.get_frame()?;
                    let (pool_raw, idx) = frame.texture();
                    let pool = unsafe { ID3D11Texture2D::from_raw_borrowed(&pool_raw) }
                        .context("pool frame had null texture")?;
                    let ctx = dxgi.context();
                    unsafe {
                        let src: &ID3D11Texture2D = match converter.as_mut() {
                            Some(c) => c.convert(ctx, &acq.texture)?,
                            None => &acq.texture,
                        };
                        ctx.CopySubresourceRegion(pool, idx, 0, 0, 0, src, 0, None);
                    }
                    drop(acq); // ReleaseFrame back to DXGI
                    last = Some(frame);
                }
                None => {
                    // Desktop unchanged — re-encode the previous frame if we have one.
                    if last.is_none() {
                        continue;
                    }
                }
            }

            // Set PTS from wall-clock time, enforcing strict monotonicity.
            let elapsed = capture_start.elapsed();
            let mut pts = (elapsed.as_secs_f64() * fps as f64) as i64;
            if pts <= last_pts {
                pts = last_pts + 1;
            }
            last_pts = pts;

            let frame = last.as_mut().expect("frame present");
            frame.set_pts(pts);

            // Encode (submit the GPU frame; the encoder does RGB->NV12 internally)
            unsafe {
                let ret = ff::avcodec_send_frame(encoder.as_mut_ptr(), frame.as_ptr());
                if ret < 0 {
                    warn!("avcodec_send_frame failed ({})", ret);
                }
            }

            let mut encoded_packet = ffmpeg_next::Packet::empty();
            while encoder.receive_packet(&mut encoded_packet).is_ok() {
                let packet = Packet::from_ffmpeg_packet(&encoded_packet, PacketType::Video);
                buffer.push(packet);
            }

            frame_count += 1;

            if frame_count.is_multiple_of(fps as u64) {
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
            unsafe {
                ff::avcodec_send_frame(encoder.as_mut_ptr(), std::ptr::null());
            }
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

/// Create a `CaptureManager` and run it to completion, logging errors.
///
/// Shared by the CLI (`tokio::spawn_blocking`) and the Tauri GUI
/// (`std::thread::spawn`) — each picks its own spawn mechanism, but the
/// "create manager, run, log failure" sequence is identical either way.
pub fn run_video_capture(
    config: &Config,
    buffer: Arc<PacketBuffer>,
    cancel: CancellationToken,
    capture_start: Instant,
) {
    match CaptureManager::new(config) {
        Ok(capture) => {
            if let Err(e) = capture.run_blocking(buffer, cancel, capture_start) {
                error!("Capture error: {:#}", e);
            }
        }
        Err(e) => {
            error!("Failed to create capture manager: {}", e);
        }
    }
}
