// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Packet representation for encoded video data

use bytes::Bytes;
use std::time::Instant;

/// Type of packet
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// Video frame data (H.264/H.265)
    Video,
    /// Audio data (AAC/Opus)
    Audio,
    /// End of stream marker
    EndOfStream,
}

/// A packet of encoded media data
#[derive(Clone)]
pub struct Packet {
    /// Raw encoded data
    pub data: Bytes,
    /// Type of packet
    pub packet_type: PacketType,
    /// Timestamp when packet was captured
    pub timestamp: Instant,
    /// Presentation timestamp (in stream timebase)
    pub pts: i64,
    /// Decode timestamp (in stream timebase)
    pub dts: i64,
    /// Duration (in stream timebase)
    pub duration: i64,
    /// Is this a keyframe
    pub is_keyframe: bool,
    /// Sequence number for ordering
    pub sequence: u64,
    /// Stream index (for multi-stream files)
    pub stream_index: usize,
}

impl Packet {
    /// Create a new packet
    pub fn new(data: impl Into<Bytes>, packet_type: PacketType, timestamp: Instant) -> Self {
        Self {
            data: data.into(),
            packet_type,
            timestamp,
            pts: 0,
            dts: 0,
            duration: 0,
            is_keyframe: false,
            sequence: 0,
            stream_index: 0,
        }
    }

    /// Create a video packet from FFmpeg packet data
    pub fn from_ffmpeg_packet(
        ffmpeg_packet: &ffmpeg_next::codec::packet::Packet,
        packet_type: PacketType,
    ) -> Self {
        let data = Bytes::copy_from_slice(ffmpeg_packet.data().unwrap_or(&[]));
        let is_keyframe = ffmpeg_packet.is_key();

        Self {
            data,
            packet_type,
            timestamp: Instant::now(),
            pts: ffmpeg_packet.pts().unwrap_or(0),
            dts: ffmpeg_packet.dts().unwrap_or(0),
            duration: ffmpeg_packet.duration(),
            is_keyframe,
            sequence: 0,
            stream_index: ffmpeg_packet.stream(),
        }
    }

    /// Check if this is a video packet
    pub fn is_video(&self) -> bool {
        self.packet_type == PacketType::Video
    }

    /// Check if this is an audio packet
    pub fn is_audio(&self) -> bool {
        self.packet_type == PacketType::Audio
    }

    /// Get packet size in bytes
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// Convert to FFmpeg packet for writing
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

impl std::fmt::Debug for Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Packet")
            .field("type", &self.packet_type)
            .field("size", &self.data.len())
            .field("pts", &self.pts)
            .field("dts", &self.dts)
            .field("is_keyframe", &self.is_keyframe)
            .field("sequence", &self.sequence)
            .finish()
    }
}

/// Extract H.264 NAL unit type from packet data
pub fn h264_nal_unit_type(data: &[u8]) -> Option<u8> {
    if data.len() < 5 {
        return None;
    }

    // Check for start code (0x00000001 or 0x000001)
    let offset = if data.starts_with(&[0x00, 0x00, 0x00, 0x01]) {
        4
    } else if data.starts_with(&[0x00, 0x00, 0x01]) {
        3
    } else {
        return None;
    };

    // NAL unit type is in the lower 5 bits of the first byte after start code
    Some(data[offset] & 0x1F)
}

/// Check if NAL unit type is a keyframe (IDR slice)
pub fn is_h264_keyframe_nal(nal_type: u8) -> bool {
    nal_type == 5 // IDR slice
}

/// Check if packet contains H.264 SPS (Sequence Parameter Set)
pub fn is_h264_sps(data: &[u8]) -> bool {
    h264_nal_unit_type(data).map(|t| t == 7).unwrap_or(false)
}

/// Check if packet contains H.264 PPS (Picture Parameter Set)
pub fn is_h264_pps(data: &[u8]) -> bool {
    h264_nal_unit_type(data).map(|t| t == 8).unwrap_or(false)
}

/// Check if packet contains H.264 SEI (Supplemental Enhancement Information)
pub fn is_h264_sei(data: &[u8]) -> bool {
    h264_nal_unit_type(data).map(|t| t == 6).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_creation() {
        let data = vec![1, 2, 3, 4, 5];
        let packet = Packet::new(data.clone(), PacketType::Video, Instant::now());

        assert_eq!(packet.data.as_ref(), data.as_slice());
        assert!(packet.is_video());
        assert_eq!(packet.size(), 5);
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
