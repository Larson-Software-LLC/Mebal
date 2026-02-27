// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Encoder setup and configuration
//!
//! Creates an H.264 encoder, preferring NVENC hardware encoding
//! with automatic fallback to libx264 software encoding.

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::config::Config;

/// Create an encoder based on config and decoder parameters.
///
/// Returns `(encoder, encoder_name)`.
///
/// Priority:
/// 1. User-specified encoder from config
/// 2. h264_nvenc (NVIDIA hardware)
/// 3. libx264 (software fallback)
pub fn create_encoder(
    config: &Config,
    _decoder: &ffmpeg_next::decoder::Video,
) -> Result<(ffmpeg_next::encoder::Video, String)> {
    let width = config.resolution.0;
    let height = config.resolution.1;

    // If user specified an encoder, try only that
    if let Some(ref name) = config.encoder {
        info!("User requested encoder: {}", name);
        return build_encoder(name, config, width, height)
            .with_context(|| format!("Failed to create user-specified encoder '{}'", name));
    }

    // Auto-detect: try NVENC first, then libx264
    match build_encoder("h264_nvenc", config, width, height) {
        Ok(result) => {
            info!("Using h264_nvenc (NVIDIA hardware encoder)");
            Ok(result)
        }
        Err(e) => {
            warn!("h264_nvenc not available ({}), falling back to libx264", e);
            build_encoder("libx264", config, width, height)
                .context("Failed to create libx264 fallback encoder")
        }
    }
}

/// Build an encoder with the given codec name
fn build_encoder(
    codec_name: &str,
    config: &Config,
    width: u32,
    height: u32,
) -> Result<(ffmpeg_next::encoder::Video, String)> {
    let codec = ffmpeg_next::encoder::find_by_name(codec_name)
        .with_context(|| format!("Encoder '{}' not found", codec_name))?;

    let ctx = ffmpeg_next::codec::Context::new_with_codec(codec);

    {
        let mut encoder = ctx.encoder().video()?;

        encoder.set_width(width);
        encoder.set_height(height);
        encoder.set_format(ffmpeg_next::format::Pixel::NV12);
        encoder.set_time_base(ffmpeg_next::Rational::new(1, config.fps as i32));
        encoder.set_frame_rate(Some(ffmpeg_next::Rational::new(config.fps as i32, 1)));
        encoder.set_bit_rate(config.bitrate_kbps * 1000);
        encoder.set_gop(config.fps * 2); // Keyframe every 2 seconds

        // Codec-specific options
        let mut opts = ffmpeg_next::Dictionary::new();

        if codec_name == "h264_nvenc" {
            opts.set("preset", "p4");
            opts.set("tune", "ll");
            opts.set("rc", "cbr");
            opts.set("zerolatency", "1");
        } else if codec_name == "libx264" {
            opts.set("preset", "ultrafast");
            opts.set("tune", "zerolatency");
            opts.set("crf", "23");
        }

        let encoder = encoder.open_with(opts)?;
        Ok((encoder, codec_name.to_string()))
    }
}

/// Get information about available encoders by probing FFmpeg
pub fn get_encoder_info() -> Vec<(String, bool)> {
    let encoders = [
        ("h264_nvenc", "NVIDIA NVENC"),
        ("h264_amf", "AMD AMF"),
        ("h264_qsv", "Intel QuickSync"),
        ("libx264", "x264 (Software)"),
    ];

    encoders
        .iter()
        .map(|(codec_name, display_name)| {
            let available = ffmpeg_next::encoder::find_by_name(codec_name).is_some();
            (display_name.to_string(), available)
        })
        .collect()
}
