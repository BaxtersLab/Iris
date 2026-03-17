Block I-1 — Integration & Validation
======================================

Objective
---------
Wire all 8 crates together into a functioning application. This block connects
every service, implements the IPC command dispatcher, completes stubs (logging,
hotplug, WMF backend basics), runs full lifecycle tests, validates error paths,
and produces a readiness report. After this block, `cargo run -p iris-ui` launches
a working Iris window that can enumerate mock devices, capture mock frames, display
them in the preview, and emit telemetry.

Prerequisites
-------------
ALL previous blocks (A-1 through H-2) must be complete.

Part 1: IPC Command Dispatcher
-------------------------------
In iris-ipc, the IpcServer's `run()` method needs a command dispatcher that routes
each IpcCommand to the appropriate service handle.

Create: crates/iris-ipc/dispatcher.rs

```rust
use super::command::IpcCommand;
use super::response::{IpcResponse, ResponseData};
use iris_core::app::AppState;
use iris_hal::backend::UvcBackend;
use iris_capture::service::CaptureHandle;
use iris_control::service::ControlHandle;
use iris_stream::service::StreamHandle;
use iris_hrt::service::HrtHandle;
use std::sync::Arc;
use std::time::Instant;

/// Central command dispatcher — routes IPC commands to service handles.
pub struct Dispatcher {
    pub app_state: Arc<AppState>,
    pub capture_handle: CaptureHandle,
    pub control_handle: ControlHandle,
    pub stream_handle: StreamHandle,
    pub hrt_handle: HrtHandle,
    start_time: Instant,
}

impl Dispatcher {
    pub fn new(
        app_state: Arc<AppState>,
        capture_handle: CaptureHandle,
        control_handle: ControlHandle,
        stream_handle: StreamHandle,
        hrt_handle: HrtHandle,
    ) -> Self {
        Self {
            app_state,
            capture_handle,
            control_handle,
            stream_handle,
            hrt_handle,
            start_time: Instant::now(),
        }
    }

    /// Dispatch a single command and return its response.
    pub async fn dispatch(&mut self, cmd: IpcCommand) -> IpcResponse {
        match cmd {
            // === Lifecycle ===
            IpcCommand::Ping => IpcResponse::Ok(ResponseData::Pong {
                uptime_ms: self.start_time.elapsed().as_millis() as u64,
            }),
            IpcCommand::GetStatus => {
                // Read from AppState
                IpcResponse::Ok(ResponseData::Status {
                    capture_state: format!("{:?}", self.app_state.capture_state()),
                    device_name: self.app_state.device_name(),
                    fps: self.app_state.current_fps(),
                    frame_count: self.app_state.frame_count(),
                    subscriber_count: self.app_state.subscriber_count(),
                })
            }
            IpcCommand::Shutdown => {
                // Signal all services to shut down
                let _ = self.hrt_handle.send(iris_hrt::event::HrtCommand::Shutdown).await;
                let _ = self.capture_handle.send(iris_capture::service::CaptureCommand::Stop).await;
                let _ = self.control_handle.shutdown().await;
                let _ = self.stream_handle.shutdown().await;
                self.app_state.set_capture_state(iris_core::app::CaptureState::ShuttingDown);
                IpcResponse::Ok(ResponseData::Empty)
            }

            // === Capture ===
            IpcCommand::StartCapture => {
                // Start capture service, update app state
                ...
            }
            IpcCommand::StopCapture => { ... }
            IpcCommand::PauseCapture => { ... }
            IpcCommand::ResumeCapture => { ... }
            IpcCommand::SetResolution { width, height } => { ... }
            IpcCommand::SetFps { fps } => { ... }
            IpcCommand::SetPixelFormat { format } => { ... }
            IpcCommand::SetRoi { x, y, width, height } => { ... }
            IpcCommand::ClearRoi => { ... }

            // === Device ===
            IpcCommand::ListDevices => { ... }
            IpcCommand::SelectDevice { device_id } => { ... }
            IpcCommand::GetDeviceCapabilities => { ... }
            IpcCommand::DisconnectDevice => { ... }

            // === Controls ===
            IpcCommand::GetControl { control } => { ... }
            IpcCommand::SetControl { control, value } => { ... }
            IpcCommand::ResetControl { control } => { ... }
            IpcCommand::ListControls => { ... }
            IpcCommand::LoadProfile { name } => { ... }
            IpcCommand::SaveProfile { name } => { ... }

            // === Stream ===
            IpcCommand::Subscribe => { ... }
            IpcCommand::Unsubscribe { subscriber_id } => { ... }
            IpcCommand::GetStreamStats => { ... }

            // === Config ===
            IpcCommand::Init { config_path } => { ... }
            IpcCommand::ReloadConfig => { ... }
            IpcCommand::GetConfig => { ... }
            IpcCommand::UpdateConfig { section, json } => { ... }
        }
    }
}
```

