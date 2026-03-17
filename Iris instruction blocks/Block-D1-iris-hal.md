Block D-1 — iris-hal
====================

Objective
---------
Implement the iris-hal crate: USB UVC hardware abstraction layer. This is a NEW
crate with no BSR equivalent. It handles device enumeration, capability probing,
hotplug detection, and provides a trait for backends (mock + Windows Media
Foundation stub).

Prerequisites
-------------
Blocks A-1, A-2, and B-1 must be complete.

File: crates/iris-hal/lib.rs
------------------------------
Public modules: device, backend, hotplug, error.

```rust
pub mod device;
pub mod backend;
pub mod hotplug;
pub mod error;
```

File: crates/iris-hal/error.rs
-------------------------------
```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HalError {
    #[error("no device found")]
    NoDevice,
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("device busy: {0}")]
    DeviceBusy(String),
    #[error("capability not supported: {0}")]
    CapabilityNotSupported(String),
    #[error("backend error: {0}")]
    BackendError(String),
    #[error("hotplug error: {0}")]
    HotplugError(String),
    #[error("timeout")]
    Timeout,
}

pub type HalResult<T> = Result<T, HalError>;
```

File: crates/iris-hal/device.rs
--------------------------------

```rust
use serde::{Deserialize, Serialize};

/// Unique identifier for a USB video device.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub String);

/// Information about a discovered video capture device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: DeviceId,
    /// Human-readable display name (e.g. "Logitech C920 HD Pro").
    pub name: String,
    /// Vendor string if available.
    pub vendor: String,
    /// USB bus/port path if available.
    pub bus_info: String,
    /// Whether the device is currently in use by another process.
    pub in_use: bool,
}

/// A single supported resolution + framerate combination.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FormatDescriptor {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub pixel_format: PixelFormat,
}

/// Pixel formats supported by Iris.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PixelFormat {
    Nv12,
    Yuy2,
    Mjpeg,
    Bgra8,
}

impl std::fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PixelFormat::Nv12 => write!(f, "NV12"),
            PixelFormat::Yuy2 => write!(f, "YUY2"),
            PixelFormat::Mjpeg => write!(f, "MJPEG"),
            PixelFormat::Bgra8 => write!(f, "BGRA8"),
        }
    }
}

/// Full capabilities of a connected device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    pub device_id: DeviceId,
    /// All supported format/resolution/fps combinations.
    pub formats: Vec<FormatDescriptor>,
    /// Camera controls this device supports.
    pub controls: Vec<ControlCapabilityInfo>,
}

/// Information about a single camera control (from HAL's perspective).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlCapabilityInfo {
    pub name: String,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub default: i64,
    pub current: i64,
    pub auto_supported: bool,
}
```

File: crates/iris-hal/backend.rs
---------------------------------
The UvcBackend trait — the abstraction over platform-specific APIs.

```rust
use async_trait::async_trait;
use super::device::*;
use super::error::HalResult;

#[async_trait]
pub trait UvcBackend: Send + Sync {
    /// Enumerate all connected USB video devices.
    async fn enumerate_devices(&self) -> HalResult<Vec<DeviceInfo>>;

    /// Open a device for use. Must be called before probe/read operations.
    async fn open_device(&mut self, id: &DeviceId) -> HalResult<()>;

    /// Close the currently open device.
    async fn close_device(&mut self) -> HalResult<()>;

    /// Probe capabilities of the currently open device.
    async fn probe_capabilities(&self) -> HalResult<DeviceCapabilities>;

    /// Set the active format (resolution, fps, pixel format).
    async fn set_format(&mut self, format: &FormatDescriptor) -> HalResult<()>;

    /// Get the active format.
    async fn get_format(&self) -> HalResult<FormatDescriptor>;

    /// Read a single raw frame from the device. Returns raw bytes.
    async fn read_frame(&mut self) -> HalResult<Vec<u8>>;

    /// Get the value of a camera control by name.
    async fn get_control(&self, name: &str) -> HalResult<i64>;

    /// Set the value of a camera control by name.
    async fn set_control(&mut self, name: &str, value: i64) -> HalResult<()>;

    /// Get the name of this backend (e.g., "mock", "wmf").
    fn backend_name(&self) -> &str;
}
```

