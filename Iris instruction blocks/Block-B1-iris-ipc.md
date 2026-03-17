Block B-1 — iris-ipc
====================

Objective
---------
Implement the iris-ipc crate: IPC command/response protocol, 26+ telemetry event
types, IPC server/client with mpsc+oneshot channels, and JSON envelope tests.
Mirrors BSR's bsr-ipc pattern but expanded for webcam telemetry.

Prerequisites
-------------
Blocks A-1 and A-2 must be complete.

File: crates/iris-ipc/lib.rs
------------------------------
Public modules: command, response, telemetry, server, client, envelope.

```rust
pub mod command;
pub mod response;
pub mod telemetry;
pub mod server;
pub mod client;
pub mod envelope;
```

File: crates/iris-ipc/command.rs
---------------------------------
All commands that can be sent to Iris's core engine.

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args")]
pub enum IpcCommand {
    // === Lifecycle ===
    /// Initialize the system with the given config path (or None for defaults).
    Init { config_path: Option<String> },
    /// Shut down gracefully.
    Shutdown,
    /// Request current status.
    GetStatus,
    /// Ping for liveness check.
    Ping,

    // === Device ===
    /// List all available USB video devices.
    ListDevices,
    /// Select a device by index or name.
    SelectDevice { device_id: String },
    /// Get capabilities of the currently selected device.
    GetDeviceCapabilities,
    /// Disconnect from the current device.
    DisconnectDevice,

    // === Capture ===
    /// Start capturing frames.
    StartCapture,
    /// Stop capturing frames.
    StopCapture,
    /// Pause capture (keeps device open).
    PauseCapture,
    /// Resume from paused state.
    ResumeCapture,
    /// Set capture resolution.
    SetResolution { width: u32, height: u32 },
    /// Set target FPS.
    SetFps { fps: u32 },
    /// Set pixel format.
    SetPixelFormat { format: String },
    /// Set region of interest (crop). None = full frame.
    SetRoi { x: u32, y: u32, width: u32, height: u32 },
    /// Clear ROI (return to full frame).
    ClearRoi,

    // === Controls ===
    /// Get current value of a camera control.
    GetControl { control: String },
    /// Set a camera control value.
    SetControl { control: String, value: i64 },
    /// Reset a control to its default.
    ResetControl { control: String },
    /// List all available controls and their capabilities.
    ListControls,
    /// Load a named control profile.
    LoadProfile { name: String },
    /// Save current controls as a named profile.
    SaveProfile { name: String },

    // === Stream ===
    /// Subscribe to the frame stream. Returns a subscriber ID.
    Subscribe,
    /// Unsubscribe from the frame stream.
    Unsubscribe { subscriber_id: u64 },
    /// Get stream statistics.
    GetStreamStats,

    // === Config ===
    /// Reload config from disk.
    ReloadConfig,
    /// Get current config as JSON.
    GetConfig,
    /// Update a config section.
    UpdateConfig { section: String, json: String },
}
```

File: crates/iris-ipc/response.rs
----------------------------------
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum IpcResponse {
    Ok(ResponseData),
    Error { code: u32, message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum ResponseData {
    Empty,
    Pong { uptime_ms: u64 },
    Status {
        capture_state: String,
        device_name: String,
        fps: f64,
        frame_count: u64,
        subscriber_count: usize,
    },
    DeviceList { devices: Vec<DeviceEntry> },
    DeviceCapabilities { capabilities: String },
    ControlValue { control: String, value: i64 },
    ControlList { controls: Vec<ControlEntry> },
    StreamStats {
        frames_delivered: u64,
        frames_dropped: u64,
        subscriber_count: usize,
        ring_buffer_usage: f32,
    },
    SubscriberId { id: u64 },
    Config { json: String },
    ProfileSaved { name: String },
    ProfileLoaded { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEntry {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub resolutions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlEntry {
    pub name: String,
    pub current: i64,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub default: i64,
    pub auto_supported: bool,
}
```