Implement every match arm. Each arm should:
1. Call the appropriate service handle method
2. Convert the result to IpcResponse::Ok or IpcResponse::Error
3. Update AppState where appropriate

Part 2: Application Bootstrap
-------------------------------
Create: crates/iris-ui/bootstrap.rs

This wires everything together at startup:

```rust
pub struct IrisRuntime {
    pub app_state: Arc<AppState>,
    pub ipc_handle: IpcHandle,
    pub capture_handle: CaptureHandle,
    pub control_handle: ControlHandle,
    pub stream_handle: StreamHandle,
    pub hrt_handle: HrtHandle,
}

impl IrisRuntime {
    /// Bootstrap all services and return the runtime.
    pub async fn bootstrap(config: IrisConfig) -> IrisResult<Self> {
        // 1. Create AppState
        let app_state = Arc::new(AppState::new());

        // 2. Create IPC server + handle
        let (ipc_server, ipc_handle) = IpcServer::new(256);

        // 3. Create HRT service
        let hrt_config = HrtConfig {
            interval_ms: 2000,
            usb_bandwidth_threshold: 0.85,
            thermal_threshold_c: 75.0,
        };
        let (hrt_service, hrt_handle) = HrtService::new(
            hrt_config,
            ipc_handle.telemetry_tx(),
        );

        // 4. Create HAL backend (mock for now)
        let mut backend = iris_hal::backend::MockUvcBackend::new();
        // Add a default mock device
        backend.add_device(
            DeviceInfo {
                id: DeviceId("mock-cam-1".into()),
                name: "Iris Mock Camera 4K".into(),
                vendor: "Iris".into(),
                bus_info: "USB#VID_0000".into(),
                in_use: false,
            },
            DeviceCapabilities {
                device_id: DeviceId("mock-cam-1".into()),
                formats: vec![
                    FormatDescriptor { width: 3840, height: 2160, fps: 30, pixel_format: PixelFormat::Bgra8 },
                    FormatDescriptor { width: 1920, height: 1080, fps: 60, pixel_format: PixelFormat::Bgra8 },
                    FormatDescriptor { width: 1280, height: 720, fps: 30, pixel_format: PixelFormat::Nv12 },
                ],
                controls: vec![
                    ControlCapabilityInfo { name: "brightness".into(), min: 0, max: 255, step: 1, default: 128, current: 128, auto_supported: false },
                    ControlCapabilityInfo { name: "exposure".into(), min: 1, max: 5000, step: 1, default: 250, current: 250, auto_supported: true },
                    ControlCapabilityInfo { name: "focus".into(), min: 0, max: 255, step: 5, default: 0, current: 0, auto_supported: true },
                ],
            },
        );

        // 5. Create CaptureService with mock backend
        let capture_config = CaptureConfig {
            width: config.capture.width,
            height: config.capture.height,
            target_fps: config.capture.target_fps,
            format: PixelFormat::Bgra8,
            max_queue_depth: config.capture.max_queue_depth,
            drop_policy: DropPolicy::from_str(&config.capture.drop_policy).unwrap_or(DropPolicy::Oldest),
            roi: None,
        };
        let mock_capture = MockCaptureBackend::new(capture_config.clone());
        let (capture_service, capture_handle) = CaptureService::new(
            mock_capture,
            capture_config,
            ipc_handle.telemetry_tx(),
        );

        // 6. Create ControlService
        let profiles_dir = std::env::current_exe()
            .unwrap_or_default()
            .parent()
            .unwrap_or(&std::path::Path::new("."))
            .join("profiles");
        let (control_service, control_handle) = ControlService::new(
            ipc_handle.telemetry_tx(),
            profiles_dir,
        );

        // 7. Create StreamService
        let (stream_service, stream_handle) = StreamService::new(
            capture_handle.frame_rx, // Wire capture output to stream input
            ipc_handle.telemetry_tx(),
            StreamMode::from_str(&config.stream.default_mode).unwrap_or(StreamMode::Pull),
            config.stream.ring_buffer_capacity,
            config.stream.max_subscribers,
        );

        // 8. Create Dispatcher
        let dispatcher = Dispatcher::new(
            app_state.clone(),
            capture_handle,
            control_handle.clone(),
            stream_handle.clone(),
            hrt_handle.clone(),
        );

        // 9. Spawn all services as Tokio tasks
        tokio::spawn(ipc_server.run_with_dispatcher(dispatcher));
        tokio::spawn(hrt_service.run());
        tokio::spawn(capture_service.run());
        tokio::spawn(control_service.run());
        tokio::spawn(stream_service.run());

        // 10. Emit SystemStarted telemetry
        ipc_handle.emit_telemetry(TelemetryEvent::SystemStarted {
            version: env!("CARGO_PKG_VERSION").into(),
        });

        Ok(Self {
            app_state,
            ipc_handle,
            capture_handle,
            control_handle,
            stream_handle,
            hrt_handle,
        })
    }
}
```

