// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Video capture module using FFmpeg gdigrab
//!
//! Captures the Windows desktop using FFmpeg's gdigrab input device,
//! decodes frames, scales BGR0 -> NV12, encodes with NVENC (or libx264 fallback),
//! and pushes encoded packets into the shared buffer.

use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::buffer::{Packet, PacketBuffer, PacketType};
use crate::config::Config;

pub mod audio;
pub mod encoder_setup;

/// Manages continuous video capture
pub struct CaptureManager {
    config: Config,
}

impl CaptureManager {
    /// Create a new capture manager
    pub fn new(config: &Config) -> Result<Self> {
        ffmpeg_next::init().context("Failed to initialize FFmpeg")?;
        ffmpeg_next::device::register_all();

        info!("FFmpeg initialized, devices registered");

        Ok(Self {
            config: config.clone(),
        })
    }

    /// Run the capture loop (blocking — call from spawn_blocking)
    ///
    /// This opens gdigrab, decodes, scales, encodes, and pushes packets into the buffer.
    /// `capture_start` is a shared epoch so audio and video PTS are aligned.
    pub fn run_blocking(
        &self,
        buffer: Arc<PacketBuffer>,
        cancel: CancellationToken,
        capture_start: Instant,
    ) -> Result<()> {
        // --- Open gdigrab input ---
        let mut input_ctx = self.open_gdigrab_input()?;

        // Find video stream
        let video_stream_index = input_ctx
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .context("No video stream found in gdigrab input")?
            .index();

        let stream = input_ctx.stream(video_stream_index).unwrap();
        let decoder_params = stream.parameters();

        // Create decoder
        let decoder_codec =
            ffmpeg_next::decoder::find(decoder_params.id()).context("Failed to find decoder")?;
        let mut decoder_ctx = ffmpeg_next::codec::Context::new_with_codec(decoder_codec);
        decoder_ctx.set_parameters(decoder_params)?;
        let mut decoder = decoder_ctx.decoder().video()?;

        info!(
            "Decoder opened: {}x{} {:?}",
            decoder.width(),
            decoder.height(),
            decoder.format()
        );

        // --- Create encoder ---
        let (mut encoder, encoder_name) = encoder_setup::create_encoder(&self.config, &decoder)?;

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

        // --- Create scaler: input format -> NV12 ---
        let mut scaler = ffmpeg_next::software::scaling::Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            ffmpeg_next::format::Pixel::NV12,
            encoder.width(),
            encoder.height(),
            ffmpeg_next::software::scaling::Flags::BILINEAR,
        )
        .context("Failed to create scaler")?;

        info!("Scaler: {:?} -> NV12", decoder.format());

        // --- Main capture loop ---
        let mut decoded_frame = ffmpeg_next::frame::Video::empty();
        let mut scaled_frame = ffmpeg_next::frame::Video::empty();
        let mut frame_count: u64 = 0;
        let fps = self.config.fps;

        info!("Starting capture loop");

        for (stream, input_packet) in input_ctx.packets() {
            if cancel.is_cancelled() {
                info!("Capture cancelled");
                break;
            }

            if stream.index() != video_stream_index {
                continue;
            }

            // Decode
            decoder.send_packet(&input_packet)?;

            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                // Scale BGR0 -> NV12
                scaler.run(&decoded_frame, &mut scaled_frame)?;

                // Compute PTS from wall-clock time so playback speed matches
                // real time even if gdigrab can't sustain the requested fps
                let elapsed = capture_start.elapsed();
                let pts = (elapsed.as_secs_f64() * fps as f64) as i64;
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
        }

        // Flush encoder
        encoder.send_eof()?;
        let mut encoded_packet = ffmpeg_next::Packet::empty();
        while encoder.receive_packet(&mut encoded_packet).is_ok() {
            let packet = Packet::from_ffmpeg_packet(&encoded_packet, PacketType::Video);
            buffer.push(packet);
        }

        info!("Capture loop ended after {} frames", frame_count);
        Ok(())
    }

    /// Open gdigrab input device
    fn open_gdigrab_input(&self) -> Result<ffmpeg_next::format::context::Input> {
        // Use FFI to get gdigrab input format
        let input_format = unsafe {
            let name = std::ffi::CString::new("gdigrab").unwrap();
            let fmt = ffmpeg_sys_next::av_find_input_format(name.as_ptr());
            if fmt.is_null() {
                anyhow::bail!(
                    "gdigrab input format not found — is FFmpeg built with gdigrab support?"
                );
            }
            fmt
        };

        // Build options dictionary
        let mut opts = ffmpeg_next::Dictionary::new();
        opts.set("framerate", &self.config.fps.to_string());
        opts.set(
            "video_size",
            &format!("{}x{}", self.config.resolution.0, self.config.resolution.1),
        );
        opts.set("draw_mouse", "1");

        // Input path: "desktop" for full screen, or "title=WindowName"
        let input_path = match &self.config.capture_source {
            Some(title) => format!("title={}", title),
            None => "desktop".to_string(),
        };

        info!(
            "Opening gdigrab: {} ({}x{} @ {}fps)",
            input_path, self.config.resolution.0, self.config.resolution.1, self.config.fps
        );

        // Open input via FFI
        let input_ctx = unsafe {
            let mut ctx: *mut ffmpeg_sys_next::AVFormatContext = std::ptr::null_mut();
            let path = std::ffi::CString::new(input_path.as_str()).unwrap();
            let mut opts_ptr = opts.disown();

            let ret = ffmpeg_sys_next::avformat_open_input(
                &mut ctx,
                path.as_ptr(),
                input_format,
                &mut opts_ptr,
            );

            // Free remaining options
            if !opts_ptr.is_null() {
                ffmpeg_sys_next::av_dict_free(&mut opts_ptr);
            }

            if ret < 0 {
                anyhow::bail!(
                    "Failed to open gdigrab input: {}",
                    ffmpeg_next::Error::from(ret)
                );
            }

            let ret = ffmpeg_sys_next::avformat_find_stream_info(ctx, std::ptr::null_mut());
            if ret < 0 {
                ffmpeg_sys_next::avformat_close_input(&mut ctx);
                anyhow::bail!(
                    "Failed to find stream info: {}",
                    ffmpeg_next::Error::from(ret)
                );
            }

            ffmpeg_next::format::context::Input::wrap(ctx)
        };

        Ok(input_ctx)
    }
}