File: crates/iris-ipc/telemetry.rs
-----------------------------------
26+ telemetry event types covering all subsystems. Every event gets a timestamp.

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEnvelope {
    pub timestamp: DateTime<Utc>,
    pub sequence: u64,
    pub event: TelemetryEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum TelemetryEvent {
    // === Lifecycle (1-4) ===
    SystemStarted { version: String },
    SystemShutdown { reason: String },
    ConfigLoaded { path: String },
    ConfigError { message: String },

    // === Device (5-10) ===
    DeviceEnumerated { count: usize },
    DeviceSelected { device_id: String, name: String },
    DeviceConnected { device_id: String },
    DeviceDisconnected { device_id: String, reason: String },
    DeviceReconnecting { attempt: u32, max_attempts: u32 },
    DeviceCapabilitiesProbed { device_id: String, resolutions: Vec<String> },

    // === Capture (11-17) ===
    CaptureStarted { width: u32, height: u32, fps: u32, format: String },
    CaptureStopped { total_frames: u64 },
    CapturePaused,
    CaptureResumed,
    FrameCaptured { sequence: u64, width: u32, height: u32, size_bytes: usize },
    FrameDropped { sequence: u64, reason: String },
    CaptureError { message: String },

    // === Controls (18-21) ===
    ControlChanged { control: String, old_value: i64, new_value: i64 },
    ControlAutoToggled { control: String, auto_enabled: bool },
    ProfileLoaded { name: String, controls_applied: usize },
    ProfileSaved { name: String },

    // === Stream (22-25) ===
    SubscriberAdded { id: u64, total: usize },
    SubscriberRemoved { id: u64, total: usize },
    StreamDelivery { subscriber_id: u64, frame_sequence: u64, latency_us: u64 },
    RingBufferOverflow { dropped_frames: u64 },

    // === Health (26-30) ===
    HealthCheck { cpu_percent: f32, memory_mb: f32, usb_bandwidth_percent: f32 },
    UsbBandwidthWarning { current_percent: f32, threshold: f32 },
    ThermalWarning { temperature_c: f32 },
    ErrorRecovered { subsystem: String, message: String },
    FatalError { subsystem: String, message: String },
}
```

File: crates/iris-ipc/envelope.rs
----------------------------------
JSON envelope wrapper for IPC transport.

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcEnvelope {
    pub id: u64,
    pub payload: IpcPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "body")]
pub enum IpcPayload {
    Command(super::command::IpcCommand),
    Response(super::response::IpcResponse),
    Telemetry(super::telemetry::TelemetryEnvelope),
}
```

File: crates/iris-ipc/server.rs
--------------------------------
Mirrors BSR's IpcServer pattern — mpsc channels for incoming commands, oneshot for
responses, broadcast for telemetry.

```rust
use tokio::sync::{mpsc, oneshot, broadcast};

pub struct IpcServer {
    /// Receive commands from clients.
    cmd_rx: mpsc::Receiver<(IpcCommand, oneshot::Sender<IpcResponse>)>,
    /// Send telemetry to all listeners.
    telemetry_tx: broadcast::Sender<TelemetryEnvelope>,
}

impl IpcServer {
    pub fn new(buffer_size: usize) -> (Self, IpcHandle) { ... }

    /// Run the server loop: receive commands, dispatch, send responses.
    pub async fn run(self) { ... }

    /// Emit a telemetry event to all subscribers.
    pub fn emit_telemetry(&self, event: TelemetryEvent) { ... }
}

/// Handle given to services to send commands and receive telemetry.
pub struct IpcHandle {
    cmd_tx: mpsc::Sender<(IpcCommand, oneshot::Sender<IpcResponse>)>,
    telemetry_rx: broadcast::Receiver<TelemetryEnvelope>,
}

impl IpcHandle {
    /// Send a command and await its response.
    pub async fn send_command(&self, cmd: IpcCommand) -> IrisResult<IpcResponse> { ... }

    /// Subscribe to telemetry events.
    pub fn subscribe_telemetry(&self) -> broadcast::Receiver<TelemetryEnvelope> { ... }
}
```

File: crates/iris-ipc/client.rs
--------------------------------
Client-side IPC (for external consumers). Takes an IpcHandle.

```rust
pub struct IpcClient {
    handle: IpcHandle,
}

impl IpcClient {
    pub fn new(handle: IpcHandle) -> Self { ... }
    pub async fn ping(&self) -> IrisResult<IpcResponse> { ... }
    pub async fn get_status(&self) -> IrisResult<IpcResponse> { ... }
    pub async fn start_capture(&self) -> IrisResult<IpcResponse> { ... }
    pub async fn stop_capture(&self) -> IrisResult<IpcResponse> { ... }
    pub async fn list_devices(&self) -> IrisResult<IpcResponse> { ... }
    pub async fn select_device(&self, id: String) -> IrisResult<IpcResponse> { ... }
    pub async fn subscribe(&self) -> IrisResult<IpcResponse> { ... }
    pub async fn unsubscribe(&self, id: u64) -> IrisResult<IpcResponse> { ... }
    // Additional convenience methods for each command...
}
```

Unit Tests
----------
File: crates/iris-ipc/tests.rs

### Required Tests

1. `test_command_json_roundtrip` — for every IpcCommand variant: serialize → deserialize → assert equal
2. `test_response_json_roundtrip` — for every IpcResponse variant
3. `test_telemetry_json_roundtrip` — for every TelemetryEvent variant
4. `test_envelope_json_roundtrip` — IpcEnvelope with Command payload, Response payload, Telemetry payload
5. `test_ipc_server_ping` — create server, send Ping, receive Pong
6. `test_ipc_server_telemetry_broadcast` — emit telemetry, verify subscriber receives it
7. `test_ipc_client_send_command` — client sends GetStatus, gets response
8. `test_telemetry_sequence_ordering` — emit multiple events, verify sequence numbers increment

Acceptance Criteria
-------------------
1. `cargo check -p iris-ipc` passes
2. `cargo test -p iris-ipc` — all 8 tests pass
3. Every IpcCommand variant round-trips through JSON
4. Every TelemetryEvent variant round-trips through JSON
5. IpcServer properly dispatches commands to handlers
6. Telemetry broadcast delivers to all subscribers
7. All 30 telemetry event types are defined and serializable
