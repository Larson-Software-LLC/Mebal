// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! Thread-safe circular buffer for encoded media packets
//!
//! Stores H.264 video and AAC audio packets in chronological order,
//! evicting by wall-clock age and byte-size limits. Retrieval uses
//! PTS-based windowing with GOP compensation for keyframe alignment.

use parking_lot::RwLock;
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, trace};

mod packet;

pub use packet::{Packet, PacketType};

#[derive(Debug, Clone)]
pub struct AudioParams {
    pub sample_rate: u32,
    pub channels: u16,
    pub frame_size: u32,
    pub extradata: Vec<u8>,
}

pub struct PacketBuffer {
    inner: RwLock<BufferInner>,
    max_duration_secs: AtomicU32,
    fps: AtomicU32,
    gop_secs: AtomicU32,
    /// SPS/PPS — set once by encoder, read by writer
    codec_extradata: OnceLock<Vec<u8>>,
    /// Set once by audio capture, read by writer
    audio_params: OnceLock<AudioParams>,
}

struct BufferInner {
    packets: VecDeque<Packet>,
    total_bytes: usize,
    max_bytes: usize,
}

/// VBR headroom: 1.25x
const VBR_HEADROOM_NUM: usize = 5;
const VBR_HEADROOM_DEN: usize = 4;

const fn max_bytes_for(bitrate_kbps: usize, duration_secs: u32) -> usize {
    (bitrate_kbps * 1024 / 8) * duration_secs as usize * VBR_HEADROOM_NUM / VBR_HEADROOM_DEN
}

impl PacketBuffer {
    pub fn new(max_duration_secs: u32, fps: u32, bitrate_kbps: usize, gop_secs: u32) -> Self {
        let estimated_packets = (fps * max_duration_secs) as usize;
        let max_bytes = max_bytes_for(bitrate_kbps, max_duration_secs);

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
            }),
            max_duration_secs: AtomicU32::new(max_duration_secs),
            fps: AtomicU32::new(fps),
            gop_secs: AtomicU32::new(gop_secs),
            codec_extradata: OnceLock::new(),
            audio_params: OnceLock::new(),
        }
    }

    pub fn push(&self, packet: Packet) {
        let mut inner = self.inner.write();

        let packet_size = packet.data.len();
        inner.total_bytes += packet_size;
        inner.packets.push_back(packet);

        trace!(
            "Added packet ({} bytes), total: {} bytes, count: {}",
            packet_size,
            inner.total_bytes,
            inner.packets.len()
        );

        Self::evict_old_packets(&mut inner, self.max_duration_secs.load(Ordering::Relaxed));
    }

    /// Returns packets for the last `duration_secs`, plus GOP overfetch for keyframe alignment.
    pub fn get_packets_for_duration(&self, duration_secs: u32) -> Vec<Packet> {
        let inner = self.inner.read();

        if inner.packets.is_empty() {
            return Vec::new();
        }

        let last_pts = inner.packets.back().unwrap().pts;
        let gop_secs = self.gop_secs.load(Ordering::Relaxed) as i64;
        let fps = self.fps.load(Ordering::Relaxed) as i64;
        let fetch_secs = duration_secs as i64 + gop_secs;
        let cutoff_pts = last_pts - fetch_secs * fps;

        let start_idx = inner.packets.partition_point(|p| p.pts < cutoff_pts);
        let count = inner.packets.len() - start_idx;

        let mut packets = Vec::with_capacity(count);
        for i in start_idx..inner.packets.len() {
            packets.push(inner.packets[i].clone());
        }

        debug!(
            "Retrieved {} packets for last {}s (overfetch {}s for GOP, cutoff_pts={}, total in buffer: {})",
            packets.len(),
            duration_secs,
            gop_secs,
            cutoff_pts,
            inner.packets.len()
        );

        packets
    }

    pub fn stats(&self) -> BufferStats {
        let inner = self.inner.read();
        let fps = self.fps.load(Ordering::Relaxed);
        let duration_secs = if inner.packets.len() >= 2 {
            let first_pts = inner.packets.front().unwrap().pts;
            let last_pts = inner.packets.back().unwrap().pts;
            ((last_pts - first_pts) / fps as i64) as u32
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

    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.packets.clear();
        inner.total_bytes = 0;
        debug!("Buffer cleared");
    }

    pub fn reconfigure(
        &self,
        max_duration_secs: u32,
        fps: u32,
        bitrate_kbps: usize,
        gop_secs: u32,
    ) {
        self.max_duration_secs
            .store(max_duration_secs, Ordering::Relaxed);
        self.fps.store(fps, Ordering::Relaxed);
        self.gop_secs.store(gop_secs, Ordering::Relaxed);
        let max_bytes = max_bytes_for(bitrate_kbps, max_duration_secs);
        self.inner.write().max_bytes = max_bytes;
        debug!(
            "Buffer reconfigured: {}s @ {}fps, max {} MB",
            max_duration_secs,
            fps,
            max_bytes / 1024 / 1024,
        );
    }

    pub fn set_codec_extradata(&self, data: Vec<u8>) {
        let _ = self.codec_extradata.set(data);
    }

    pub fn get_codec_extradata(&self) -> Option<Vec<u8>> {
        self.codec_extradata.get().cloned()
    }

    pub fn set_audio_params(&self, params: AudioParams) {
        let _ = self.audio_params.set(params);
    }

    pub fn get_audio_params(&self) -> Option<AudioParams> {
        self.audio_params.get().cloned()
    }

    fn evict_old_packets(inner: &mut BufferInner, max_duration_secs: u32) {
        let now = Instant::now();
        let max_age = Duration::from_secs(max_duration_secs as u64);

        while let Some(front) = inner.packets.front() {
            if now.saturating_duration_since(front.timestamp) > max_age {
                let removed = inner.packets.pop_front().unwrap();
                inner.total_bytes -= removed.data.len();
            } else {
                break;
            }
        }

        while inner.total_bytes > inner.max_bytes && inner.packets.len() > 1 {
            if let Some(removed) = inner.packets.pop_front() {
                inner.total_bytes -= removed.data.len();
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BufferStats {
    pub packet_count: usize,
    pub total_bytes: usize,
    pub max_bytes: usize,
    pub duration_secs: u32,
}

impl BufferStats {
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
        let buffer = PacketBuffer::new(60, 30, 8000, 2);
        let stats = buffer.stats();
        assert_eq!(stats.max_bytes, max_bytes_for(8000, 60));
    }
}
