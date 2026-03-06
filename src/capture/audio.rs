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

/// RAII wrapper for an FFmpeg `AVFrame` pointer.
///
/// Calls `av_frame_free` on drop, preventing leaks on early returns or `?`.
struct AvFrame(*mut ffmpeg_sys_next::AVFrame);

impl AvFrame {
    fn alloc() -> Result<Self> {
        let ptr = unsafe { ffmpeg_sys_next::av_frame_alloc() };
        if ptr.is_null() {
            anyhow::bail!("Failed to allocate AVFrame");
        }
        Ok(Self(ptr))
    }

    fn as_mut_ptr(&mut self) -> *mut ffmpeg_sys_next::AVFrame {
        self.0
    }
}

impl Drop for AvFrame {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ffmpeg_sys_next::av_frame_free(&mut self.0) }
        }
    }
}

/// RAII wrapper for an FFmpeg `AVPacket` pointer.
///
/// Calls `av_packet_free` on drop (which internally unrefs then frees).
struct AvPacket(*mut ffmpeg_sys_next::AVPacket);

impl AvPacket {
    fn alloc() -> Result<Self> {
        let ptr = unsafe { ffmpeg_sys_next::av_packet_alloc() };
        if ptr.is_null() {
            anyhow::bail!("Failed to allocate AVPacket");
        }
        Ok(Self(ptr))
    }

    fn as_mut_ptr(&mut self) -> *mut ffmpeg_sys_next::AVPacket {
        self.0
    }
}

impl Drop for AvPacket {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ffmpeg_sys_next::av_packet_free(&mut self.0) }
        }
    }
}

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

    /// Run the audio capture loop (blocking — call from `spawn_blocking`).
    ///
    /// `capture_start` is the shared epoch so audio and video PTS are aligned.
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

        // Configure AAC encoder via FFI (ffmpeg-next doesn't expose audio encoder fully)
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

        // PTS offset so audio aligns with video (both share capture_start epoch).
        let audio_base_pts =
            (capture_start.elapsed().as_secs_f64() * self.config.fps as f64) as i64;

        // --- Processing loop: receive PCM, accumulate frames, encode ---
        let fps = self.config.fps;
        let ch = channels as usize;
        let mut pcm_buffer: Vec<f32> = Vec::with_capacity(frame_size * ch * 2);
        let mut audio_pts: i64 = 0; // PTS in encoder timebase (1/sample_rate)

        // Allocate an AVFrame for feeding the encoder (freed automatically via Drop)
        let mut frame = AvFrame::alloc()?;
        unsafe {
            let p = frame.as_mut_ptr();
            (*p).format = ffmpeg_sys_next::AVSampleFormat::AV_SAMPLE_FMT_FLTP as i32;
            (*p).sample_rate = sample_rate as i32;
            (*p).nb_samples = frame_size as i32;
            (*p).ch_layout = std::mem::zeroed();
            ffmpeg_sys_next::av_channel_layout_default(&mut (*p).ch_layout, ch as i32);
            let ret = ffmpeg_sys_next::av_frame_get_buffer(p, 0);
            if ret < 0 {
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
                    let ret = ffmpeg_sys_next::av_frame_make_writable(frame.as_mut_ptr());
                    if ret < 0 {
                        error!("Failed to make audio frame writable");
                        break;
                    }
                }

                // De-interleave f32 -> planar f32
                unsafe {
                    let p = frame.as_mut_ptr();
                    for c in 0..ch {
                        let plane = (*p).data[c] as *mut f32;
                        for s in 0..frame_size {
                            *plane.add(s) = pcm_buffer[s * ch + c];
                        }
                    }
                    (*p).pts = audio_pts;
                }

                // Remove consumed samples
                pcm_buffer.drain(..frame_size * ch);
                audio_pts += frame_size as i64;

                // Send frame to encoder
                unsafe {
                    let ret = ffmpeg_sys_next::avcodec_send_frame(
                        encoder_ctx.as_mut_ptr(),
                        frame.as_mut_ptr(),
                    );
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

        // Skip flush on cancel — avoids stale packets racing into a cleared buffer.
        if !cancel.is_cancelled() {
            unsafe {
                ffmpeg_sys_next::avcodec_send_frame(encoder_ctx.as_mut_ptr(), std::ptr::null());
            }
            self.drain_encoder(&encoder_ctx, &buffer, audio_base_pts, sample_rate, fps)?;
        }

        // Drop cpal stream first to stop callbacks before frame is freed.
        drop(stream);
        // `frame` dropped here via RAII → av_frame_free called automatically.

        info!("Audio capture ended");
        Ok(())
    }

    /// Drain encoded packets from the AAC encoder and push to the buffer.
    ///
    /// Converts encoder PTS (1/sample_rate) → buffer timebase (1/fps)
    /// and adds `audio_base_pts` for A/V alignment.
    fn drain_encoder(
        &self,
        encoder_ctx: &ffmpeg_next::codec::Context,
        buffer: &Arc<PacketBuffer>,
        audio_base_pts: i64,
        sample_rate: u32,
        fps: u32,
    ) -> Result<()> {
        let mut pkt = AvPacket::alloc()?;

        loop {
            let ret = unsafe {
                ffmpeg_sys_next::avcodec_receive_packet(
                    encoder_ctx.as_ptr() as *mut _,
                    pkt.as_mut_ptr(),
                )
            };
            if ret < 0 {
                // EAGAIN or EOF — no more packets right now
                break;
            }

            let (data, encoder_pts, flags) = unsafe {
                let p = pkt.as_mut_ptr();
                let data = if !(*p).data.is_null() && (*p).size > 0 {
                    std::slice::from_raw_parts((*p).data, (*p).size as usize).to_vec()
                } else {
                    Vec::new()
                };
                (data, (*p).pts, (*p).flags)
            };

            // Convert encoder PTS (1/sample_rate) to buffer timebase (1/fps),
            // then add the wall-clock offset captured when audio started.
            let buffer_pts = audio_base_pts + encoder_pts * fps as i64 / sample_rate as i64;

            // Skip AAC encoder priming delay packets (negative PTS).
            if buffer_pts < 0 {
                unsafe { ffmpeg_sys_next::av_packet_unref(pkt.as_mut_ptr()) };
                continue;
            }

            let mut packet = Packet::new(data, PacketType::Audio, Instant::now());
            packet.pts = buffer_pts;
            packet.dts = buffer_pts;
            packet.is_keyframe = (flags & ffmpeg_sys_next::AV_PKT_FLAG_KEY) != 0;

            buffer.push(packet);

            unsafe { ffmpeg_sys_next::av_packet_unref(pkt.as_mut_ptr()) };
        }
        // `pkt` dropped here via RAII → av_packet_free called automatically.

        Ok(())
    }
}
