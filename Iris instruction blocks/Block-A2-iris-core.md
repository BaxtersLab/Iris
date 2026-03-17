Block A-2 — iris-core
=====================

Objective
---------
Implement the iris-core crate: configuration system, application state with watch
channels, logging initialization, and error types. This mirrors BSR's bsr-core
(config.rs, app.rs, logging.rs) but is adapted for webcam-specific settings.

Prerequisites
-------------
Block A-1 (workspace scaffolding) must be complete.

File: crates/iris-core/lib.rs
------------------------------
Public modules: config, app, logging, error.

```rust
pub mod config;
pub mod app;
pub mod logging;
pub mod error;
```

File: crates/iris-core/error.rs
-------------------------------
Define `IrisError` using thiserror:

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum IrisError {
    #[error("config error: {0}")]
    Config(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("device error: {0}")]
    Device(String),
    #[error("capture error: {0}")]
    Capture(String),
    #[error("stream error: {0}")]
    Stream(String),
    #[error("control error: {0}")]
    Control(String),
    #[error("ipc error: {0}")]
    Ipc(String),
}

pub type IrisResult<T> = Result<T, IrisError>;
```

File: crates/iris-core/config.rs
---------------------------------
Mirrors BSR's config.rs pattern. Nested config structs, Default impls, TOML
serialization, load/save/validate, exe-relative path resolution.

### Structs

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrisConfig {
    pub device: DeviceConfig,
    pub capture: CaptureConfig,
    pub controls: ControlsConfig,
    pub stream: StreamConfig,
    pub telemetry: TelemetryConfig,
    pub ui: UiConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    /// Preferred device name substring (e.g. "C920"). Empty = first available.
    pub preferred_device: String,
    /// Whether to auto-reconnect on USB disconnect.
    pub auto_reconnect: bool,
    /// Maximum reconnect attempts before giving up.
    pub max_reconnect_attempts: u32,
    /// Reconnect delay in milliseconds.
    pub reconnect_delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureConfig {
    /// Target width (e.g. 3840 for 4K).
    pub width: u32,
    /// Target height (e.g. 2160 for 4K).
    pub height: u32,
    /// Target frames per second.
    pub target_fps: u32,
    /// Pixel format preference: "nv12", "yuy2", "mjpeg", "bgra8"
    pub pixel_format: String,
    /// Maximum frame queue depth before dropping.
    pub max_queue_depth: usize,
    /// Drop policy: "oldest" or "newest"
    pub drop_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlsConfig {
    /// Auto-exposure enabled at startup.
    pub auto_exposure: bool,
    /// Auto-focus enabled at startup.
    pub auto_focus: bool,
    /// Auto-white-balance enabled at startup.
    pub auto_white_balance: bool,
    /// Default profile name to load at startup (empty = none).
    pub default_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamConfig {
    /// Default stream mode: "pull", "push", "shared_memory", "ipc"
    pub default_mode: String,
    /// Ring buffer capacity (number of frames for shared-memory mode).
    pub ring_buffer_capacity: usize,
    /// Maximum number of concurrent subscribers.
    pub max_subscribers: usize,
    /// IPC output pipe name (for IPC mode).
    pub ipc_pipe_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Whether telemetry emission is enabled.
    pub enabled: bool,
    /// Telemetry output mode: "ipc", "file", "both"
    pub output_mode: String,
    /// Path for file-based telemetry output.
    pub file_path: String,
    /// Maximum telemetry events per second (rate limiting).
    pub max_events_per_second: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    /// Whether the UI window opens at launch.
    pub show_on_start: bool,
    /// Preview scale factor (0.25, 0.5, 1.0).
    pub preview_scale: f32,
    /// Show telemetry panel on start.
    pub show_telemetry_panel: bool,
    /// Show diagnostics panel on start.
    pub show_diagnostics_panel: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Minimum log level: "trace", "debug", "info", "warn", "error"
    pub level: String,
    /// Log to file
    pub log_to_file: bool,
    /// Log file directory (relative to exe)
    pub log_dir: String,
}
```

### Default Impls
Provide sensible defaults:
- DeviceConfig: preferred_device = "", auto_reconnect = true, max_reconnect_attempts = 5, reconnect_delay_ms = 2000
- CaptureConfig: width = 3840, height = 2160, target_fps = 30, pixel_format = "nv12", max_queue_depth = 4, drop_policy = "oldest"
- ControlsConfig: auto_exposure = true, auto_focus = true, auto_white_balance = true, default_profile = ""
- StreamConfig: default_mode = "pull", ring_buffer_capacity = 8, max_subscribers = 4, ipc_pipe_name = "\\\\.\\pipe\\iris-stream"
- TelemetryConfig: enabled = true, output_mode = "ipc", file_path = "logs/telemetry.jsonl", max_events_per_second = 120
- UiConfig: show_on_start = true, preview_scale = 0.5, show_telemetry_panel = true, show_diagnostics_panel = false
- LoggingConfig: level = "info", log_to_file = true, log_dir = "logs"

