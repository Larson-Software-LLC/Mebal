// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! System audio capture via WASAPI loopback (cpal) + AAC encoding (FFmpeg)
//!
//! Captures the default audio output device using cpal's WASAPI loopback mode,
//! encodes to AAC via FFmpeg, and pushes packets into the shared buffer.

use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::buffer::{AudioParams, Packet, PacketBuffer, PacketType};
use crate::config::Config;

/// Manages system audio capture via WASAPI loopback
pub struct AudioCaptureManager {
    config: Config,
}

impl AudioCaptureManager {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
        }
    }

    /// Run the audio capture loop (blocking — call from spawn_blocking).
    ///
    /// `capture_start` is the shared epoch created in main so audio and video
    /// PTS values are aligned.
    pub fn run_blocking(
        &self,
        buffer: Arc<PacketBuffer>,
        cancel: CancellationToken,
        capture_start: Instant,
    ) -> Result<()> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        // --- Open default output device for loopback ---
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .context("No default audio output device found")?;

        info!("Audio device: {}", device.name().unwrap_or_default());

        // Get the device's default output config (we'll capture in this format)
        let supported_config = device
            .default_output_config()
            .context("Failed to get default audio output config")?;

        let sample_rate = supported_config.sample_rate().0;
        let channels = supported_config.channels();

        info!(
            "Audio config: {}Hz, {} channels, {:?}",
            sample_rate,
            channels,
            supported_config.sample_format()
        );

        // --- Create FFmpeg AAC encoder ---
        let aac_codec = ffmpeg_next::encoder::find_by_name("aac")
            .context("AAC encoder not found in FFmpeg build")?;

        let mut encoder_ctx = ffmpeg_next::codec::Context::new_with_codec(aac_codec);

        // Configure AAC encoder via unsafe FFI (ffmpeg-next doesn't expose audio encoder API fully)
        unsafe {
            let ctx = encoder_ctx.as_mut_ptr();
            (*ctx).sample_fmt = ffmpeg_sys_next::AVSampleFormat::AV_SAMPLE_FMT_FLTP;
            (*ctx).sample_rate = sample_rate as i32;
            (*ctx).bit_rate = (self.config.audio_bitrate_kbps * 1000) as i64;
            (*ctx).time_base = ffmpeg_sys_next::AVRational {
                num: 1,
                den: sample_rate as i32,
            };

            // Set channel layout
            (*ctx).ch_layout = std::mem::zeroed();
            ffmpeg_sys_next::av_channel_layout_default(&mut (*ctx).ch_layout, channels as i32);

            // Open encoder
            let ret = ffmpeg_sys_next::avcodec_open2(ctx, aac_codec.as_ptr(), std::ptr::null_mut());
            if ret < 0 {
                anyhow::bail!(
                    "Failed to open AAC encoder: {}",
                    ffmpeg_next::Error::from(ret)
                );
            }
        }

        // Extract extradata from encoder
        let extradata = unsafe {
            let ctx = encoder_ctx.as_ptr();
            let size = (*ctx).extradata_size as usize;
            if size > 0 && !(*ctx).extradata.is_null() {
                std::slice::from_raw_parts((*ctx).extradata, size).to_vec()
            } else {
                Vec::new()
            }
        };

        // Get the encoder's frame_size (AAC is typically 1024)
        let frame_size = unsafe { (*encoder_ctx.as_ptr()).frame_size as usize };
        info!(
            "AAC encoder opened: frame_size={}, extradata={} bytes",
            frame_size,
            extradata.len()
        );

        // Store audio params on the buffer
        buffer.set_audio_params(AudioParams {
            sample_rate,
            channels,
            frame_size: frame_size as u32,
            extradata,
        });

        // --- Set up crossbeam channel for cpal callback -> processing thread ---
        let (tx, rx) = crossbeam::channel::bounded::<Vec<f32>>(64);

        // --- Start cpal input stream on the output device (loopback) ---
        let stream_config = cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let tx_clone = tx.clone();
        let stream = device
            .build_input_stream(
                &stream_config,
                move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                    // Send PCM data to processing thread; drop if channel is full
                    let _ = tx_clone.try_send(data.to_vec());
                },
                move |err| {
                    error!("cpal stream error: {}", err);
                },
                None, // no timeout
            )
            .context("Failed to build cpal input stream (loopback)")?;

        stream.play().context("Failed to start cpal stream")?;
        info!("Audio capture started (WASAPI loopback)");

        // Wall-clock offset in buffer timebase units at the moment audio starts,
        // so encoder PTS (starting from 0) can be shifted to align with video PTS.
        let audio_base_pts =
            (capture_start.elapsed().as_secs_f64() * self.config.fps as f64) as i64;

        // --- Processing loop: receive PCM, accumulate frames, encode ---
        let fps = self.config.fps;
        let ch = channels as usize;
        let mut pcm_buffer: Vec<f32> = Vec::with_capacity(frame_size * ch * 2);
        let mut audio_pts: i64 = 0; // PTS in encoder timebase (1/sample_rate)

        // Allocate an AVFrame for feeding the encoder
        let frame_ptr = unsafe { ffmpeg_sys_next::av_frame_alloc() };
        if frame_ptr.is_null() {
            anyhow::bail!("Failed to allocate AVFrame for audio");
        }

        unsafe {
            (*frame_ptr).format = ffmpeg_sys_next::AVSampleFormat::AV_SAMPLE_FMT_FLTP as i32;
            (*frame_ptr).sample_rate = sample_rate as i32;
            (*frame_ptr).nb_samples = frame_size as i32;
            (*frame_ptr).ch_layout = std::mem::zeroed();
            ffmpeg_sys_next::av_channel_layout_default(&mut (*frame_ptr).ch_layout, ch as i32);
            let ret = ffmpeg_sys_next::av_frame_get_buffer(frame_ptr, 0);
            if ret < 0 {
                ffmpeg_sys_next::av_frame_free(&mut (frame_ptr as *mut _));
                anyhow::bail!(
                    "Failed to allocate audio frame buffer: {}",
                    ffmpeg_next::Error::from(ret)
                );
            }
        }

        loop {
            // Check cancellation
            if cancel.is_cancelled() {
                info!("Audio capture cancelled");
                break;
            }

            // Receive PCM chunks with timeout for cancel checks
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(samples) => {
                    pcm_buffer.extend_from_slice(&samples);
                }
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                    warn!("Audio channel disconnected");
                    break;
                }
            }

            // Process complete frames
            while pcm_buffer.len() >= frame_size * ch {
                // Make frame writable
                unsafe {
                    let ret = ffmpeg_sys_next::av_frame_make_writable(frame_ptr);
                    if ret < 0 {
                        error!("Failed to make audio frame writable");
                        break;
                    }
                }

                // De-interleave f32 -> planar f32
                unsafe {
                    for c in 0..ch {
                        let plane = (*frame_ptr).data[c] as *mut f32;
                        for s in 0..frame_size {
                            *plane.add(s) = pcm_buffer[s * ch + c];
                        }
                    }
                    (*frame_ptr).pts = audio_pts;
                }

                // Remove consumed samples
                pcm_buffer.drain(..frame_size * ch);
                audio_pts += frame_size as i64;

                // Send frame to encoder
                unsafe {
                    let ret =
                        ffmpeg_sys_next::avcodec_send_frame(encoder_ctx.as_mut_ptr(), frame_ptr);
                    if ret < 0 {
                        debug!(
                            "avcodec_send_frame error: {}",
                            ffmpeg_next::Error::from(ret)
                        );
                        continue;
                    }
                }

                // Receive encoded packets
                self.drain_encoder(&encoder_ctx, &buffer, audio_base_pts, sample_rate, fps)?;
            }
        }

        // Flush encoder
        unsafe {
            ffmpeg_sys_next::avcodec_send_frame(encoder_ctx.as_mut_ptr(), std::ptr::null());
        }
        self.drain_encoder(&encoder_ctx, &buffer, audio_base_pts, sample_rate, fps)?;

        // Cleanup
        unsafe {
            ffmpeg_sys_next::av_frame_free(&mut (frame_ptr as *mut _));
        }
        drop(stream);

        info!("Audio capture ended");
        Ok(())
    }

    /// Drain all available packets from the encoder and push to buffer.
    ///
    /// Uses the encoder's own PTS (in 1/sample_rate timebase) converted to
    /// buffer timebase (1/fps), offset by `audio_base_pts` for A/V alignment.
    /// This guarantees monotonically increasing PTS even when multiple packets
    /// are drained in a single call.
    fn drain_encoder(
        &self,
        encoder_ctx: &ffmpeg_next::codec::Context,
        buffer: &Arc<PacketBuffer>,
        audio_base_pts: i64,
        sample_rate: u32,
        fps: u32,
    ) -> Result<()> {
        unsafe {
            let mut pkt = ffmpeg_sys_next::av_packet_alloc();
            if pkt.is_null() {
                anyhow::bail!("Failed to allocate AVPacket");
            }

            loop {
                let ret =
                    ffmpeg_sys_next::avcodec_receive_packet(encoder_ctx.as_ptr() as *mut _, pkt);
                if ret < 0 {
                    // EAGAIN or EOF — no more packets right now
                    break;
                }

                let data = if !(*pkt).data.is_null() && (*pkt).size > 0 {
                    std::slice::from_raw_parts((*pkt).data, (*pkt).size as usize).to_vec()
                } else {
                    Vec::new()
                };

                // Convert encoder PTS (1/sample_rate) to buffer timebase (1/fps),
                // then add the wall-clock offset captured when audio started.
                let encoder_pts = (*pkt).pts;
                let buffer_pts = audio_base_pts + encoder_pts * fps as i64 / sample_rate as i64;

                let mut packet = Packet::new(data, PacketType::Audio, Instant::now());
                packet.pts = buffer_pts;
                packet.dts = buffer_pts;
                packet.is_keyframe = ((*pkt).flags & ffmpeg_sys_next::AV_PKT_FLAG_KEY as i32) != 0;

                buffer.push(packet);

                ffmpeg_sys_next::av_packet_unref(pkt);
            }

            ffmpeg_sys_next::av_packet_free(&mut pkt);
        }

        Ok(())
    }
}