### Mock Backend

```rust
pub struct MockUvcBackend {
    devices: Vec<DeviceInfo>,
    opened_device: Option<DeviceId>,
    current_format: Option<FormatDescriptor>,
    frame_counter: u64,
}

impl MockUvcBackend {
    pub fn new() -> Self { ... }

    /// Add a fake device for testing.
    pub fn add_device(&mut self, info: DeviceInfo, caps: DeviceCapabilities) { ... }
}

#[async_trait]
impl UvcBackend for MockUvcBackend {
    // Implement all trait methods with mock behavior:
    // - enumerate_devices: return self.devices
    // - open_device: set opened_device
    // - read_frame: return a synthetic frame (solid color, incrementing counter)
    // - get_control/set_control: return/store values in a HashMap
    // etc.
}
```

### Windows Media Foundation Stub

```rust
#[cfg(windows)]
pub struct WmfBackend {
    // Fields will be populated in Block I-1 integration.
    // For now, all methods return HalError::BackendError("not yet implemented").
}

#[cfg(windows)]
impl WmfBackend {
    pub fn new() -> Self { Self {} }
}

#[cfg(windows)]
#[async_trait]
impl UvcBackend for WmfBackend {
    // All methods: Err(HalError::BackendError("WMF not yet implemented".into()))
}
```

File: crates/iris-hal/hotplug.rs
---------------------------------
USB hotplug detection (event-based, not polling).

```rust
use tokio::sync::mpsc;
use super::device::DeviceId;

/// Events from the USB hotplug monitor.
#[derive(Debug, Clone)]
pub enum HotplugEvent {
    DeviceConnected(DeviceId),
    DeviceDisconnected(DeviceId),
}

/// Monitors USB device connections/disconnections.
pub struct HotplugMonitor {
    event_tx: mpsc::Sender<HotplugEvent>,
}

impl HotplugMonitor {
    /// Create a new hotplug monitor. Returns the monitor and a receiver for events.
    pub fn new(buffer_size: usize) -> (Self, mpsc::Receiver<HotplugEvent>) { ... }

    /// Start monitoring for USB hotplug events.
    /// For now, this is a stub that does nothing (real implementation in I-1).
    /// It should run as an async task and emit events when devices are added/removed.
    pub async fn run(self) {
        // Stub: loop + sleep to keep the task alive. Will be wired to WMI or
        // SetupDiGetClassDevs polling in integration.
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
}

/// Handle for subscribing to hotplug events.
pub struct HotplugHandle {
    event_rx: mpsc::Receiver<HotplugEvent>,
}

impl HotplugHandle {
    pub async fn recv(&mut self) -> Option<HotplugEvent> {
        self.event_rx.recv().await
    }
}
```

Unit Tests
----------
File: crates/iris-hal/tests.rs

### Required Tests

1. `test_mock_enumerate_devices` — add 2 mock devices, enumerate, verify 2 returned
2. `test_mock_open_close_device` — open device, verify open, close, verify closed
3. `test_mock_probe_capabilities` — open device, probe caps, verify formats/controls
4. `test_mock_set_format` — set a format, get format, verify it matches
5. `test_mock_read_frame` — open device, set format, read frame, verify non-empty bytes
6. `test_mock_control_get_set` — set a control value, get it back, verify match
7. `test_device_id_equality` — DeviceId("a") == DeviceId("a"), != DeviceId("b")
8. `test_pixel_format_display` — PixelFormat::Nv12.to_string() == "NV12"
9. `test_hotplug_channel` — create HotplugMonitor, send fake event through channel, receive it

Acceptance Criteria
-------------------
1. `cargo check -p iris-hal` passes
2. `cargo test -p iris-hal` — all 9 tests pass
3. UvcBackend trait is fully defined with all required methods
4. MockUvcBackend implements all trait methods
5. WmfBackend stub compiles on Windows (all methods return not-yet-implemented)
6. DeviceInfo, DeviceCapabilities, PixelFormat are serializable
7. Hotplug monitor channel works (even if actual monitoring is stubbed)
