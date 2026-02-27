# Mebal - Replay Buffer

A high-performance replay buffer for continuous video recording, written in Rust. Mebal continuously records video into a circular buffer and saves the last N seconds when triggered by a hotkey.

## Features

- 🎮 **Instant Replay**: Always recording, save moments after they happen
- 🚀 **High Performance**: Stores encoded H.264 packets (not raw frames) for efficiency
- 🔥 **Hardware Acceleration**: Supports NVIDIA NVENC, Intel QuickSync, VAAPI, and more
- ⌨️ **Global Hotkeys**: Trigger saves from anywhere with customizable hotkeys
- 🖥️ **Cross-Platform**: Linux (X11/KMS), Windows, macOS support
- ⚙️ **Configurable**: TOML-based configuration with CLI overrides
- 📹 **Multiple Formats**: MP4, MKV, MOV output support

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         Mebal                                   │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐    ┌──────────────┐    ┌──────────────┐       │
│  │   Capture   │───▶│Packet Buffer │◀───│   Hotkey     │       │
│  │  (FFmpeg)   │    │  (Circular)  │    │   Handler    │       │
│  └─────────────┘    └──────────────┘    └──────────────┘       │
│                            │                                    │
│                            ▼                                    │
│                     ┌──────────────┐                           │
│                     │Video Writer  │                           │
│                     │  (MP4/MKV)   │                           │
│                     └──────────────┘                           │
└─────────────────────────────────────────────────────────────────┘
```

## Installation

### Prerequisites

- Rust 1.70+ 
- FFmpeg development libraries
- Platform-specific capture dependencies

#### Linux
```bash
# Ubuntu/Debian
sudo apt-get install libavcodec-dev libavformat-dev libavutil-dev libswscale-dev

# For X11 capture
sudo apt-get install libx11-dev

# Fedora
sudo dnf install ffmpeg-devel libX11-devel
```

#### Windows
Install FFmpeg via vcpkg or download prebuilt libraries.

#### macOS
```bash
brew install ffmpeg
```

### Build from Source

```bash
git clone https://github.com/yourusername/mebal
cd mebal
cargo build --release
```

The binary will be at `target/release/mebal`.

## Usage

### Basic Usage

```bash
# Run with default settings
mebal

# Use custom hotkey
mebal --hotkey "Ctrl+F10"

# Adjust buffer and save duration
mebal --buffer-duration 600 --save-duration 60

# Specify output directory
mebal --output ~/Videos/replays

# Run in test mode (test pattern)
mebal --test

# List available capture sources
mebal --list-sources

# List available encoders
mebal --list-encoders
```

### Configuration

Configuration is stored in:
- **Linux**: `~/.config/mebal/config.toml`
- **Windows**: `%APPDATA%\mebal\config.toml`
- **macOS**: `~/Library/Application Support/mebal/config.toml`

Example `config.toml`:

```toml
# Buffer duration in seconds (how long to keep in memory)
buffer_duration_secs = 300

# Duration of saved clips in seconds
save_duration_secs = 30

# Video bitrate in kbps
bitrate_kbps = 8000

# Frames per second
fps = 60

# Output directory
output_directory = "/home/user/Videos/mebal"

# Output filename prefix
output_prefix = "replay"

# Hotkey combination
hotkey = "F9"

# Video resolution
resolution = [1920, 1080]

# Capture source (optional, auto-detected if not set)
# capture_source = "test"  # For testing
# capture_source = ":0.0"  # X11 display
# capture_source = "kms"   # KMS/DRM

# Audio capture (disabled by default)
audio_enabled = false
audio_bitrate_kbps = 128
```

## Module Structure

```
src/
├── main.rs           # Application entry point
├── lib.rs            # Library exports
├── error.rs          # Error types
├── buffer/           # Circular packet buffer
│   ├── mod.rs        # Buffer implementation
│   └── packet.rs     # Packet types
├── capture/          # Video capture
│   ├── mod.rs        # Capture manager
│   ├── display.rs    # Display sources
│   └── encoder_setup.rs  # Encoder configuration
├── writer/           # Video output
│   └── mod.rs        # Video file writer
├── hotkey/           # Global hotkey handling
│   ├── mod.rs        # Hotkey manager
│   └── parser.rs     # Hotkey string parser
└── config.rs         # Configuration management
```

## Key Design Decisions

### 1. Encoded Packet Storage

Instead of storing raw video frames (which would require ~500MB/s at 1080p60), Mebal stores **encoded H.264 packets** (~8-16MB/s). This allows for:
- Longer buffer durations with less memory
- Lower CPU usage (no re-encoding on save)
- Instant save operations

### 2. Circular Buffer

The packet buffer is a thread-safe circular buffer that:
- Automatically evicts old packets when full
- Maintains chronological order
- Supports lock-free reads for saving

### 3. Hardware Acceleration

Mebal automatically detects and uses hardware encoders:
- **NVIDIA**: NVENC (`h264_nvenc`)
- **Intel**: QuickSync (`h264_qsv`) / VAAPI (`h264_vaapi`)
- **AMD**: AMF (`h264_amf`)
- **macOS**: VideoToolbox (`h264_videotoolbox`)
- **Fallback**: x264 software encoder

### 4. Cross-Platform Capture

Platform-specific capture backends:
- **Linux**: X11 (`x11grab`) or KMS/DRM (`kmsgrab`)
- **Windows**: GDI (`gdigrab`) or DirectShow
- **macOS**: AVFoundation

## API Usage

Mebal can also be used as a library:

```rust
use mebal::{Config, CaptureManager, PacketBuffer, VideoWriter, HotkeyManager};
use std::sync::Arc;
use tokio::sync::RwLock;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load configuration
    let config = Config::load()?;
    
    // Create packet buffer
    let buffer = Arc::new(RwLock::new(PacketBuffer::new(
        config.buffer_duration_secs,
        config.fps,
    )));
    
    // Start capture
    let mut capture = CaptureManager::new(&config)?;
    let capture_handle = tokio::spawn({
        let buffer = buffer.clone();
        async move {
            capture.run(buffer).await
        }
    });
    
    // Set up hotkey
    let mut hotkey = HotkeyManager::new("F9")?;
    hotkey.on_trigger(move || {
        // Save replay logic
    });
    
    // Run
    tokio::select! {
        _ = hotkey.run() => {},
        _ = capture_handle => {},
    }
    
    Ok(())
}
```

## Performance

Memory usage estimation:
- **1080p60 @ 8Mbps**: ~60MB per minute of buffer
- **5-minute buffer**: ~300MB RAM
- **Save operation**: Near-instant (no re-encoding)

## Troubleshooting

### No capture source detected
```bash
# List available sources
mebal --list-sources

# Use test pattern
mebal --test
```

### High CPU usage
- Enable hardware encoding: Check `mebal --list-encoders`
- Reduce resolution or FPS in config
- Lower bitrate

### Hotkey not working
- Check if the hotkey is already bound by another application
- Try a different hotkey: `mebal --hotkey "Ctrl+F10"`
- Run with elevated permissions if needed

### Poor video quality
- Increase bitrate in config: `bitrate_kbps = 12000`
- Use a slower encoder preset (if using x264)

## License

MIT License - See LICENSE file for details.

## Contributing

Contributions are welcome! Please read CONTRIBUTING.md for guidelines.

## Acknowledgments

- [FFmpeg](https://ffmpeg.org/) - The universal multimedia toolkit
- [rust-ffmpeg](https://github.com/zmwangx/rust-ffmpeg) - Rust FFmpeg bindings
- [global-hotkey](https://github.com/tauri-apps/global-hotkey) - Cross-platform hotkey support
