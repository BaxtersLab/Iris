Block E-1 — iris-capture
========================

Objective
---------
Implement the iris-capture crate: async frame pipeline with FPS pacing, frame drop
policy, ROI cropping, pixel format metadata, and telemetry. Mirrors BSR's
bsr-capture (CaptureBackend trait, generic CaptureService<B>, mock backend) but
improved with sequence counters, drop policy, and ROI support.

Prerequisites
-------------
Blocks A-1, A-2, B-1, C-1, and D-1 must be complete.

File: crates/iris-capture/lib.rs
---------------------------------
Public modules: frame, backend, service, telemetry.

```rust
pub mod frame;
pub mod backend;
pub mod service;
pub mod telemetry;
```

File: crates/iris-capture/frame.rs
-----------------------------------

```rust
use iris_hal::device::PixelFormat;
use serde::{Deserialize, Serialize};

/// A captured video frame with metadata.
#[derive(Debug, Clone)]
pub struct CaptureFrame {
    /// Monotonically increasing sequence number (per session).
    pub sequence: u64,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Pixel format of the raw data.
    pub format: PixelFormat,
    /// Raw pixel data.
    pub data: Vec<u8>,
    /// Timestamp when the frame was captured (monotonic, in microseconds).
    pub timestamp_us: u64,
    /// Whether this frame was cropped by ROI.
    pub is_cropped: bool,
}

impl CaptureFrame {
    /// Calculate the size of the raw data in bytes.
    pub fn size_bytes(&self) -> usize {
        self.data.len()
    }

    /// Calculate expected data size based on resolution and format.
    pub fn expected_size(width: u32, height: u32, format: PixelFormat) -> usize {
        let pixels = (width * height) as usize;
        match format {
            PixelFormat::Bgra8 => pixels * 4,
            PixelFormat::Nv12 => pixels * 3 / 2,
            PixelFormat::Yuy2 => pixels * 2,
            PixelFormat::Mjpeg => 0, // variable, can't predict
        }
    }
}

/// Region of interest for cropping.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Roi {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Roi {
    /// Validate that the ROI fits within the given frame dimensions.
    pub fn validate(&self, frame_width: u32, frame_height: u32) -> bool {
        self.x + self.width <= frame_width
            && self.y + self.height <= frame_height
            && self.width > 0
            && self.height > 0
    }
}
```

File: crates/iris-capture/backend.rs
-------------------------------------
The CaptureBackend trait — abstraction over HAL backends.

```rust
use async_trait::async_trait;
use super::frame::CaptureFrame;
use iris_core::error::IrisResult;

/// Drop policy when the frame queue is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropPolicy {
    /// Drop the oldest frame in the queue.
    Oldest,
    /// Drop the newest frame (the one just captured).
    Newest,
}

impl DropPolicy {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "oldest" => Some(DropPolicy::Oldest),
            "newest" => Some(DropPolicy::Newest),
            _ => None,
        }
    }
}

/// Configuration for the capture pipeline.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub width: u32,
    pub height: u32,
    pub target_fps: u32,
    pub format: iris_hal::device::PixelFormat,
    pub max_queue_depth: usize,
    pub drop_policy: DropPolicy,
    pub roi: Option<super::frame::Roi>,
}

#[async_trait]
pub trait CaptureBackend: Send + Sync {
    /// Start capturing frames.
    async fn start(&mut self) -> IrisResult<()>;

    /// Stop capturing frames.
    async fn stop(&mut self) -> IrisResult<()>;

    /// Read the next frame. Blocks until a frame is available or timeout.
    async fn next_frame(&mut self) -> IrisResult<CaptureFrame>;

    /// Whether the backend is currently capturing.
    fn is_capturing(&self) -> bool;
}
```

### Mock CaptureBackend

```rust
pub struct MockCaptureBackend {
    capturing: bool,
    sequence: u64,
    config: CaptureConfig,
}

impl MockCaptureBackend {
    pub fn new(config: CaptureConfig) -> Self { ... }
}

#[async_trait]
impl CaptureBackend for MockCaptureBackend {
    async fn start(&mut self) -> IrisResult<()> {
        self.capturing = true;
        self.sequence = 0;
        Ok(())
    }

    async fn stop(&mut self) -> IrisResult<()> {
        self.capturing = false;
        Ok(())
    }

    async fn next_frame(&mut self) -> IrisResult<CaptureFrame> {
        if !self.capturing {
            return Err(IrisError::Capture("not capturing".into()));
        }
        // Sleep for 1/fps duration to simulate pacing
        tokio::time::sleep(std::time::Duration::from_millis(
            1000 / self.config.target_fps as u64,
        )).await;

        self.sequence += 1;
        let size = CaptureFrame::expected_size(
            self.config.width, self.config.height, self.config.format
        );
        let data = vec![128u8; size.max(1024)]; // Synthetic gray frame

        Ok(CaptureFrame {
            sequence: self.sequence,
            width: self.config.width,
            height: self.config.height,
            format: self.config.format,
            data,
            timestamp_us: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as u64,
            is_cropped: false,
        })
    }

    fn is_capturing(&self) -> bool {
        self.capturing
    }
}
```

