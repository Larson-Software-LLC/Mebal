// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Circular packet buffer for storing encoded video data
//!
//! This module provides a thread-safe, lock-free(ish) circular buffer
//! optimized for storing H.264 encoded video packets. It maintains
//! packets in chronological order and can efficiently retrieve
//! the last N seconds of video.

use parking_lot::RwLock;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tracing::{debug, trace};

mod packet;

pub use packet::{Packet, PacketType};

/// Audio stream parameters stored alongside the buffer
#[derive(Debug, Clone)]
pub struct AudioParams {
    pub sample_rate: u32,
    pub channels: u16,
    pub frame_size: u32,
    pub extradata: Vec<u8>,
}

/// A thread-safe circular buffer for video packets
pub struct PacketBuffer {
    /// Internal storage for packets
    inner: RwLock<BufferInner>,
    /// Maximum duration to keep in buffer (in seconds)
    max_duration_secs: u32,
    /// Expected frames per second (for capacity estimation)
    fps: u32,
    /// GOP interval in seconds (used for keyframe compensation during retrieval)
    gop_secs: u32,
    /// Codec extradata (SPS/PPS) needed by the writer to produce valid MP4
    codec_extradata: RwLock<Option<Vec<u8>>>,
    /// Audio stream parameters (set by audio capture, read by writer)
    audio_params: RwLock<Option<AudioParams>>,
}

/// Internal buffer state
struct BufferInner {
    /// The packet queue
    packets: VecDeque<Packet>,
    /// Total bytes currently in buffer
    total_bytes: usize,
    /// Maximum bytes before eviction
    max_bytes: usize,
    /// Sequence number for ordering
    sequence: u64,
}

impl PacketBuffer {
    /// Create a new packet buffer
    ///
    /// # Arguments
    /// * `max_duration_secs` - Maximum duration to keep in buffer
    /// * `fps` - Expected frames per second
    /// * `bitrate_kbps` - Video bitrate in kilobits per second (used for buffer sizing)
    /// * `gop_secs` - GOP interval in seconds (used for keyframe compensation)
    pub fn new(max_duration_secs: u32, fps: u32, bitrate_kbps: usize, gop_secs: u32) -> Self {
        let estimated_packets = (fps * max_duration_secs) as usize;
        // Compute max_bytes from bitrate with 1.25x headroom for VBR spikes
        let max_bytes = (bitrate_kbps * 1024 / 8) * max_duration_secs as usize * 5 / 4;

        debug!(
            "Creating PacketBuffer: {}s @ {}fps, ~{} packets capacity, max {} MB ({}kbps * 1.25)",
            max_duration_secs,
            fps,
            estimated_packets,
            max_bytes / 1024 / 1024,
            bitrate_kbps,
        );

        Self {
            inner: RwLock::new(BufferInner {
                packets: VecDeque::with_capacity(estimated_packets),
                total_bytes: 0,
                max_bytes,
                sequence: 0,
            }),
            max_duration_secs,
            fps,
            gop_secs,
            codec_extradata: RwLock::new(None),
            audio_params: RwLock::new(None),
        }
    }

    /// Add a packet to the buffer
    ///
    /// This will evict old packets if the buffer is full
    /// or if packets are older than max_duration.
    pub fn push(&self, packet: Packet) {
        let mut inner = self.inner.write();

        let packet_size = packet.data.len();
        let sequence = inner.sequence;
        inner.sequence += 1;

        // Add packet with sequence number
        let mut packet = packet;
        packet.sequence = sequence;
        inner.packets.push_back(packet);
        inner.total_bytes += packet_size;

        trace!(
            "Added packet #{} ({} bytes), total: {} bytes, count: {}",
            sequence,
            packet_size,
            inner.total_bytes,
            inner.packets.len()
        );

        // Evict old packets if needed
        Self::evict_old_packets(&mut inner, self.max_duration_secs);
    }

