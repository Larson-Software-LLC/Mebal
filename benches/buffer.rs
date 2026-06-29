use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use mebal::buffer::{Packet, PacketBuffer, PacketType};
use std::time::Instant;

/// Realistic packet size distribution: mostly P-frames (~4 KB) with
/// periodic keyframes (~16 KB), matching typical H.264 @ 8 Mbps / 60 fps.
fn make_packet(pts: i64, is_keyframe: bool) -> Packet {
    let size = if is_keyframe { 16_384 } else { 4_096 };
    let mut p = Packet::new(
        Bytes::from(vec![0xABu8; size]),
        PacketType::Video,
        Instant::now(),
    );
    p.pts = pts;
    p.dts = pts;
    p.duration = 1;
    p.is_keyframe = is_keyframe;
    p
}

/// Fill a buffer with `seconds` of 60 fps video (keyframe every 120 frames = 2s GOP).
fn fill_buffer(buffer: &PacketBuffer, seconds: u32) {
    let fps = 60u32;
    let gop_frames = 120u32;
    let total = (seconds * fps) as i64;
    for i in 0..total {
        buffer.push(make_packet(i, (i as u32) % gop_frames == 0));
    }
}

// ---------------------------------------------------------------------------
// push throughput
// ---------------------------------------------------------------------------

fn bench_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("push");

    // Pre-build packets so allocation isn't measured
    let packets: Vec<Packet> = (0..600)
        .map(|i| make_packet(i, (i as u32) % 120 == 0))
        .collect();

    group.bench_function("60fps_10s", |b| {
        b.iter_with_setup(
            || (PacketBuffer::new(300, 60, 8000, 2), packets.clone()),
            |(buffer, pkts)| {
                for p in pkts {
                    buffer.push(p);
                }
            },
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// get_packets_for_duration (the save/clone path)
// ---------------------------------------------------------------------------

fn bench_get_packets(c: &mut Criterion) {
    let mut group = c.benchmark_group("get_packets_for_duration");

    for &buffer_secs in &[60u32, 300] {
        for &save_secs in &[5u32, 30] {
            let label = format!("buf{}s_save{}s", buffer_secs, save_secs);

            // Pre-fill once; the benchmark just measures retrieval.
            let buffer = PacketBuffer::new(buffer_secs, 60, 8000, 2);
            fill_buffer(&buffer, buffer_secs);

            group.bench_with_input(
                BenchmarkId::new("retrieve", &label),
                &save_secs,
                |b, &secs| {
                    b.iter(|| {
                        black_box(buffer.get_packets_for_duration(secs));
                    });
                },
            );
        }
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// trim_to_keyframe
// ---------------------------------------------------------------------------

fn bench_trim_to_keyframe(c: &mut Criterion) {
    use mebal::writer::trim_to_keyframe;

    let mut group = c.benchmark_group("trim_to_keyframe");

    // Build a realistic packet slice: 30s @ 60fps = 1800 packets,
    // keyframe every 120 frames. First keyframe at index 0.
    let packets: Vec<Packet> = (0..1800)
        .map(|i| make_packet(i, (i as u32) % 120 == 0))
        .collect();

    group.bench_function("1800_packets", |b| {
        b.iter_with_setup(
            || packets.clone(),
            |pkts| {
                black_box(trim_to_keyframe(pkts));
            },
        );
    });

    // Worst case: keyframe only at the very end
    let worst: Vec<Packet> = (0..1800)
        .map(|i| make_packet(i, i == 1799))
        .collect();

    group.bench_function("1800_packets_late_keyframe", |b| {
        b.iter_with_setup(
            || worst.clone(),
            |pkts| {
                black_box(trim_to_keyframe(pkts));
            },
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Contended access: push while reading (simulates capture + save overlap)
// ---------------------------------------------------------------------------

fn bench_contended(c: &mut Criterion) {
    use std::sync::Arc;

    let mut group = c.benchmark_group("contended_save");

    // 300s buffer, measure get_packets_for_duration while another thread pushes
    group.bench_function("push_during_read_300s", |b| {
        let buffer = Arc::new(PacketBuffer::new(300, 60, 8000, 2));
        fill_buffer(&buffer, 300);

        // Spawn a background pusher that continuously writes
        let push_buf = Arc::clone(&buffer);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = Arc::clone(&stop);

        let pusher = std::thread::spawn(move || {
            let mut pts = 300 * 60; // continue after fill
            while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
                push_buf.push(make_packet(pts as i64, (pts % 120) == 0));
                pts += 1;
            }
        });

        b.iter(|| {
            black_box(buffer.get_packets_for_duration(30));
        });

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        pusher.join().unwrap();
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_push,
    bench_get_packets,
    bench_trim_to_keyframe,
    bench_contended,
);
criterion_main!(benches);
