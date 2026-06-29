//! Integration tests for the mebal replay buffer pipeline.
//!
//! These tests exercise the buffer → writer flow using synthetic packets
//! and (optionally) a real MP4 fixture extracted via FFmpeg's demuxer.

use bytes::Bytes;
use mebal::buffer::{Packet, PacketBuffer, PacketType};
use mebal::config::{Config, GOP_INTERVAL_SECS};
use mebal::writer::{VideoWriter, find_first_keyframe, trim_to_keyframe};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a synthetic video packet.
fn video_packet(pts: i64, is_keyframe: bool, size: usize) -> Packet {
    Packet {
        data: Bytes::from(vec![0xABu8; size]),
        packet_type: PacketType::Video,
        timestamp: Instant::now(),
        pts,
        dts: pts,
        duration: 1,
        is_keyframe,
        stream_index: 0,
    }
}

/// Build a synthetic audio packet.
fn audio_packet(pts: i64, size: usize) -> Packet {
    Packet {
        data: Bytes::from(vec![0xCDu8; size]),
        packet_type: PacketType::Audio,
        timestamp: Instant::now(),
        pts,
        dts: pts,
        duration: 1024,
        is_keyframe: true,
        stream_index: 1,
    }
}

/// Populate a buffer with `seconds` of synthetic 60 fps video, placing a
/// keyframe every `gop_frames` frames, interleaved with audio packets.
fn fill_buffer(
    buffer: &PacketBuffer,
    seconds: u32,
    fps: u32,
    gop_frames: u32,
    include_audio: bool,
) {
    let total_frames = (seconds * fps) as i64;
    for i in 0..total_frames {
        let is_key = (i as u32) % gop_frames == 0;
        buffer.push(video_packet(i, is_key, 4096));

        if include_audio && i % 2 == 0 {
            // ~30 audio packets per second (rough AAC cadence at 60 fps)
            buffer.push(audio_packet(i, 256));
        }
    }
}

fn test_config() -> Config {
    Config {
        buffer_duration_secs: 60,
        save_duration_secs: 5,
        bitrate_kbps: 8000,
        fps: 60,
        output_directory: std::env::temp_dir()
            .join("mebal_test")
            .to_string_lossy()
            .to_string(),
        output_prefix: "test".to_string(),
        hotkey: "F9".to_string(),
        resolution: (1920, 1080),
        capture_source: None,
        encoder: None,
        audio_enabled: false,
        audio_bitrate_kbps: 192,
    }
}

// ---------------------------------------------------------------------------
// Buffer → trim pipeline
// ---------------------------------------------------------------------------

#[test]
fn buffer_to_trim_pipeline_produces_keyframe_aligned_output() {
    let config = test_config();
    let buffer = PacketBuffer::new(
        config.buffer_duration_secs,
        config.fps,
        config.total_bitrate_kbps(),
        GOP_INTERVAL_SECS,
    );

    // 10 seconds of video at 60 fps, keyframe every 60 frames (1 s)
    fill_buffer(&buffer, 10, 60, 60, false);

    let packets = buffer.get_packets_for_duration(config.save_duration_secs);
    assert!(!packets.is_empty(), "should retrieve packets from buffer");

    let trimmed = trim_to_keyframe(packets);
    assert!(!trimmed.is_empty(), "should find a keyframe to trim to");
    assert!(
        trimmed[0].is_keyframe,
        "first packet after trim must be a keyframe"
    );
    assert_eq!(
        trimmed[0].packet_type,
        PacketType::Video,
        "first keyframe must be a video packet"
    );
}

#[test]
fn trim_returns_empty_when_no_keyframes_present() {
    let packets: Vec<Arc<Packet>> = (0..100)
        .map(|i| Arc::new(video_packet(i, false, 128)))
        .collect();

    let trimmed = trim_to_keyframe(packets);
    assert!(trimmed.is_empty(), "no keyframes → empty result");
}

#[test]
fn find_first_keyframe_skips_audio_keyframes() {
    let packets = vec![
        Arc::new(audio_packet(0, 64)), // audio is_keyframe=true, should be ignored
        Arc::new(video_packet(1, false, 128)),
        Arc::new(video_packet(2, true, 128)), // first *video* keyframe
        Arc::new(video_packet(3, false, 128)),
    ];

    assert_eq!(find_first_keyframe(&packets), Some(2));
}