    /// Get packets for the last N seconds
    ///
    /// Uses PTS-based retrieval with GOP compensation to ensure
    /// the writer has a keyframe to start from. Over-fetches by
    /// `gop_secs` worth of packets so `trim_to_keyframe()` can
    /// find a keyframe without shortening the clip.
    pub fn get_packets_for_duration(&self, duration_secs: u32) -> Vec<Packet> {
        let inner = self.inner.read();

        if inner.packets.is_empty() {
            return Vec::new();
        }

        // Use last packet's PTS as the reference point
        let last_pts = inner.packets.back().unwrap().pts;

        // Over-fetch by gop_secs so trim_to_keyframe doesn't shorten the clip.
        // Since encoder timebase is 1/fps, fps PTS units = 1 second.
        let fetch_secs = duration_secs as i64 + self.gop_secs as i64;
        let cutoff_pts = last_pts - fetch_secs * self.fps as i64;

        let packets: Vec<Packet> = inner
            .packets
            .iter()
            .skip_while(|p| p.pts < cutoff_pts)
            .cloned()
            .collect();

        debug!(
            "Retrieved {} packets for last {}s (overfetch {}s for GOP, cutoff_pts={}, total in buffer: {})",
            packets.len(),
            duration_secs,
            self.gop_secs,
            cutoff_pts,
            inner.packets.len()
        );

        packets
    }

    /// Get all packets in the buffer
    pub fn get_all_packets(&self) -> Vec<Packet> {
        let inner = self.inner.read();
        inner.packets.iter().cloned().collect()
    }

    /// Get current buffer statistics
    pub fn stats(&self) -> BufferStats {
        let inner = self.inner.read();
        // Derive duration from PTS range
        let duration_secs = if inner.packets.len() >= 2 {
            let first_pts = inner.packets.front().unwrap().pts;
            let last_pts = inner.packets.back().unwrap().pts;
            ((last_pts - first_pts) / self.fps as i64) as u32
        } else {
            0
        };
        BufferStats {
            packet_count: inner.packets.len(),
            total_bytes: inner.total_bytes,
            max_bytes: inner.max_bytes,
            duration_secs,
        }
    }

    /// Clear all packets from the buffer
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.packets.clear();
        inner.total_bytes = 0;
        inner.sequence = 0;
        debug!("Buffer cleared");
    }

    /// Store codec extradata (SPS/PPS) from the encoder
    pub fn set_codec_extradata(&self, data: Vec<u8>) {
        let mut extradata = self.codec_extradata.write();
        *extradata = Some(data);
    }

    /// Get codec extradata (SPS/PPS) for the writer
    pub fn get_codec_extradata(&self) -> Option<Vec<u8>> {
        let extradata = self.codec_extradata.read();
        extradata.clone()
    }

    /// Store audio stream parameters from the audio encoder
    pub fn set_audio_params(&self, params: AudioParams) {
        let mut ap = self.audio_params.write();
        *ap = Some(params);
    }

    /// Get audio stream parameters for the writer
    pub fn get_audio_params(&self) -> Option<AudioParams> {
        let ap = self.audio_params.read();
        ap.clone()
    }

    /// Evict packets that are too old or exceed size limit
    fn evict_old_packets(inner: &mut BufferInner, max_duration_secs: u32) {
        let now = Instant::now();
        let max_age = Duration::from_secs(max_duration_secs as u64);

        // Remove old packets based on time
        while let Some(front) = inner.packets.front() {
            if now.saturating_duration_since(front.timestamp) > max_age {
                let removed = inner.packets.pop_front().unwrap();
                inner.total_bytes -= removed.data.len();
            } else {
                break;
            }
        }

        // Remove old packets if we exceed size limit
        while inner.total_bytes > inner.max_bytes && inner.packets.len() > 1 {
            if let Some(removed) = inner.packets.pop_front() {
                inner.total_bytes -= removed.data.len();
            }
        }
    }
}

/// Buffer statistics
#[derive(Debug, Clone, Copy)]
pub struct BufferStats {
    pub packet_count: usize,
    pub total_bytes: usize,
    pub max_bytes: usize,
    pub duration_secs: u32,
}

