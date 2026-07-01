// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Encoder setup for the zero-copy D3D11 pipeline.
//!
//! Builds an H.264 hardware encoder (NVENC, then AMF) that consumes D3D11 BGRA
//! frames from `HwFrames` and does the RGB->NV12 conversion internally. There is
//! no software fallback — the pipeline is hardware-only by design.

use anyhow::{Context, Result};
use ffmpeg_sys_next as ff;
use tracing::{info, warn};

use super::hwframe::HwFrames;
use crate::config::Config;

/// Create an H.264 hardware encoder bound to `hw`'s D3D11 frame pool.
///
/// Returns `(encoder, encoder_name)`. Priority:
/// 1. User-specified encoder from config
/// 2. h264_nvenc (NVIDIA)
/// 3. h264_amf (AMD)
pub fn create_encoder(
    config: &Config,
    hw: &HwFrames,
) -> Result<(ffmpeg_next::encoder::Video, String)> {
    if let Some(ref name) = config.encoder {
        info!("User requested encoder: {}", name);
        return build_encoder(name, config, hw)
            .with_context(|| format!("Failed to create user-specified encoder '{}'", name));
    }

    match build_encoder("h264_nvenc", config, hw) {
        Ok(result) => {
            info!("Using h264_nvenc (NVIDIA hardware encoder)");
            Ok(result)
        }
        Err(e) => {
            warn!("h264_nvenc not available ({}), trying h264_amf", e);
            build_encoder("h264_amf", config, hw)
                .context("no hardware H.264 encoder (NVENC/AMF) available")
        }
    }
}

/// Build and open an encoder, binding it to the D3D11 frame pool.
fn build_encoder(
    codec_name: &str,
    config: &Config,
    hw: &HwFrames,
) -> Result<(ffmpeg_next::encoder::Video, String)> {
    let (width, height) = config.resolution;
    let fps = config.fps as i32;

    let codec = ffmpeg_next::encoder::find_by_name(codec_name)
        .with_context(|| format!("Encoder '{}' not found", codec_name))?;

    let ctx = ffmpeg_next::codec::Context::new_with_codec(codec);
    let mut encoder = ctx.encoder().video()?;

    encoder.set_width(width);
    encoder.set_height(height);
    encoder.set_time_base(ffmpeg_next::Rational::new(1, fps));
    encoder.set_frame_rate(Some(ffmpeg_next::Rational::new(fps, 1)));
    encoder.set_bit_rate(config.bitrate_kbps * 1000);
    encoder.set_gop(config.fps * crate::config::GOP_INTERVAL_SECS);

    // Hardware fields ffmpeg-next can't set: feed D3D11 BGRA frames and let the
    // encoder convert. Global header so MP4 extradata (SPS/PPS) is populated; the
    // writer depends on it.
    //
    // NVENC converts RGB input to YUV with the BT.601 (limited) matrix internally
    // and ignores a BT.709 request, so we signal BT.601 to keep the VUI matching
    // the actual pixels — signalling 709 here risks a matrix mismatch (colour
    // shift) on drivers that honour the VUI.
    unsafe {
        let c = encoder.as_mut_ptr();
        (*c).pix_fmt = ff::AVPixelFormat::AV_PIX_FMT_D3D11;
        (*c).hw_frames_ctx = ff::av_buffer_ref(hw.frames_ref());
        (*c).flags |= ff::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
        (*c).color_range = ff::AVColorRange::AVCOL_RANGE_MPEG;
        (*c).colorspace = ff::AVColorSpace::AVCOL_SPC_SMPTE170M;
        (*c).color_primaries = ff::AVColorPrimaries::AVCOL_PRI_SMPTE170M;
        (*c).color_trc = ff::AVColorTransferCharacteristic::AVCOL_TRC_SMPTE170M;
    }

    let mut opts = ffmpeg_next::Dictionary::new();
    if codec_name == "h264_nvenc" {
        opts.set("preset", "p4");
        opts.set("tune", "ll");
        opts.set("rc", "cbr");
        opts.set("zerolatency", "1");
    } else if codec_name == "h264_amf" {
        opts.set("usage", "lowlatency");
        opts.set("rc", "cbr");
    }

    let encoder = encoder.open_with(opts)?;
    Ok((encoder, codec_name.to_string()))
}

/// Get information about available hardware encoders by probing FFmpeg.
pub fn get_encoder_info() -> Vec<(String, bool)> {
    let encoders = [("h264_nvenc", "NVIDIA NVENC"), ("h264_amf", "AMD AMF")];

    encoders
        .iter()
        .map(|(codec_name, display_name)| {
            let available = ffmpeg_next::encoder::find_by_name(codec_name).is_some();
            (display_name.to_string(), available)
        })
        .collect()
}
