// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

use mebal::capture::audio::run_audio_capture;
use mebal::capture::run_video_capture;
use mebal::{App, AppState, Config, HotkeyManager};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{error, info};

pub struct TauriAppState {
    pub inner: App,
    cancel_token: Mutex<Option<CancellationToken>>,
    capturing: AtomicBool,
    hotkey_manager: Mutex<Option<HotkeyManager>>,
    capture_threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
    pub task_tracker: TaskTracker,
    /// Fixed for the app's lifetime — packet PTS is `elapsed since this` * fps.
    /// Must not be reset on stop/start-capture cycles: buffered packets from
    /// before a pause keep their PTS, and re-anchoring to a fresh `Instant::now()`
    /// on resume would make new packets' PTS lower than the still-buffered old
    /// ones, breaking the buffer's ascending-PTS invariant.
    capture_epoch: Instant,
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
            inner: AppState::new(config),
            cancel_token: Mutex::new(None),
            capturing: AtomicBool::new(false),
            hotkey_manager: Mutex::new(None),
            capture_threads: Mutex::new(Vec::new()),
            task_tracker: TaskTracker::new(),
            capture_epoch: Instant::now(),
        }
    }

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

    pub fn start_capture(&self) -> anyhow::Result<()> {
        if self.is_capturing() {
            return Ok(());
        }

        let config = self.inner.config();
        let cancel = CancellationToken::new();
        let capture_start = self.capture_epoch;

        let mut threads = self.capture_threads.lock();

        let buffer = Arc::clone(&self.inner.packet_buffer);
        let video_cancel = cancel.clone();
        let video_config = (*config).clone();
        let video_handle = std::thread::spawn(move || {
            run_video_capture(&video_config, buffer, video_cancel, capture_start)
        });
        threads.push(video_handle);

        if config.audio_enabled {
            let buffer = Arc::clone(&self.inner.packet_buffer);
            let audio_cancel = cancel.clone();
            let audio_config = (*config).clone();
            let audio_handle = std::thread::spawn(move || {
                run_audio_capture(&audio_config, buffer, audio_cancel, capture_start)
            });
            threads.push(audio_handle);
        }

        *self.cancel_token.lock() = Some(cancel);
        self.capturing.store(true, Ordering::SeqCst);
        info!("Capture started");
        Ok(())
    }

    pub fn stop_capture(&self) {
        if let Some(token) = self.cancel_token.lock().take() {
            token.cancel();
        }

        let threads: Vec<_> = self.capture_threads.lock().drain(..).collect();
        for handle in threads {
            let _ = handle.join();
        }

        self.capturing.store(false, Ordering::SeqCst);
        info!("Capture stopped");
    }

    pub fn restart_with_config(&self, _config: Config) {
        self.stop_capture();
        self.inner.reconfigure_buffer();
        self.inner.packet_buffer.clear();
        if let Err(e) = self.start_capture() {
            error!("Failed to restart capture: {}", e);
        }
    }
}

pub async fn status_poll_loop(handle: &AppHandle, cancel: CancellationToken) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let ts = handle.state::<TauriAppState>();
                let status = ts.status();
                let _ = handle.emit("buffer-status", &status);
            }
            _ = cancel.cancelled() => break,
        }
    }
}
