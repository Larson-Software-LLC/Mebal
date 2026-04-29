# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Mebal is a replay buffer system written in Rust that continuously captures the Windows desktop (video + system audio) into a circular buffer and saves the last N seconds to an MP4 when triggered by a global hotkey. It ships as a binary, a library crate, and a Tauri GUI.

## Build & Development Commands

```bash
cargo build                    # Debug build
cargo build --release          # Optimized build (LTO, single codegen unit)
cargo test                     # Run all tests
cargo bench                    # Run criterion benchmarks (buffer)
cargo run                      # Run CLI with default settings
cargo run -- --list-encoders   # List FFmpeg encoders and availability
cargo run -- --no-audio        # Disable audio capture
```

CLI flags override config values: `--hotkey`, `--buffer-duration`, `--save-duration`, `--output`, `--config`, `--no-audio`.

The Tauri GUI lives in `src-tauri/`; run it with `cargo tauri dev` from that directory (UI assets are served from `ui/` via Vite).

## Architecture

`main.rs` spawns three blocking workers (video capture, audio capture, hotkey hook) and joins them with `tokio::select!` against `Ctrl+C`:

1. **CaptureManager** (`src/capture/mod.rs`) — DXGI Desktop Duplication → BGRA→NV12 scaling (sws) → H.264 encode (NVENC, libx264 fallback). Reconnects on DXGI errors up to 20 times. Pushes encoded packets into the shared buffer.
2. **AudioCaptureManager** (`src/capture/audio.rs`) — WASAPI loopback via `cpal` → AAC encode (FFmpeg) → shared buffer. Failure is non-fatal; capture continues video-only.
3. **HotkeyManager** (`src/hotkey/`) — Non-exclusive global hotkey via `livesplit-hotkey` (low-level keyboard hook on its own thread). Callback runs save asynchronously through the app's `TaskTracker` so Ctrl+C drains in-flight saves before exit.

Shared state flows through `App` (a `Clone` newtype over `Arc<AppState>`, defined in `src/app.rs`):
- `Arc<PacketBuffer>` — circular buffer holding both video and audio packets
- `ArcSwap<Config>` — hot-swappable config (GUI updates it at runtime)
- `AtomicBool` saving guard to prevent overlapping saves
- `TaskTracker` for graceful shutdown of in-flight save tasks

### Data flow

```
DXGI capture ─┐                  ┌── HotkeyManager → App::save_replay
              ├─→ PacketBuffer ──┤
WASAPI loop ──┘                  └── (status poll, GUI events)
                                          ↓
                                     VideoWriter → MP4
```

### Key modules

| Module | Path | Purpose |
|--------|------|---------|
| `app` | `src/app.rs` | `App`/`AppState`: shared buffer, hot-swappable config, save coordination, `TaskTracker` |
| `buffer` | `src/buffer/mod.rs`, `packet.rs` | Mixed-stream circular buffer (H.264 + AAC) with age + byte eviction; PTS-windowed retrieval with GOP overfetch; stores codec extradata + audio params |
| `capture` | `src/capture/mod.rs`, `audio.rs`, `dxgi.rs`, `encoder_setup.rs` | DXGI video capture, WASAPI audio capture, NVENC/libx264 encoder selection |
| `hotkey` | `src/hotkey/mod.rs`, `parser.rs` | Non-exclusive global hotkey via `livesplit-hotkey`; parses `"Ctrl+Shift+F9"` strings |
| `writer` | `src/writer/mod.rs` | Muxes pre-encoded video + audio packets into MP4; trims to first video keyframe; rebases PTS/DTS per stream; drops non-monotonic packets |
| `config` | `src/config.rs` | TOML config with platform paths, CLI overrides, validation; defines `GOP_INTERVAL_SECS = 1` |
| `error` | `src/error.rs` | `MebalError` enum and `MebalResult<T>` |

### FFmpeg integration

FFmpeg is a **required** dependency (`ffmpeg-next` + `ffmpeg-sys-next` with the `static` feature). There is no feature flag — capture and writer use FFmpeg unconditionally. `build.rs` handles native link config; the writer uses raw `ffmpeg-sys-next` FFI to mux pre-encoded packets without re-encoding.

### Tauri GUI (`src-tauri/`)

`mebal-gui` (workspace member) wraps the library crate:
- `state::TauriAppState` holds `mebal::App`, the cancel token, capture worker thread handles, and the `HotkeyManager` (kept alive for hook duration).
- `commands.rs` — invoke handlers: `get_config`, `set_config`, `save_replay`, `get_status`, `start_capture`, `stop_capture`, `get_encoder_info`.
- `tray.rs` — system tray + hotkey registration on startup.
- `lib.rs` auto-starts capture on launch and runs a 1Hz `buffer-status` event poll for the UI. Closing the settings window hides it instead of exiting (`prevent_close` + `prevent_exit`).

Frontend lives in `ui/` (Vite + TypeScript). Build output goes to `ui/dist`.

## Configuration

TOML config loaded from platform-specific paths via `dirs::config_dir()`:
- Linux: `~/.config/mebal/config.toml`
- Windows: `%APPDATA%\mebal\config.toml`
- macOS: `~/Library/Application Support/mebal/config.toml`

Output recordings default to `dirs::video_dir()/mebal/`.

Key defaults: 300s buffer, 30s save duration, 8000 kbps video, 192 kbps audio (enabled), 60 fps, 2560×1440, hotkey `F9`. Validation requires `save_duration + GOP_INTERVAL_SECS ≤ buffer_duration`.

## Concurrency model

- `parking_lot::RwLock` guards the `PacketBuffer` interior; lock sections are short and synchronous.
- `arc-swap::ArcSwap<Config>` lets the GUI swap config without locking readers; `App::update_config` returns whether a capture restart is needed (resolution, fps, encoder, bitrate, capture source, audio toggle, or buffer duration changed).
- Save operations are guarded by an `AtomicBool` — concurrent triggers are dropped with a warning.
- Capture and audio workers are `std::thread::spawn` (GUI) or `tokio::task::spawn_blocking` (CLI), both driven by a `tokio_util::sync::CancellationToken`.
- `TaskTracker` ensures Ctrl+C waits for in-flight saves before the process exits.
- DXGI capture polls non-blocking with 0ms timeout; on `Ok(false)` (desktop unchanged) it re-uses the previous frame to maintain target FPS. Packet PTS is wall-clock-based and monotonically enforced.
