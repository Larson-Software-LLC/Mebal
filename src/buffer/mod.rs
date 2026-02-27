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

/// A thread-safe circular buffer for video packets
pub struct PacketBuffer {
    /// Internal storage for packets
    inner: RwLock<BufferInner>,
    /// Maximum duration to keep in buffer (in seconds)
    max_duration_secs: u32,
    /// Expected frames per second (for capacity estimation)
    fps: u32,
    /// Codec extradata (SPS/PPS) needed by the writer to produce valid MP4
    codec_extradata: RwLock<Option<Vec<u8>>>,
}

/// Internal buffer state
struct BufferInner {
    /// The packet queue
    packets: VecDeque<Packet>,
    /// Total bytes currently in buffer
    total_bytes: usize,
    /// Maximum bytes before eviction
    max_bytes: usize,
    /// Start time of the buffer
    start_time: Option<Instant>,
    /// Sequence number for ordering
    sequence: u64,
}

impl PacketBuffer {
    /// Create a new packet buffer
    ///
    /// # Arguments
    /// * `max_duration_secs` - Maximum duration to keep in buffer
    /// * `fps` - Expected frames per second
    pub fn new(max_duration_secs: u32, fps: u32) -> Self {
        // Estimate capacity: fps * duration * average_packet_size
        // Average H.264 packet at 8Mbps 60fps = ~16KB per frame
        let estimated_packets = (fps * max_duration_secs) as usize;
        let max_bytes = estimated_packets * 16 * 1024; // 16KB average

        debug!(
            "Creating PacketBuffer: {}s @ {}fps, ~{} packets capacity",
            max_duration_secs, fps, estimated_packets
        );

        Self {
            inner: RwLock::new(BufferInner {
                packets: VecDeque::with_capacity(estimated_packets),
                total_bytes: 0,
                max_bytes,
                start_time: None,
                sequence: 0,
            }),
            max_duration_secs,
            fps,
            codec_extradata: RwLock::new(None),
        }
    }

    /// Add a packet to the buffer
    ///
    /// This will evict old packets if the buffer is full
    /// or if packets are older than max_duration.
    pub fn push(&self, packet: Packet) {
        let mut inner = self.inner.write();

        // Set start time if this is the first packet
        if inner.start_time.is_none() {
            inner.start_time = Some(Instant::now());
        }

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
    /// Returns packets in chronological order
    pub fn get_packets_for_duration(&self, duration_secs: u32) -> Vec<Packet> {
        let inner = self.inner.read();

        if inner.packets.is_empty() {
            return Vec::new();
        }

        // Find the cutoff time
        let now = Instant::now();
        let cutoff = now.checked_sub(Duration::from_secs(duration_secs as u64));

        // Find the first packet after cutoff
        let packets: Vec<Packet> = inner
            .packets
            .iter()
            .skip_while(|p| cutoff.is_some_and(|c| p.timestamp < c))
            .cloned()
            .collect();

        debug!(
            "Retrieved {} packets for last {}s (total in buffer: {})",
            packets.len(),
            duration_secs,
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
        BufferStats {
            packet_count: inner.packets.len(),
            total_bytes: inner.total_bytes,
            max_bytes: inner.max_bytes,
            duration_secs: inner
                .start_time
                .map(|t| t.elapsed().as_secs() as u32)
                .unwrap_or(0),
        }
    }

    /// Clear all packets from the buffer
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.packets.clear();
        inner.total_bytes = 0;
        inner.start_time = None;
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

    fn create_test_packet(data: Vec<u8>) -> Packet {
        Packet::new(data, PacketType::Video, Instant::now())
    }

    #[test]
    fn test_push_and_get() {
        let buffer = PacketBuffer::new(60, 30);

        // Add some packets
        for i in 0..10 {
            let packet = create_test_packet(vec![i as u8; 100]);
            buffer.push(packet);
        }

        let stats = buffer.stats();
        assert_eq!(stats.packet_count, 10);
        assert_eq!(stats.total_bytes, 1000);
    }

    #[test]
    fn test_get_packets_for_duration() {
        let buffer = PacketBuffer::new(60, 30);

        // Add packets
        for i in 0..5 {
            let packet = create_test_packet(vec![i as u8; 100]);
            buffer.push(packet);
        }

        let packets = buffer.get_packets_for_duration(30);
        assert_eq!(packets.len(), 5);
    }

    #[test]
    fn test_clear() {
        let buffer = PacketBuffer::new(60, 30);

        for i in 0..5 {
            buffer.push(create_test_packet(vec![i as u8; 100]));
        }

        buffer.clear();

        let stats = buffer.stats();
        assert_eq!(stats.packet_count, 0);
        assert_eq!(stats.total_bytes, 0);
    }
}
