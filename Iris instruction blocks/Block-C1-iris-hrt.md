Block C-1 — iris-hrt
====================

Objective
---------
Implement the iris-hrt crate: health, runtime, and thermal monitoring service.
Mirrors BSR's bsr-hrt pattern but adds USB bandwidth watchdog and device-specific
health events. Runs as a background Tokio task emitting telemetry via iris-ipc.

Prerequisites
-------------
Blocks A-1, A-2, and B-1 must be complete.

File: crates/iris-hrt/lib.rs
------------------------------
Public modules: event, service.

```rust
pub mod event;
pub mod service;
```

File: crates/iris-hrt/event.rs
-------------------------------

```rust
use serde::{Deserialize, Serialize};

/// Events the HRT service can detect / emit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HrtEvent {
    /// Periodic health tick with system metrics.
    HealthTick {
        cpu_percent: f32,
        memory_mb: f32,
        usb_bandwidth_percent: f32,
    },
    /// USB bandwidth usage exceeds threshold.
    UsbBandwidthWarning {
        current_percent: f32,
        threshold: f32,
    },
    /// USB device disconnected unexpectedly.
    UsbDisconnected {
        device_id: String,
    },
    /// Temperature exceeds safe operating range.
    ThermalWarning {
        temperature_c: f32,
    },
    /// A subsystem recovered from an error.
    ErrorRecovered {
        subsystem: String,
        message: String,
    },
    /// A fatal error occurred in a subsystem.
    FatalError {
        subsystem: String,
        message: String,
    },
}

/// Commands that can be sent to the HRT service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HrtCommand {
    /// Start monitoring.
    Start,
    /// Stop monitoring.
    Stop,
    /// Force an immediate health check.
    ForceCheck,
    /// Set the health check interval in milliseconds.
    SetInterval { interval_ms: u64 },
    /// Set the USB bandwidth warning threshold (0.0-1.0).
    SetUsbThreshold { threshold: f32 },
    /// Shutdown the HRT service.
    Shutdown,
}

/// Current status of the HRT service.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HrtStatus {
    Idle,
    Monitoring,
    Stopped,
}
```

File: crates/iris-hrt/service.rs
---------------------------------

```rust
use iris_ipc::telemetry::{TelemetryEnvelope, TelemetryEvent};
use tokio::sync::{mpsc, watch, broadcast};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Configuration for the HRT service.
pub struct HrtConfig {
    /// Health check interval in milliseconds.
    pub interval_ms: u64,
    /// USB bandwidth warning threshold (0.0 - 1.0).
    pub usb_bandwidth_threshold: f32,
    /// Temperature warning threshold in Celsius.
    pub thermal_threshold_c: f32,
}

impl Default for HrtConfig {
    fn default() -> Self {
        Self {
            interval_ms: 2000,
            usb_bandwidth_threshold: 0.85,
            thermal_threshold_c: 75.0,
        }
    }
}

/// The HRT background service.
pub struct HrtService {
    config: HrtConfig,
    /// Receive commands.
    cmd_rx: mpsc::Receiver<HrtCommand>,
    /// Send status updates.
    status_tx: watch::Sender<HrtStatus>,
    /// Bridge to IPC telemetry.
    telemetry_tx: broadcast::Sender<TelemetryEnvelope>,
    /// Sequence counter for telemetry envelopes.
    sequence: Arc<AtomicU64>,
}

impl HrtService {
    /// Create a new HRT service. Returns the service and a handle.
    pub fn new(
        config: HrtConfig,
        telemetry_tx: broadcast::Sender<TelemetryEnvelope>,
    ) -> (Self, HrtHandle) { ... }

    /// Run the monitoring loop.
    /// - On Start: begin periodic health checks at `interval_ms`
    /// - On each tick: collect CPU%, memory, USB bandwidth
    /// - If USB bandwidth > threshold: emit UsbBandwidthWarning telemetry
    /// - If temperature > threshold: emit ThermalWarning telemetry
    /// - Always emit HealthCheck telemetry on each tick
    /// - On ForceCheck: do an immediate check
    /// - On SetInterval: update the tick interval
    /// - On Stop: pause monitoring
    /// - On Shutdown: exit the loop
    pub async fn run(mut self) { ... }

    /// Collect current system metrics.
    /// For now, return placeholder values (cpu=0.0, memory=0.0, usb=0.0).
    /// Real metrics will be wired in Block I-1 integration.
    fn collect_metrics(&self) -> HrtEvent { ... }

    /// Emit a telemetry event through the IPC bridge.
    fn emit(&self, event: TelemetryEvent) { ... }
}

/// Handle for sending commands and reading status.
pub struct HrtHandle {
    cmd_tx: mpsc::Sender<HrtCommand>,
    status_rx: watch::Receiver<HrtStatus>,
}

impl HrtHandle {
    /// Send a command to the HRT service.
    pub async fn send(&self, cmd: HrtCommand) -> IrisResult<()> { ... }

    /// Get current HRT status.
    pub fn status(&self) -> HrtStatus { ... }

    /// Subscribe to status changes.
    pub fn subscribe_status(&self) -> watch::Receiver<HrtStatus> { ... }
}
```

### Key Design Points

1. **Telemetry bridge**: Every HrtEvent detected by the service is converted to the
   corresponding TelemetryEvent from iris-ipc and emitted via the broadcast channel.
   This is the same pattern as BSR's bsr-hrt.

2. **USB bandwidth watchdog**: Each health tick checks USB bandwidth. If above
   threshold, emit both HrtEvent::UsbBandwidthWarning locally AND
   TelemetryEvent::UsbBandwidthWarning through IPC.

3. **Placeholder metrics**: collect_metrics() returns zeroed values for now. Block I-1
   will wire real system metrics (sysinfo crate or Windows API).

4. **Graceful shutdown**: The run loop must exit cleanly on Shutdown command. Do not
   panic. Drop all channels on exit.

Unit Tests
----------
File: crates/iris-hrt/tests.rs

### Required Tests

1. `test_hrt_start_stop` — create service, send Start, verify status = Monitoring, send Stop, verify status = Stopped
2. `test_hrt_health_tick` — start service, wait for at least one health tick telemetry event
3. `test_hrt_force_check` — send ForceCheck, verify immediate telemetry emission
4. `test_hrt_set_interval` — send SetInterval, verify the tick rate changes
5. `test_hrt_shutdown` — send Shutdown, verify service task completes
6. `test_hrt_usb_bandwidth_warning` — mock metrics with bandwidth > threshold, verify UsbBandwidthWarning event emitted
7. `test_hrt_status_watch` — subscribe to status, send commands, verify status transitions

Acceptance Criteria
-------------------
1. `cargo check -p iris-hrt` passes
2. `cargo test -p iris-hrt` — all 7 tests pass
3. HRT service runs as an async task, does not block
4. Telemetry events are properly bridged to iris-ipc broadcast
5. Health check interval is configurable
6. USB bandwidth warning fires when threshold exceeded
7. Clean shutdown on command
