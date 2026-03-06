// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Video file writer for saving replay clips
use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, warn};

use crate::buffer::{AudioParams, Packet, PacketType};
use crate::config::Config;

/// Video stream parameters for the output container
#[derive(Debug, Clone)]
pub struct VideoParams {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: usize,
    pub extradata: Vec<u8>,
}

/// Video writer for saving replay clips
pub struct VideoWriter {
    video_params: VideoParams,
    audio_params: Option<AudioParams>,
    audio_bitrate_kbps: usize,
}

impl VideoWriter {
    pub fn new(
        config: &Config,
        codec_extradata: Vec<u8>,
        audio_params: Option<AudioParams>,
    ) -> Self {
        Self {
            video_params: VideoParams {
                width: config.resolution.0,
                height: config.resolution.1,
                fps: config.fps,
                bitrate_kbps: config.bitrate_kbps,
                extradata: codec_extradata,
            },
            audio_params,
            audio_bitrate_kbps: config.audio_bitrate_kbps,
        }
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

        // Add video stream — codec arg is just for identification, we set codecpar manually
        let codec = ffmpeg_next::encoder::find_by_name("libx264")
            .or_else(|| ffmpeg_next::encoder::find(ffmpeg_next::codec::Id::H264))
            .context("H264 codec not found")?;

        let mut stream = output_ctx.add_stream(codec)?;
        let stream_index = stream.index();

        // Set codec parameters via FFI (ffmpeg-next's Parameters loses codec_id)
        unsafe {
            let st = stream.as_mut_ptr();
            let codecpar = (*st).codecpar;

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
                    std::ptr::copy_nonoverlapping(self.video_params.extradata.as_ptr(), data, size);
                    (*codecpar).extradata = data;
                    (*codecpar).extradata_size = size as i32;
                }
            }