impl BufferStats {
    /// Get buffer utilization as a percentage
    pub fn utilization_percent(&self) -> f64 {
        if self.max_bytes == 0 {
            0.0
        } else {
            (self.total_bytes as f64 / self.max_bytes as f64) * 100.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_packet(data: Vec<u8>, pts: i64) -> Packet {
        let mut packet = Packet::new(data, PacketType::Video, Instant::now());
        packet.pts = pts;
        packet.dts = pts;
        packet
    }

    #[test]
    fn test_push_and_get() {
        let buffer = PacketBuffer::new(60, 30, 8000, 2);

        // Add some packets
        for i in 0..10 {
            let packet = create_test_packet(vec![i as u8; 100], i);
            buffer.push(packet);
        }

        let stats = buffer.stats();
        assert_eq!(stats.packet_count, 10);
        assert_eq!(stats.total_bytes, 1000);
    }

    #[test]
    fn test_get_packets_for_duration() {
        let fps = 30;
        let buffer = PacketBuffer::new(60, fps, 8000, 2);

        // Add 5 seconds of packets (150 packets at 30fps)
        for i in 0..150i64 {
            let packet = create_test_packet(vec![0u8; 100], i);
            buffer.push(packet);
        }

        // Request last 3 seconds — with 2s GOP compensation, fetches 5s worth
        let packets = buffer.get_packets_for_duration(3);
        // cutoff_pts = 149 - (3+2)*30 = 149 - 150 = -1, so all 150 packets returned
        assert_eq!(packets.len(), 150);

        // Request last 1 second — with 2s GOP, fetches 3s worth
        let packets = buffer.get_packets_for_duration(1);
        // cutoff_pts = 149 - (1+2)*30 = 149 - 90 = 59, packets with pts >= 59
        assert_eq!(packets.len(), 91); // pts 59..=149
    }

    #[test]
    fn test_clear() {
        let buffer = PacketBuffer::new(60, 30, 8000, 2);

        for i in 0..5 {
            buffer.push(create_test_packet(vec![i as u8; 100], i));
        }

        buffer.clear();

        let stats = buffer.stats();
        assert_eq!(stats.packet_count, 0);
        assert_eq!(stats.total_bytes, 0);
    }

    #[test]
    fn test_gop_compensation_overfetch() {
        let fps = 60;
        let gop_secs = 2;
        let buffer = PacketBuffer::new(300, fps, 8000, gop_secs);

        // Add 10 seconds of packets (600 packets at 60fps)
        for i in 0..600i64 {
            let packet = create_test_packet(vec![0u8; 100], i);
            buffer.push(packet);
        }

        // Request 5 seconds — should over-fetch by gop_secs (2s), so 7s total
        let packets = buffer.get_packets_for_duration(5);
        // cutoff_pts = 599 - (5+2)*60 = 599 - 420 = 179
        // Packets with pts >= 179: pts 179..=599 = 421 packets
        assert_eq!(packets.len(), 421);

        // Verify the over-fetch: 421 packets at 60fps = 7 seconds worth
        let first_pts = packets.first().unwrap().pts;
        let last_pts = packets.last().unwrap().pts;
        let fetched_duration = (last_pts - first_pts) as f64 / fps as f64;
        assert!(fetched_duration >= 6.9 && fetched_duration <= 7.1);
    }

    #[test]
    fn test_stats_pts_based_duration() {
        let fps = 60;
        let buffer = PacketBuffer::new(300, fps, 8000, 2);

        // Add 5 seconds of packets
        for i in 0..(5 * fps as i64) {
            let packet = create_test_packet(vec![0u8; 100], i);
            buffer.push(packet);
        }

        let stats = buffer.stats();
        // (299 - 0) / 60 = 4 (integer division)
        assert_eq!(stats.duration_secs, 4);
    }

    #[test]
    fn test_bitrate_based_max_bytes() {
        // 8000 kbps * 1024 / 8 = 1_024_000 bytes/sec
        // * 60 seconds * 1.25 = 76_800_000
        let buffer = PacketBuffer::new(60, 30, 8000, 2);
        let stats = buffer.stats();
        assert_eq!(stats.max_bytes, 8000 * 1024 / 8 * 60 * 5 / 4);
    }
}