Part 3: Complete Stubs
----------------------

### iris-core/logging.rs
Replace the `todo!()` with a working implementation using tracing-subscriber.

### iris-ui/app.rs
Wire IrisApp to use IrisRuntime:
- Poll stream_handle for frames → update preview
- Poll telemetry broadcast → append to telemetry_log
- Wire button clicks to IPC commands (start/stop capture, select device, etc.)
- Wire control sliders to ControlHandle

### iris-hal/hotplug.rs
For now, implement basic polling (enumerate devices every 5 seconds, diff against
previous list, emit HotplugEvent::Connected/Disconnected). Real WMI-based hotplug
can come later.

Part 4: Integration Tests
--------------------------
Create: tests/integration_tests.rs (workspace-level tests directory)

### Required Tests

1. `test_full_lifecycle` — bootstrap → start capture → receive 10 frames → stop capture → shutdown
2. `test_ping_pong_via_ipc` — bootstrap → send Ping → receive Pong with uptime > 0
3. `test_device_enumeration` — bootstrap → ListDevices → verify mock device in list
4. `test_capture_start_stop_telemetry` — start capture → verify CaptureStarted event → stop → verify CaptureStopped event
5. `test_stream_subscribe_receive` — subscribe → start capture → receive frames via subscription → unsubscribe
6. `test_control_set_get` — set brightness=200 → get brightness → verify 200
7. `test_profile_save_load` — set controls → save profile "test" → change controls → load profile "test" → verify restored
8. `test_hrt_health_events` — start HRT → wait for HealthCheck telemetry → stop
9. `test_shutdown_graceful` — bootstrap → shutdown → verify all services stopped (no hanging tasks)
10. `test_error_path_no_device` — attempt capture without selecting device → verify error response
11. `test_concurrent_subscribers` — subscribe 3 clients → start capture → all 3 receive frames → unsubscribe all
12. `test_config_reload` — bootstrap → modify config → ReloadConfig → verify new values applied

Part 5: Readiness Report
--------------------------
At the end of this block, create: docs/READINESS.md

Content:
```markdown
# Iris Readiness Report

## Build Status
- [ ] `cargo build --release` passes
- [ ] `cargo test` — all unit tests pass (X total)
- [ ] `cargo test` with `--features compressed-ipc,websocket` passes
- [ ] Integration tests pass (12 tests)

## Feature Checklist
- [ ] Config system: load/save/validate
- [ ] IPC protocol: all commands, all responses, 30 telemetry events
- [ ] HRT monitoring: health ticks, USB bandwidth watchdog
- [ ] HAL: mock backend functional, WMF stub compiles
- [ ] Capture: frame pipeline, FPS pacing, drop policy, ROI
- [ ] Controls: get/set/auto/profiles
- [ ] Stream: Pull/Push/SharedMemory modes, ring buffer, multi-subscriber
- [ ] UI: charcoal theme, preview, controls, telemetry, diagnostics, status bar
- [ ] Compressed IPC: MJPEG encode, passthrough (feature flag)
- [ ] WebSocket bridge: telemetry forwarding (feature flag)
- [ ] Command dispatcher: all routes implemented
- [ ] Graceful shutdown: all services clean exit

## Known Limitations
- WMF backend is stub-only (all methods return "not yet implemented")
- H.264 compressor is stub-only
- Hotplug uses polling, not native WMI events
- WebSocket bridge has no authentication
- Real system metrics (CPU, memory, USB bandwidth) are placeholder zeros

## Next Steps
- Implement WMF backend for real USB webcam capture
- Add H.264 encoding via OpenH264 or x264
- Native WMI-based hotplug events
- WebSocket authentication (token-based)
- Installer packaging (Inno Setup)
- System tray integration
```

Acceptance Criteria
-------------------
1. `cargo build --release` passes with zero errors
2. `cargo test` — ALL unit tests across all 8 crates pass
3. `cargo test --features compressed-ipc,websocket` — all feature-gated tests pass
4. All 12 integration tests pass
5. `cargo run -p iris-ui` launches and shows:
   - Charcoal-themed window
   - Mock camera in device list
   - Clicking "Start Capture" shows synthetic frames in preview
   - Telemetry events appear in telemetry panel
   - Diagnostics update in real-time
   - Control sliders adjust mock values
6. Graceful shutdown: closing the window stops all services, no panics
7. docs/READINESS.md is populated with actual results
8. No compiler warnings (or only minor ones from dependencies)
