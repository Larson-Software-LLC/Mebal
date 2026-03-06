# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Mebal is a replay buffer system written in Rust that continuously captures video into a circular buffer and saves the last N seconds to a file when triggered by a global hotkey. It is both a binary and a library crate.

## Build & Development Commands

```bash
cargo build                    # Debug build
cargo build --release          # Optimized build (LTO enabled, single codegen unit)
cargo test                     # Run all tests
cargo run                      # Run with default settings
cargo run -- --test            # Run with test pattern (no FFmpeg needed)
cargo run -- --list-sources    # List available capture sources
cargo run -- --list-encoders   # List available encoders
```

CLI flags override config values: `--hotkey`, `--buffer-duration`, `--save-duration`, `--output`.

## Architecture

Concurrent tasks run via `tokio::select!` in main:

1. **CaptureManager** (`src/capture/`) — Continuously generates encoded packets and pushes them into the shared buffer. Currently a stub producing test patterns; real FFmpeg capture gated behind the `ffmpeg` feature flag.
2. **HotkeyManager** (`src/hotkey/`) — Registers a non-exclusive global hotkey via the `livesplit-hotkey` crate (low-level keyboard hook; keypresses pass through to other apps). The hook fires on its own thread; the callback is passed to `HotkeyManager::new()`. No polling or message pump needed.
3. **Signal handler** — Listens for Ctrl+C to initiate clean shutdown.

Shared state flows through `Arc<AppState>` (defined in `main.rs`), which holds:
- `Arc<PacketBuffer>` — the circular buffer (internally uses `parking_lot::RwLock`)
- `Config` — validated settings
- `AtomicBool` — save-in-progress guard

### Data flow

```
CaptureManager → PacketBuffer ← HotkeyManager triggers save
                      ↓
                 VideoWriter → output file
```

### Key modules

| Module | Path | Purpose |
|--------|------|---------|
| `buffer` | `src/buffer/mod.rs`, `packet.rs` | Circular buffer storing H.264 `VideoPacket`s; time- and size-based eviction |
| `capture` | `src/capture/mod.rs`, `display.rs`, `encoder_setup.rs` | Capture management, display source abstraction (per-platform), encoder config |
| `hotkey` | `src/hotkey/mod.rs`, `parser.rs` | Non-exclusive global hotkey via `livesplit-hotkey`; parses `"Ctrl+Shift+F9"` style strings into `livesplit_hotkey::Hotkey` |
| `writer` | `src/writer/mod.rs` | Writes packets to MP4/MKV container files |
| `config` | `src/config.rs` | TOML-based config with platform-specific paths, CLI override support, validation |
| `error` | `src/error.rs` | `MebalError` enum and `MebalResult<T>` type alias |

### FFmpeg integration

FFmpeg support is behind the `ffmpeg` feature flag. The `ffmpeg-next` dependency is commented out in Cargo.toml—uncomment it and enable the feature to build with real capture/encoding. Without it, the capture and writer modules use stub implementations.

### Tauri GUI (`src-tauri/`)

The GUI uses the same `HotkeyManager` from the library crate (no Tauri global-shortcut plugin). The `TauriAppState` holds the `HotkeyManager` to keep the hook alive.

## Configuration

TOML config loaded from platform-specific paths:
- Linux: `~/.config/mebal/config.toml`
- Windows: `%APPDATA%\mebal\config.toml`
- macOS: `~/Library/Application Support/mebal/config.toml`

Key defaults: 300s buffer, 30s save duration, 8000 kbps bitrate, 60 fps, 1920x1080, hotkey `F9`.

## Concurrency model

- `parking_lot::RwLock` is used for the buffer's internal state as lock sections are brief and synchronous
- Save operations are guarded by an `AtomicBool` to prevent concurrent saves
- Capture, hotkey, and signal tasks are joined with `tokio::select!` for clean shutdown