### Methods on IrisConfig
```rust
impl IrisConfig {
    /// Load from iris.toml next to the executable, or return defaults.
    pub fn load() -> IrisResult<Self> { ... }

    /// Save current config to iris.toml next to the executable.
    pub fn save(&self) -> IrisResult<()> { ... }

    /// Validate all fields. Return Err with details on invalid values.
    pub fn validate(&self) -> IrisResult<()> { ... }

    /// Resolve the absolute config file path (exe-relative).
    pub fn config_path() -> IrisResult<std::path::PathBuf> { ... }
}
```

Validation rules:
- width must be 1..=7680, height must be 1..=4320
- target_fps must be 1..=240
- pixel_format must be one of: "nv12", "yuy2", "mjpeg", "bgra8"
- max_queue_depth must be >= 1
- drop_policy must be "oldest" or "newest"
- ring_buffer_capacity must be >= 2
- max_subscribers must be >= 1
- preview_scale must be > 0.0 and <= 2.0
- level must be one of: "trace", "debug", "info", "warn", "error"

File: crates/iris-core/app.rs
------------------------------
Application state, broadcast via tokio::sync::watch.

```rust
use tokio::sync::watch;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureState {
    /// No device connected.
    Disconnected,
    /// Device found, initializing.
    Initializing,
    /// Actively capturing frames.
    Capturing,
    /// Capture paused by user.
    Paused,
    /// Error state with message.
    Error(String),
    /// Shutting down.
    ShuttingDown,
}

#[derive(Debug, Clone)]
pub struct AppState {
    /// Current capture state.
    capture_state: watch::Sender<CaptureState>,
    /// Receiver for capture state.
    capture_state_rx: watch::Receiver<CaptureState>,
    /// Connected device name (empty if none).
    device_name: watch::Sender<String>,
    device_name_rx: watch::Receiver<String>,
    /// Current FPS (measured).
    current_fps: watch::Sender<f64>,
    current_fps_rx: watch::Receiver<f64>,
    /// Frame counter (total frames captured this session).
    frame_count: watch::Sender<u64>,
    frame_count_rx: watch::Receiver<u64>,
    /// Active subscriber count.
    subscriber_count: watch::Sender<usize>,
    subscriber_count_rx: watch::Receiver<usize>,
}
```

Provide:
- `AppState::new()` — creates all watch channels with initial values
- `subscribe_capture_state()` — returns a watch::Receiver<CaptureState>
- Getter + setter for each field
- `AppState` should be wrapped in `Arc<AppState>` in usage

File: crates/iris-core/logging.rs
---------------------------------
```rust
/// Initialize the tracing subscriber.
/// If `log_to_file` is true, also write to a file in `log_dir`.
pub fn init_logging(level: &str, log_to_file: bool, log_dir: &str) -> IrisResult<()> {
    // Use tracing_subscriber with env_filter
    // If log_to_file, add a file appender layer
    // Format: timestamp level target message
    todo!()
}
```

Note: The `todo!()` is acceptable here; it will be filled in during integration (I-1).
Provide a basic implementation that at minimum sets up stdout logging with the
given level filter.

Unit Tests
----------
File: crates/iris-core/tests.rs (included from lib.rs via `#[cfg(test)] mod tests;`)

### Required Tests

1. `test_default_config` — IrisConfig::default() produces valid config
2. `test_config_roundtrip` — serialize to TOML, deserialize back, assert equality
3. `test_config_validation_valid` — default config passes validate()
4. `test_config_validation_invalid_fps` — target_fps = 0 fails validation
5. `test_config_validation_invalid_resolution` — width = 0 fails validation
6. `test_config_validation_invalid_pixel_format` — pixel_format = "rgb565" fails
7. `test_config_validation_invalid_drop_policy` — drop_policy = "random" fails
8. `test_app_state_capture_state` — create AppState, set capture state, subscriber sees update
9. `test_app_state_fps_update` — set current_fps, subscriber sees value
10. `test_app_state_device_name` — set device name, receiver gets it

Acceptance Criteria
-------------------
1. `cargo check -p iris-core` passes
2. `cargo test -p iris-core` — all 10 tests pass
3. Config roundtrip: default → toml string → parse → assert equal to original
4. AppState watch channels propagate correctly
5. No unwrap() on user-facing error paths (use IrisResult)
