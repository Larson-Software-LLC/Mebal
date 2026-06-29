// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Packet representation for encoded media data

use bytes::Bytes;
use std::fmt;
use std::time::Instant;

/// Type of media packet
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// Video frame data (H.264/H.265)
    Video,
    /// Audio data (AAC)
    Audio,
}

/// A packet of encoded media data
#[derive(Clone)]
pub struct Packet {
    /// Raw encoded data
    pub data: Bytes,
    /// Type of packet
    pub packet_type: PacketType,
    /// Wall-clock timestamp when packet was captured
    pub timestamp: Instant,
    /// Presentation timestamp (in buffer timebase: 1/fps)
    pub pts: i64,
    /// Decode timestamp (in buffer timebase: 1/fps)
    pub dts: i64,
    /// Duration (in stream timebase)
    pub duration: i64,
    /// Whether this is a keyframe
    pub is_keyframe: bool,
    /// Stream index from the encoder
    pub stream_index: usize,
}

impl Packet {
    /// Create a new packet with default timestamps
    pub fn new(data: impl Into<Bytes>, packet_type: PacketType, timestamp: Instant) -> Self {
        Self {
            data: data.into(),
            packet_type,
            timestamp,
            pts: 0,
            dts: 0,
            duration: 0,
            is_keyframe: false,
            stream_index: 0,
        }
    }

    /// Create a packet from an FFmpeg encoded packet
    pub fn from_ffmpeg_packet(
        ffmpeg_packet: &ffmpeg_next::codec::packet::Packet,
        packet_type: PacketType,
    ) -> Self {
        Self {
            data: Bytes::copy_from_slice(ffmpeg_packet.data().unwrap_or(&[])),
            packet_type,
            timestamp: Instant::now(),
            pts: ffmpeg_packet.pts().unwrap_or(0),
            dts: ffmpeg_packet.dts().unwrap_or(0),
            duration: ffmpeg_packet.duration(),
            is_keyframe: ffmpeg_packet.is_key(),
            stream_index: ffmpeg_packet.stream(),
        }
    }

    /// Convert to an FFmpeg packet for muxing
    pub fn to_ffmpeg_packet(&self) -> ffmpeg_next::codec::packet::Packet {
        use ffmpeg_next::codec::packet::Packet as FfmpegPacket;

        let mut packet = FfmpegPacket::copy(self.data.as_ref());
        packet.set_pts(Some(self.pts));
        packet.set_dts(Some(self.dts));
        packet.set_duration(self.duration);
        packet.set_stream(self.stream_index);

        packet
    }
}

impl fmt::Debug for Packet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Packet")
            .field("type", &self.packet_type)
            .field("size", &self.data.len())
            .field("pts", &self.pts)
            .field("dts", &self.dts)
            .field("is_keyframe", &self.is_keyframe)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // H.264 NAL unit helpers (test-only; promote to pub when needed in production)
    const H264_NAL_TYPE_MASK: u8 = 0x1F;
    const H264_NAL_IDR: u8 = 5;
    const H264_NAL_SPS: u8 = 7;
    const H264_NAL_PPS: u8 = 8;

    fn h264_nal_unit_type(data: &[u8]) -> Option<u8> {
        if data.len() < 5 {
            return None;
        }
        let offset = if data.starts_with(&[0x00, 0x00, 0x00, 0x01]) {
            4
        } else if data.starts_with(&[0x00, 0x00, 0x01]) {
            3
        } else {
            return None;
        };
        Some(data[offset] & H264_NAL_TYPE_MASK)
    }

    fn is_h264_keyframe_nal(nal_type: u8) -> bool {
        nal_type == H264_NAL_IDR
    }

    fn is_h264_sps(data: &[u8]) -> bool {
        h264_nal_unit_type(data) == Some(H264_NAL_SPS)
    }

    fn is_h264_pps(data: &[u8]) -> bool {
        h264_nal_unit_type(data) == Some(H264_NAL_PPS)
    }

    #[test]
    fn test_packet_creation() {
        let data = vec![1, 2, 3, 4, 5];
        let packet = Packet::new(data.clone(), PacketType::Video, Instant::now());

        assert_eq!(packet.data.as_ref(), data.as_slice());
        assert_eq!(packet.packet_type, PacketType::Video);
        assert_eq!(packet.data.len(), 5);
    }

    #[test]
    fn test_h264_nal_detection() {
        // SPS with 4-byte start code
        let sps = vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x0A];
        assert_eq!(h264_nal_unit_type(&sps), Some(7));
        assert!(is_h264_sps(&sps));

        // PPS with 3-byte start code
        let pps = vec![0x00, 0x00, 0x01, 0x68, 0xCE, 0x3C, 0x80];
        assert_eq!(h264_nal_unit_type(&pps), Some(8));
        assert!(is_h264_pps(&pps));

        // IDR slice (keyframe)
        let idr = vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00];
        assert_eq!(h264_nal_unit_type(&idr), Some(5));
        assert!(is_h264_keyframe_nal(5));
    }
}
