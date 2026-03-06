use mebal::{AppState, AudioCaptureManager, CaptureManager, Config, HotkeyManager};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Tauri-managed application state wrapping the core library AppState.
pub struct TauriAppState {
    pub inner: Arc<AppState>,
    cancel_token: Mutex<Option<CancellationToken>>,
    capturing: AtomicBool,
    hotkey_manager: Mutex<Option<HotkeyManager>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    pub packet_count: usize,
    pub total_bytes: usize,
    pub max_bytes: usize,
    pub duration_secs: u32,
    pub utilization_percent: f64,
    pub is_capturing: bool,
    pub is_saving: bool,
}

impl TauriAppState {
    pub fn new(config: Config) -> Self {
        Self {
            inner: Arc::new(AppState::new(config)),
            cancel_token: Mutex::new(None),
            capturing: AtomicBool::new(false),
            hotkey_manager: Mutex::new(None),
        }
    }

    /// Store the hotkey manager so it stays alive (and can be re-registered).
    pub fn set_hotkey_manager(&self, manager: HotkeyManager) {
        *self.hotkey_manager.lock() = Some(manager);
    }

    pub fn is_capturing(&self) -> bool {
        self.capturing.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> StatusResponse {
        let stats = self.inner.packet_buffer.stats();
        StatusResponse {
            packet_count: stats.packet_count,
            total_bytes: stats.total_bytes,
            max_bytes: stats.max_bytes,
            duration_secs: stats.duration_secs,
            utilization_percent: stats.utilization_percent(),
            is_capturing: self.is_capturing(),
            is_saving: self.inner.is_saving(),
        }
    }

    /// Start video + audio capture tasks.
    pub fn start_capture(&self) -> anyhow::Result<()> {
        if self.is_capturing() {
            return Ok(());
        }

        let config = self.inner.config();
        let cancel = CancellationToken::new();
        let capture_start = Instant::now();

        // Video capture
        let buffer = Arc::clone(&self.inner.packet_buffer);
        let video_cancel = cancel.clone();
        let video_config = config.clone();
        std::thread::spawn(move || match CaptureManager::new(&video_config) {
            Ok(capture) => {
                if let Err(e) = capture.run_blocking(buffer, video_cancel, capture_start) {
                    error!("Capture error: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to create capture manager: {}", e);
            }
        });

        // Audio capture
        if config.audio_enabled {
            let buffer = Arc::clone(&self.inner.packet_buffer);
            let audio_cancel = cancel.clone();
            let audio_config = config.clone();
            std::thread::spawn(move || {
                let audio = AudioCaptureManager::new(&audio_config);
                if let Err(e) = audio.run_blocking(buffer, audio_cancel, capture_start) {
                    warn!("Audio capture failed: {} — continuing video-only", e);
                }
            });
        }

        *self.cancel_token.lock() = Some(cancel);
        self.capturing.store(true, Ordering::SeqCst);
        info!("Capture started");
        Ok(())
    }

    /// Stop capture by cancelling the token.
    pub fn stop_capture(&self) {
        if let Some(token) = self.cancel_token.lock().take() {
            token.cancel();
        }
        self.capturing.store(false, Ordering::SeqCst);
        info!("Capture stopped");
    }

    /// Stop capture, reconfigure the buffer, and restart with the current config.
    pub fn restart_with_config(&self, _config: Config) {
        self.stop_capture();
        self.inner.reconfigure_buffer();
        self.inner.packet_buffer.clear();
        if let Err(e) = self.start_capture() {
            error!("Failed to restart capture: {}", e);
        }
    }
}

/// Polls buffer status every second and emits `buffer-status` events.
pub async fn status_poll_loop(handle: &AppHandle) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        interval.tick().await;
        let ts = handle.state::<TauriAppState>();
        let status = ts.status();
        let _ = handle.emit("buffer-status", &status);
    }
}
