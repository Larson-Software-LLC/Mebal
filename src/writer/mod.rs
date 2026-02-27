// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Video file writer for saving replay clips
//!
//! Writes encoded H.264 packets to an MP4 container file using FFmpeg.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::info;

use crate::buffer::{Packet, PacketType};
use crate::config::Config;

/// Video parameters for output
#[derive(Debug, Clone)]
pub struct VideoParams {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: usize,
    pub codec_name: String,
    pub extradata: Vec<u8>,
}

/// Video writer for saving replay clips
pub struct VideoWriter {
    video_params: VideoParams,
}

impl VideoWriter {
    /// Create a new video writer
    pub fn new(config: &Config, codec_extradata: Vec<u8>) -> Self {
        let video_params = VideoParams {
            width: config.resolution.0,
            height: config.resolution.1,
            fps: config.fps,
            bitrate_kbps: config.bitrate_kbps,
            codec_name: config.encoder.clone().unwrap_or_else(|| "h264".to_string()),
            extradata: codec_extradata,
        };

        Self { video_params }
    }

    /// Write packets to a video file
    ///
    /// This is a blocking operation — call from spawn_blocking.
    pub fn write_packets_blocking<P: AsRef<Path>>(
        &self,
        packets: Vec<Packet>,
        output_path: P,
    ) -> Result<()> {
        let path = output_path.as_ref();
        info!("Writing {} packets to {:?}", packets.len(), path);

        if packets.is_empty() {
            anyhow::bail!("No packets to write");
        }

        // Trim to start from a keyframe
        let packets = trim_to_keyframe(packets);
        if packets.is_empty() {
            anyhow::bail!("No keyframe found in packets");
        }

        info!(
            "Writing {} packets (trimmed to keyframe), first PTS={}",
            packets.len(),
            packets[0].pts
        );

        // Create output context
        let mut output_ctx = ffmpeg_next::format::output(&path)
            .with_context(|| format!("Failed to create output file {:?}", path))?;

        // Add video stream
        let codec = ffmpeg_next::encoder::find_by_name("libx264")
            .or_else(|| ffmpeg_next::encoder::find(ffmpeg_next::codec::Id::H264))
            .context("H264 codec not found")?;

        let mut stream = output_ctx.add_stream(codec)?;

        // Set codec parameters via unsafe since high-level API is limited
        {
            let mut params = stream.parameters();
            unsafe {
                let codecpar = params.as_mut_ptr();
                (*codecpar).codec_type = ffmpeg_sys_next::AVMediaType::AVMEDIA_TYPE_VIDEO;
                (*codecpar).codec_id = ffmpeg_sys_next::AVCodecID::AV_CODEC_ID_H264;
                (*codecpar).width = self.video_params.width as i32;
                (*codecpar).height = self.video_params.height as i32;
                (*codecpar).bit_rate = (self.video_params.bitrate_kbps * 1000) as i64;
                (*codecpar).format = ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_NV12 as i32;

                // Set extradata (SPS/PPS)
                if !self.video_params.extradata.is_empty() {
                    let size = self.video_params.extradata.len();
                    let data = ffmpeg_sys_next::av_mallocz(
                        size + ffmpeg_sys_next::AV_INPUT_BUFFER_PADDING_SIZE as usize,
                    ) as *mut u8;
                    if !data.is_null() {
                        std::ptr::copy_nonoverlapping(
                            self.video_params.extradata.as_ptr(),
                            data,
                            size,
                        );
                        (*codecpar).extradata = data;
                        (*codecpar).extradata_size = size as i32;
                    }
                }
            }

            stream.set_parameters(params);
        }

        // Set time base
        unsafe {
            let st = stream.as_mut_ptr();
            (*st).time_base = ffmpeg_sys_next::AVRational {
                num: 1,
                den: self.video_params.fps as i32,
            };
        }

        let stream_index = stream.index();

        // Write header
        output_ctx
            .write_header()
            .context("Failed to write output header")?;

        // Rebase timestamps so first packet starts at 0
        let pts_offset = packets[0].pts;
        let dts_offset = packets[0].dts;

        for packet in &packets {
            if packet.packet_type != PacketType::Video {
                continue;
            }

            let mut ffmpeg_pkt = packet.to_ffmpeg_packet();

            // Rebase PTS/DTS
            ffmpeg_pkt.set_pts(Some(packet.pts - pts_offset));
            ffmpeg_pkt.set_dts(Some(packet.dts - dts_offset));
            ffmpeg_pkt.set_stream(stream_index);

            // Set keyframe flag
            if packet.is_keyframe {
                ffmpeg_pkt.set_flags(ffmpeg_next::codec::packet::Flags::KEY);
            }

            ffmpeg_pkt
                .write_interleaved(&mut output_ctx)
                .with_context(|| format!("Failed to write packet PTS={}", packet.pts))?;
        }

        // Write trailer
        output_ctx
            .write_trailer()
            .context("Failed to write output trailer")?;

        info!("Video file written to {:?}", path);
        Ok(())
    }
}

/// Find first keyframe in packet list
pub fn find_first_keyframe(packets: &[Packet]) -> Option<usize> {
    packets.iter().position(|p| p.is_keyframe)
}

/// Trim packets to start from a keyframe
pub fn trim_to_keyframe(packets: Vec<Packet>) -> Vec<Packet> {
    if let Some(keyframe_idx) = find_first_keyframe(&packets) {
        packets.into_iter().skip(keyframe_idx).collect()
    } else {
        packets
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn create_test_packet(pts: i64, is_keyframe: bool) -> Packet {
        Packet {
            data: vec![0u8; 100],
            packet_type: PacketType::Video,
            timestamp: Instant::now(),
            pts,
            dts: pts,
            duration: 1,
            is_keyframe,
            sequence: pts as u64,
            stream_index: 0,
        }
    }

    #[test]
    fn test_find_first_keyframe() {
        let packets = vec![
            create_test_packet(0, false),
            create_test_packet(1, false),
            create_test_packet(2, true),
            create_test_packet(3, false),
        ];

        assert_eq!(find_first_keyframe(&packets), Some(2));
    }

    #[test]
    fn test_trim_to_keyframe() {
        let packets = vec![
            create_test_packet(0, false),
            create_test_packet(1, false),
            create_test_packet(2, true),
            create_test_packet(3, false),
        ];

        let trimmed = trim_to_keyframe(packets);
        assert_eq!(trimmed.len(), 2);
        assert!(trimmed[0].is_keyframe);
    }
}