// ---------------------------------------------------------------------------
// Buffer interleaved audio + video
// ---------------------------------------------------------------------------

#[test]
fn buffer_preserves_packet_ordering_with_interleaved_streams() {
    let buffer = PacketBuffer::new(60, 60, 8000, 1);
    fill_buffer(&buffer, 3, 60, 60, true);

    let packets = buffer.get_packets_for_duration(60);

    // Verify insertion order is preserved (PTS non-decreasing)
    for window in packets.windows(2) {
        assert!(
            window[1].pts >= window[0].pts,
            "packets must stay in insertion order"
        );
    }

    // Verify we have both audio and video
    let has_video = packets.iter().any(|p| p.packet_type == PacketType::Video);
    let has_audio = packets.iter().any(|p| p.packet_type == PacketType::Audio);
    assert!(has_video, "must have video packets");
    assert!(has_audio, "must have audio packets");
}

// ---------------------------------------------------------------------------
// Buffer eviction
// ---------------------------------------------------------------------------

#[test]
fn buffer_evicts_by_size_limit() {
    // Tiny budget: 100 KB at 1000 kbps for 1 second
    let buffer = PacketBuffer::new(300, 60, 1000, 1);

    // Push way more data than the budget allows (~2 MB)
    for i in 0..500i64 {
        buffer.push(video_packet(i, i % 60 == 0, 4096));
    }

    let stats = buffer.stats();
    assert!(
        stats.total_bytes <= stats.max_bytes,
        "total_bytes ({}) must not exceed max_bytes ({})",
        stats.total_bytes,
        stats.max_bytes,
    );
}

#[test]
fn config_deserializes_with_missing_fields() {
    // Minimal TOML — every field should fall back to its serde default
    let config: Config = toml::from_str("").expect("empty TOML should use defaults");
    assert_eq!(config.buffer_duration_secs, 300);
    assert_eq!(config.save_duration_secs, 30);
    assert_eq!(config.fps, 60);
    assert!(config.audio_enabled);
}

#[test]
fn config_validation_rejects_zero_fps() {
    let mut config = Config::default();
    config.fps = 0;
    assert!(config.validate().is_err());
}

#[test]
fn config_validation_rejects_save_exceeding_buffer() {
    let mut config = Config::default();
    config.save_duration_secs = config.buffer_duration_secs; // equal → fails (needs room for GOP)
    assert!(config.validate().is_err());
}

// ---------------------------------------------------------------------------
// AppState save guard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn save_guard_prevents_concurrent_saves() {
    let config = test_config();
    let app = mebal::AppState::new(config);

    // Fill buffer so save_replay has packets to work with
    fill_buffer(&app.packet_buffer, 10, 60, 60, false);

    assert!(!app.is_saving(), "should not be saving initially");

    // First save should proceed (it will fail at the writer since we have no
    // real codec extradata, but the guard behaviour is what we're testing)
    let s1 = app.clone();
    let h1 = tokio::spawn(async move { s1.save_replay().await });

    // Give the first save a moment to set the guard
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // While the first is in progress, a second call should be a no-op
    let s2 = app.clone();
    let result = s2.save_replay().await;
    assert!(result.is_ok(), "concurrent save should succeed (no-op)");

    // Wait for first save to finish
    let _ = h1.await;

    // Guard should be released
    assert!(!app.is_saving(), "guard should be released after save");
}

// ---------------------------------------------------------------------------
// Buffer reconfiguration
// ---------------------------------------------------------------------------

#[test]
fn buffer_reconfigure_updates_parameters() {
    let buffer = PacketBuffer::new(60, 30, 4000, 1);

    // Fill with some data
    for i in 0..100i64 {
        buffer.push(video_packet(i, i % 30 == 0, 1024));
    }

    let old_stats = buffer.stats();

    // Reconfigure to a higher bitrate → larger max_bytes
    buffer.reconfigure(120, 60, 16000, 2);

    let new_stats = buffer.stats();
    assert!(
        new_stats.max_bytes > old_stats.max_bytes,
        "max_bytes should increase with higher bitrate and duration"
    );
}

// ---------------------------------------------------------------------------
// Writer with real MP4 fixture (skipped if fixture not present)
// ---------------------------------------------------------------------------