            (*st).time_base = ffmpeg_sys_next::AVRational {
                num: 1,
                den: self.video_params.fps as i32,
            };
        }

        // --- Conditionally add audio stream ---
        let audio_stream_index = if let Some(ref audio) = self.audio_params {
            let idx = unsafe {
                let fmt_ctx = output_ctx.as_mut_ptr();
                let audio_codec = ffmpeg_sys_next::avcodec_find_encoder(
                    ffmpeg_sys_next::AVCodecID::AV_CODEC_ID_AAC,
                );
                let st = ffmpeg_sys_next::avformat_new_stream(fmt_ctx, audio_codec);
                if st.is_null() {
                    anyhow::bail!("Failed to add audio stream");
                }
                let idx = (*st).index as usize;

                let codecpar = (*st).codecpar;
                (*codecpar).codec_type = ffmpeg_sys_next::AVMediaType::AVMEDIA_TYPE_AUDIO;
                (*codecpar).codec_id = ffmpeg_sys_next::AVCodecID::AV_CODEC_ID_AAC;
                (*codecpar).sample_rate = audio.sample_rate as i32;
                (*codecpar).bit_rate = (self.audio_bitrate_kbps * 1000) as i64;

                (*codecpar).ch_layout = std::mem::zeroed();
                ffmpeg_sys_next::av_channel_layout_default(
                    &mut (*codecpar).ch_layout,
                    audio.channels as i32,
                );

                // Set audio extradata
                if !audio.extradata.is_empty() {
                    let size = audio.extradata.len();
                    let data = ffmpeg_sys_next::av_mallocz(
                        size + ffmpeg_sys_next::AV_INPUT_BUFFER_PADDING_SIZE as usize,
                    ) as *mut u8;
                    if !data.is_null() {
                        std::ptr::copy_nonoverlapping(audio.extradata.as_ptr(), data, size);
                        (*codecpar).extradata = data;
                        (*codecpar).extradata_size = size as i32;
                    }
                }

                (*codecpar).frame_size = audio.frame_size as i32;

                (*st).time_base = ffmpeg_sys_next::AVRational {
                    num: 1,
                    den: audio.sample_rate as i32,
                };

                idx
            };
            info!("Audio stream added at index {}", idx);
            Some(idx)
        } else {
            None
        };

        // Write header — the muxer may change the stream's time_base
        output_ctx
            .write_header()
            .context("Failed to write output header")?;

        // Get the stream's actual time_base (muxer may have changed it from 1/fps)
        let video_stream_tb = output_ctx.stream(stream_index).unwrap().time_base();
        let buffer_time_base = ffmpeg_next::Rational::new(1, self.video_params.fps as i32);

        let audio_stream_tb =
            audio_stream_index.map(|idx| output_ctx.stream(idx).unwrap().time_base());

        info!(
            "Time base: buffer={}/{}, video_stream={}/{}{}",
            buffer_time_base.numerator(),
            buffer_time_base.denominator(),
            video_stream_tb.numerator(),
            video_stream_tb.denominator(),
            if let Some(atb) = audio_stream_tb {
                format!(", audio_stream={}/{}", atb.numerator(), atb.denominator())
            } else {
                String::new()
            },
        );

        // Compute per-stream PTS offsets (rebase to 0)
        let video_pts_offset = packets
            .iter()
            .find(|p| p.packet_type == PacketType::Video)
            .map(|p| p.pts)
            .unwrap_or(0);
        let video_dts_offset = packets
            .iter()
            .find(|p| p.packet_type == PacketType::Video)
            .map(|p| p.dts)
            .unwrap_or(0);

        let audio_pts_offset = packets
            .iter()
            .find(|p| p.packet_type == PacketType::Audio)
            .map(|p| p.pts)
            .unwrap_or(0);
        let audio_dts_offset = packets
            .iter()
            .find(|p| p.packet_type == PacketType::Audio)
            .map(|p| p.dts)
            .unwrap_or(0);

        // Guard: skip audio packets with non-monotonic DTS (stale session data).
        let mut last_audio_dts: Option<i64> = None;

        for packet in &packets {
            match packet.packet_type {
                PacketType::Video => {
                    let mut ffmpeg_pkt = packet.to_ffmpeg_packet();
                    ffmpeg_pkt.set_pts(Some(packet.pts - video_pts_offset));
                    ffmpeg_pkt.set_dts(Some(packet.dts - video_dts_offset));
                    ffmpeg_pkt.set_stream(stream_index);
                    ffmpeg_pkt.rescale_ts(buffer_time_base, video_stream_tb);

                    if packet.is_keyframe {
                        ffmpeg_pkt.set_flags(ffmpeg_next::codec::packet::Flags::KEY);
                    }

                    ffmpeg_pkt
                        .write_interleaved(&mut output_ctx)
                        .with_context(|| {
                            format!("Failed to write video packet PTS={}", packet.pts)
                        })?;
                }
                PacketType::Audio => {
                    if let Some(audio_idx) = audio_stream_index {
                        let rebased_dts = packet.dts - audio_dts_offset;

                        if let Some(last) = last_audio_dts {
                            if rebased_dts <= last {
                                warn!(
                                    "Skipping non-monotonic audio packet (DTS={}, last={})",
                                    rebased_dts, last
                                );
                                continue;
                            }
                        }
                        last_audio_dts = Some(rebased_dts);

                        let audio_tb = audio_stream_tb.unwrap();
                        let mut ffmpeg_pkt = packet.to_ffmpeg_packet();
                        ffmpeg_pkt.set_pts(Some(packet.pts - audio_pts_offset));
                        ffmpeg_pkt.set_dts(Some(rebased_dts));
                        ffmpeg_pkt.set_stream(audio_idx);
                        ffmpeg_pkt.rescale_ts(buffer_time_base, audio_tb);

                        ffmpeg_pkt
                            .write_interleaved(&mut output_ctx)
                            .with_context(|| {
                                format!("Failed to write audio packet PTS={}", packet.pts)
                            })?;
                    }
                }
            }
        }

        // Write trailer
        output_ctx
            .write_trailer()
            .context("Failed to write output trailer")?;

        info!("Media file written to {:?}", path);
        Ok(())
    }
}

/// Find the index of the first video keyframe (ignores audio keyframes).
pub fn find_first_keyframe(packets: &[Packet]) -> Option<usize> {
    packets
        .iter()
        .position(|p| p.is_keyframe && p.packet_type == PacketType::Video)
}

/// Trim packets to start from a keyframe
pub fn trim_to_keyframe(packets: Vec<Packet>) -> Vec<Packet> {
    if let Some(keyframe_idx) = find_first_keyframe(&packets) {
        packets.into_iter().skip(keyframe_idx).collect()
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn create_test_packet(pts: i64, is_keyframe: bool) -> Packet {
        Packet {
            data: bytes::Bytes::from(vec![0u8; 100]),
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
    fn test_find_first_keyframe_ignores_audio() {
        let packets = vec![
            // Audio keyframe should be skipped
            Packet {
                data: bytes::Bytes::from(vec![0u8; 50]),
                packet_type: PacketType::Audio,
                timestamp: Instant::now(),
                pts: 0,
                dts: 0,
                duration: 1,
                is_keyframe: true,
                sequence: 0,
                stream_index: 0,
            },
            create_test_packet(1, false),
            create_test_packet(2, true),
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
