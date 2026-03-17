use tokio::sync::watch;
use std::sync::Arc;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureState {
    Disconnected,
    Initializing,
    Capturing,
    Paused,
    Error(String),
    ShuttingDown,
}

#[derive(Debug, Clone)]
pub struct AppState {
    capture_state_tx: watch::Sender<CaptureState>,
    _capture_state_rx: watch::Receiver<CaptureState>,

    device_name_tx: watch::Sender<String>,
    _device_name_rx: watch::Receiver<String>,

    current_fps_tx: watch::Sender<f64>,
    _current_fps_rx: watch::Receiver<f64>,

    frame_count_tx: watch::Sender<u64>,
    _frame_count_rx: watch::Receiver<u64>,

    subscriber_count_tx: watch::Sender<usize>,
    _subscriber_count_rx: watch::Receiver<usize>,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        let (capture_state_tx, capture_state_rx) = watch::channel(CaptureState::Disconnected);
        let (device_name_tx, device_name_rx) = watch::channel(String::new());
        let (current_fps_tx, current_fps_rx) = watch::channel(0.0);
        let (frame_count_tx, frame_count_rx) = watch::channel(0u64);
        let (subscriber_count_tx, subscriber_count_rx) = watch::channel(0usize);

        Arc::new(Self {
            capture_state_tx,
            _capture_state_rx: capture_state_rx,
            device_name_tx,
            _device_name_rx: device_name_rx,
            current_fps_tx,
            _current_fps_rx: current_fps_rx,
            frame_count_tx,
            _frame_count_rx: frame_count_rx,
            subscriber_count_tx,
            _subscriber_count_rx: subscriber_count_rx,
        })
    }

    pub fn subscribe_capture_state(self: &Arc<Self>) -> watch::Receiver<CaptureState> {
        self.capture_state_tx.subscribe()
    }

    pub fn set_capture_state(&self, s: CaptureState) {
        let _ = self.capture_state_tx.send(s);
    }

    pub fn subscribe_device_name(self: &Arc<Self>) -> watch::Receiver<String> {
        self.device_name_tx.subscribe()
    }

    pub fn set_device_name(&self, name: String) {
        let _ = self.device_name_tx.send(name);
    }

    pub fn subscribe_current_fps(self: &Arc<Self>) -> watch::Receiver<f64> {
        self.current_fps_tx.subscribe()
    }

    pub fn set_current_fps(&self, fps: f64) {
        let _ = self.current_fps_tx.send(fps);
    }

    pub fn subscribe_frame_count(self: &Arc<Self>) -> watch::Receiver<u64> {
        self.frame_count_tx.subscribe()
    }

    pub fn set_frame_count(&self, count: u64) {
        let _ = self.frame_count_tx.send(count);
    }

    pub fn subscribe_subscriber_count(self: &Arc<Self>) -> watch::Receiver<usize> {
        self.subscriber_count_tx.subscribe()
    }

    pub fn set_subscriber_count(&self, n: usize) {
        let _ = self.subscriber_count_tx.send(n);
    }
}