/// If a test fixture MP4 exists at `tests/fixtures/sample.mp4`, demux it,
/// push the packets through the buffer, and write them back to a new file.
///
/// This verifies the full demux → buffer → writer round-trip.
///
/// The fixture may use any codec (H.264, HEVC, etc.) and frame rate.
/// We detect the actual fps from the stream and assign sequential
/// frame-number DTS/PTS to avoid precision loss from timebase rescaling
/// (the live capture pipeline always produces sequential DTS anyway).
#[test]
fn writer_round_trip_with_fixture() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample.mp4");

    if !fixture.exists() {
        eprintln!(
            "Skipping writer_round_trip_with_fixture: no fixture at {:?}",
            fixture
        );
        return;
    }

    mebal::init_ffmpeg();

    // Demux the fixture
    let mut input_ctx = ffmpeg_next::format::input(&fixture).expect("open fixture MP4");

    let video_stream_idx = input_ctx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .expect("fixture must have a video stream")
        .index();

    // Detect actual fps from the stream
    let stream_rate = input_ctx.stream(video_stream_idx).unwrap().rate();
    let fps = (stream_rate.numerator() as u32)
        .checked_div(stream_rate.denominator() as u32)
        .unwrap_or(30)
        .max(1);

    // Collect extradata, resolution, and codec ID from the fixture
    let (extradata, width, height, codec_id) = {
        let stream = input_ctx.stream(video_stream_idx).unwrap();
        let params = stream.parameters();
        unsafe {
            let codecpar = params.as_ptr();
            let extra = if (*codecpar).extradata.is_null() || (*codecpar).extradata_size <= 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(
                    (*codecpar).extradata,
                    (*codecpar).extradata_size as usize,
                )
                .to_vec()
            };
            (
                extra,
                (*codecpar).width as u32,
                (*codecpar).height as u32,
                (*codecpar).codec_id,
            )
        }
    };

    // Demux video packets.  Assign sequential frame-number DTS (0, 1, 2, …)
    // instead of rescaling the fixture's timestamps.  This mirrors the live
    // capture pipeline where the encoder produces DTS = frame_index.
    // We sort by original DTS first to get decode order.
    let mut raw_packets: Vec<(i64, bool, Vec<u8>)> = Vec::new();

    for (stream, pkt) in input_ctx.packets() {
        if stream.index() != video_stream_idx {
            continue;
        }
        raw_packets.push((
            pkt.dts().unwrap_or(0),
            pkt.is_key(),
            pkt.data().unwrap_or(&[]).to_vec(),
        ));
    }

    // Sort by original DTS to ensure decode order
    raw_packets.sort_by_key(|(dts, _, _)| *dts);

    let mut packets = Vec::with_capacity(raw_packets.len());
    for (i, (_orig_dts, is_key, data)) in raw_packets.into_iter().enumerate() {
        let mut p = Packet::new(Bytes::from(data), PacketType::Video, Instant::now());
        p.pts = i as i64;
        p.dts = i as i64;
        p.duration = 1;
        p.is_keyframe = is_key;
        packets.push(p);
    }

    assert!(!packets.is_empty(), "fixture must contain video packets");

    // Push through a buffer
    let buffer = PacketBuffer::new(300, fps, 8000, GOP_INTERVAL_SECS);
    buffer.set_codec_extradata(extradata.clone());

    for p in &packets {
        buffer.push(p.clone());
    }

    let retrieved = buffer.get_packets_for_duration(30);
    assert!(!retrieved.is_empty());

    // Write to a temp file
    let output = std::env::temp_dir().join("mebal_test_roundtrip.mp4");

    let config = Config {
        resolution: (width, height),
        fps,
        bitrate_kbps: 8000,
        audio_enabled: false,
        ..test_config()
    };
    let writer = VideoWriter::new(&config, extradata, None, codec_id);
    let result = writer.write_packets_blocking(retrieved, &output);
    assert!(result.is_ok(), "writer should succeed: {:?}", result.err());

    // Verify the output file is a valid MP4
    let out_ctx = ffmpeg_next::format::input(&output).expect("open output MP4");
    let has_video = out_ctx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .is_some();
    assert!(has_video, "output MP4 must have a video stream");

    // Cleanup
    let _ = std::fs::remove_file(&output);
}