File: crates/iris-capture/service.rs
--------------------------------------
The CaptureService — manages the capture pipeline, FPS pacing, frame queue, drop
policy, and ROI cropping.

```rust
use tokio::sync::{mpsc, watch, broadcast};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct CaptureService<B: CaptureBackend> {
    backend: B,
    config: CaptureConfig,
    /// Output channel for captured frames.
    frame_tx: mpsc::Sender<CaptureFrame>,
    /// Telemetry bridge.
    telemetry_tx: broadcast::Sender<iris_ipc::telemetry::TelemetryEnvelope>,
    /// Capture state broadcaster.
    state_tx: watch::Sender<CaptureServiceState>,
    /// Frames captured count.
    frame_count: Arc<AtomicU64>,
    /// Frames dropped count.
    drop_count: Arc<AtomicU64>,
    /// Current ROI (None = full frame).
    roi: Option<Roi>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureServiceState {
    Idle,
    Capturing,
    Paused,
    Error(String),
}

impl<B: CaptureBackend> CaptureService<B> {
    /// Create a new CaptureService.
    pub fn new(
        backend: B,
        config: CaptureConfig,
        telemetry_tx: broadcast::Sender<iris_ipc::telemetry::TelemetryEnvelope>,
    ) -> (Self, CaptureHandle) { ... }

    /// Run the capture loop:
    /// 1. Call backend.start()
    /// 2. Loop: backend.next_frame() → apply ROI crop → try_send to frame_tx
    /// 3. If frame_tx is full: apply drop_policy
    ///    - Oldest: recv one from the channel (discard), then send new
    ///    - Newest: drop the new frame
    /// 4. Emit FrameCaptured telemetry for each successful frame
    /// 5. Emit FrameDropped telemetry for each dropped frame
    /// 6. On error: emit CaptureError telemetry, set state to Error
    /// 7. On stop command: call backend.stop(), emit CaptureStopped
    pub async fn run(mut self) { ... }

    /// Apply ROI cropping to a frame.
    /// For BGRA8: extract the rectangular region.
    /// For other formats: skip cropping (return full frame with log warning).
    fn apply_roi(&self, frame: &mut CaptureFrame) { ... }
}

/// Handle for controlling the capture service from outside.
pub struct CaptureHandle {
    /// Receive captured frames.
    pub frame_rx: mpsc::Receiver<CaptureFrame>,
    /// Send control commands.
    cmd_tx: mpsc::Sender<CaptureCommand>,
    /// Watch capture state.
    state_rx: watch::Receiver<CaptureServiceState>,
    /// Read frame count.
    frame_count: Arc<AtomicU64>,
    /// Read drop count.
    drop_count: Arc<AtomicU64>,
}

#[derive(Debug)]
pub enum CaptureCommand {
    Pause,
    Resume,
    Stop,
    SetRoi(Option<Roi>),
    SetFps(u32),
}

impl CaptureHandle {
    pub async fn send(&self, cmd: CaptureCommand) -> IrisResult<()> { ... }
    pub fn state(&self) -> CaptureServiceState { ... }
    pub fn frame_count(&self) -> u64 { ... }
    pub fn drop_count(&self) -> u64 { ... }
}
```

File: crates/iris-capture/telemetry.rs
---------------------------------------

```rust
use serde::{Deserialize, Serialize};

/// Telemetry snapshot for the capture subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureTelemetry {
    pub frames_captured: u64,
    pub frames_dropped: u64,
    pub current_fps: f64,
    pub target_fps: u32,
    pub resolution: String,
    pub format: String,
    pub queue_depth: usize,
    pub roi_active: bool,
}
```

Unit Tests
----------
File: crates/iris-capture/tests.rs

### Required Tests

1. `test_capture_frame_size` — CaptureFrame::expected_size for each PixelFormat
2. `test_roi_validation_valid` — Roi within bounds passes validation
3. `test_roi_validation_invalid` — Roi exceeding frame dimensions fails
4. `test_mock_backend_start_stop` — start, verify capturing, stop, verify not
5. `test_mock_backend_next_frame` — start, get frame, verify sequence=1, width/height correct
6. `test_mock_backend_multiple_frames` — read 5 frames, verify sequences 1-5
7. `test_capture_service_basic_flow` — create service with mock backend, run in task, receive 3 frames from handle
8. `test_capture_service_pause_resume` — start, pause, verify no frames, resume, verify frames again
9. `test_capture_service_drop_policy_oldest` — set max_queue_depth=2, send 5 frames faster than consumer, verify oldest dropped
10. `test_capture_service_roi` — set ROI, capture frame, verify is_cropped=true and dimensions match ROI
11. `test_capture_telemetry_emission` — verify FrameCaptured telemetry events are emitted

Acceptance Criteria
-------------------
1. `cargo check -p iris-capture` passes
2. `cargo test -p iris-capture` — all 11 tests pass
3. CaptureService properly paces frames to target FPS
4. Drop policy correctly applied when queue is full
5. ROI cropping works for BGRA8 format
6. Telemetry events emitted for captures, drops, and errors
7. Pause/resume cycle works without losing state
8. Sequence numbers are monotonically increasing per session
