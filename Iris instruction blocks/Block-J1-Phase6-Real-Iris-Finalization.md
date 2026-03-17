# Block J-1 — Phase 6: Real Iris Finalization & Assisted Human Testing Checklist

## Objective

Remove every remaining mock, stub, and placeholder from the Iris production path so
that the application captures real USB webcam frames, reports live status, responds
to device selection, and survives a human tester sitting in front of the machine
and exercising every control.  After this block:

- `cargo run -p iris-ui --release` opens a window that enumerates and captures from
  a physical USB webcam without requiring any environment variable override.
- The status panel shows real fps, real frame count, and real device name.
- Selecting a camera in the UI actually switches the live capture source.
- The window title no longer says "Mock UI".
- All 59 (or more) workspace tests continue to pass with zero warnings.

---

## Prerequisites

- Phase-5 committed and tagged (already done — local tag `phase-5`).
- A physical USB webcam plugged in to the development machine.
- Rust stable toolchain, `cargo` available.
- Windows 10/11 (WMF APIs required for items 1–3).
- `windows` crate v0.54 with features
  `Win32_Media_MediaFoundation` and `Win32_System_Com` already present in
  `crates/iris-hal/Cargo.toml`.

---

## Gap Inventory (what remains from the Phase-5 audit)

| # | File | Issue | Priority |
|---|------|-------|----------|
| 1 | `crates/iris-hal/wmf_backend.rs` | `open_device`, `read_frame`, `set_control`, `close_device` all return `HalError::NotImplemented` | **Critical** |
| 2 | `crates/iris-ui/bootstrap.rs` | Capture backend defaults to `MockCaptureBackend`; `IRIS_BACKEND=dxgi` env var required to get real frames | **Critical** |
| 3 | `crates/iris-ui/bootstrap.rs` | `SelectDevice` handler is a no-op — returns `Empty` and does nothing | **High** |
| 4 | `crates/iris-ui/bootstrap.rs` | `GetStatus` always returns `"Mock Camera"`, `fps: 0.0`, `frame_count: 0` | **High** |
| 5 | `crates/iris-ui/ui_app.rs` | Header label still reads `"Iris — Mock UI"` | **Low / Trivial** |
| 6 | `crates/iris-hal/hotplug.rs` | `HotplugMonitor::run()` is a stub — no real OS device-change watcher | **Medium** |
| 7 | `crates/iris-hrt/service.rs:127` | "placeholder zeroed metrics" — USB bandwidth / health never computed | **Low** |

---

## Part 1 — WMF `open_device` and `read_frame`

### 1-A  Implement `WmfUvcBackend::open_device`

File: `crates/iris-hal/wmf_backend.rs`

The `open_device` method must:

1. Call `MFCreateSourceReaderFromMediaSource` (or `MFCreateSourceReaderFromURL`
   with the device's symbolic link) to get an `IMFSourceReader`.
2. Choose the best matching media type (prefer `MFVideoFormat_NV12` or
   `MFVideoFormat_YUY2`; fall back to whatever the device offers).
3. Store the `IMFSourceReader` in a new `open_reader` field inside
   `WmfUvcBackend` wrapped in `Option<Arc<Mutex<...>>>`.

Skeleton:

```rust
use windows::Win32::Media::MediaFoundation::{
    MFCreateSourceReaderFromMediaSource, IMFSourceReader,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, GUID,
};

pub fn open_device(&mut self, id: &DeviceId) -> HalResult<()> {
    // Find the IMFActivate for this device id from a fresh enumerate call,
    // then call ActivateObject to get an IMFMediaSource, then
    // MFCreateSourceReaderFromMediaSource.
    let activate = self.find_activate(id)?;
    let source: IMFMediaSource = unsafe { activate.ActivateObject()? };
    let reader: IMFSourceReader = unsafe {
        MFCreateSourceReaderFromMediaSource(&source, None)?
    };
    self.reader = Some(Arc::new(Mutex::new(reader)));
    Ok(())
}
```

### 1-B  Implement `WmfUvcBackend::read_frame`

```rust
pub fn read_frame(&mut self) -> HalResult<RawFrame> {
    let reader = self.reader.as_ref().ok_or(HalError::NotOpen)?;
    let reader = reader.lock().unwrap();
    let mut flags = 0u32;
    let mut timestamp = 0i64;
    let mut sample = None;
    unsafe {
        reader.ReadSample(
            MF_SOURCE_READER_FIRST_VIDEO_STREAM.0,
            0,
            None,
            Some(&mut flags),
            Some(&mut timestamp),
            Some(&mut sample),
        )?;
    }
    let sample = sample.ok_or(HalError::NoFrame)?;
    // Lock the media buffer and copy bytes into Vec<u8>.
    let buffer = unsafe { sample.GetBufferByIndex(0)? };
    let mut data_ptr = std::ptr::null_mut();
    let mut max_len = 0u32;
    let mut cur_len = 0u32;
    unsafe { buffer.Lock(&mut data_ptr, Some(&mut max_len), Some(&mut cur_len))?; }
    let bytes = unsafe { std::slice::from_raw_parts(data_ptr as *const u8, cur_len as usize) }
        .to_vec();
    unsafe { buffer.Unlock()?; }
    Ok(RawFrame { data: bytes, timestamp_us: (timestamp / 10) as u64 })
}
```

### 1-C  Wire WMF as the default capture backend on Windows

File: `crates/iris-ui/bootstrap.rs`

Replace the `backend_name == "dxgi"` block so that Windows automatically uses WMF
when no env override is set, and DXGI remains available via `IRIS_BACKEND=dxgi`:

```rust
// Priority: explicit env > WMF (Windows default) > mock
let boxed_backend: Box<dyn CaptureBackend + Send + Sync> = match backend_name.as_str() {
    "dxgi" => {
        #[cfg(windows)]
        { Box::new(iris_capture::DxgiCaptureBackend::new(capture_cfg.clone())) }
        #[cfg(not(windows))]
        { Box::new(iris_capture::backend::MockCaptureBackend::new(capture_cfg.clone())) }
    }
    "wmf" | "" => {
        #[cfg(windows)]
        {
            // Try to open the first WMF device; fall back to mock if none found.
            let mut wmf = iris_hal::wmf_backend::wmf::WmfUvcBackend::new();
            let devices = wmf.enumerate_sync().unwrap_or_default();
            if let Some(first) = devices.first() {
                if wmf.open_device(&first.id).is_ok() {
                    Box::new(iris_capture::WmfCaptureBackend::new(wmf, capture_cfg.clone()))
                } else {
                    Box::new(iris_capture::backend::MockCaptureBackend::new(capture_cfg.clone()))
                }
            } else {
                Box::new(iris_capture::backend::MockCaptureBackend::new(capture_cfg.clone()))
            }
        }
        #[cfg(not(windows))]
        { Box::new(iris_capture::backend::MockCaptureBackend::new(capture_cfg.clone())) }
    }
    _ => Box::new(iris_capture::backend::MockCaptureBackend::new(capture_cfg.clone())),
};
```

Add a corresponding `WmfCaptureBackend` wrapper in `crates/iris-capture/src/wmf.rs` that
implements `CaptureBackend` by delegating `next_frame()` to `WmfUvcBackend::read_frame()`.

---

## Part 2 — Wire `SelectDevice` in the Dispatcher

File: `crates/iris-ui/bootstrap.rs`

`IrisDispatcher` needs access to a shared `selected_device: Arc<Mutex<Option<DeviceId>>>` and
a `CaptureHandle` so it can stop the current capture, switch the device, and restart.

1. Add `selected_device: Arc<Mutex<Option<String>>>` to `IrisRuntime` and pass it into
   `IrisDispatcher`.
2. Replace the no-op handler:

```rust
IpcCommand::SelectDevice { device_id } => {
    // Stop current capture.
    let _ = cmd_sender.send(CaptureCommand::Stop).await;
    // Record the selected device id so bootstrap can re-open WMF on the right device.
    let mut lock = selected_device.lock().unwrap();
    *lock = Some(device_id.clone());
    drop(lock);
    // Restart capture on the new device.
    let _ = cmd_sender.send(CaptureCommand::Resume).await;
    IpcResponse::Ok(ResponseData::Empty)
}
```

3. In the UI (`ui_app.rs`), ensure the `Select` button sends
   `IpcCommand::SelectDevice { device_id: entry.id.clone() }` via the IPC handle.

---

## Part 3 — `GetStatus` reads live `AppState`

File: `crates/iris-ui/bootstrap.rs`

Replace the hardcoded mock values with real reads from `AppState`:

```rust
IpcCommand::GetStatus => {
    let state = app_state_ref.capture_state();
    let status = ResponseData::Status {
        capture_state: format!("{:?}", state),
        device_name: app_state_ref.device_name().unwrap_or_else(|| "None".into()),
        fps: app_state_ref.current_fps(),
        frame_count: app_state_ref.frame_count(),
        subscriber_count: app_state_ref.subscriber_count(),
    };
    IpcResponse::Ok(status)
}
```

`IrisDispatcher` must hold an `Arc<AppState>` clone (already available from `IrisRuntime`).
Ensure `AppState` exposes `device_name() -> Option<String>`, `current_fps() -> f32`,
`frame_count() -> u64`, and `subscriber_count() -> usize`. Add or verify these methods in
`crates/iris-core/app.rs`.

---

## Part 4 — Remove the "Mock UI" label

File: `crates/iris-ui/ui_app.rs`, find:

```rust
ui.label("Iris — Mock UI");
```

Replace with:

```rust
ui.label("Iris");
```

---

## Part 5 — Hotplug watcher (Windows `WM_DEVICECHANGE`)

File: `crates/iris-hal/hotplug.rs`

Implement `HotplugMonitor::run()` using a dedicated Win32 message-loop thread that
registers for `DBT_DEVTYP_DEVICEINTERFACE` notifications on the USB device class GUID
(`{A5DCBF10-6530-11D2-901F-00C04FB951ED}`).

High-level steps:

1. Spawn a `std::thread` (not a Tokio task) to own the Win32 message pump.
2. Inside the thread:
   - Create a message-only window (`CreateWindowExW` with `HWND_MESSAGE` parent).
   - Call `RegisterDeviceNotificationW` with `DEVICE_NOTIFY_WINDOW_HANDLE` for the
     USB device interface class.
   - Run `GetMessageW` / `DispatchMessageW` loop.
   - On `WM_DEVICECHANGE` with `DBT_DEVICEARRIVAL` or `DBT_DEVICEREMOVECOMPLETE`,
     send a `HotplugEvent` through the `mpsc::UnboundedSender<HotplugEvent>`.
3. Subscribe to `HotplugHandle` in the bootstrap and forward arrival/removal events to
   the UI log so the device list auto-refreshes without pressing `Detect Cameras`.

```rust
// crates/iris-hal/hotplug.rs  (Windows path)
#[cfg(windows)]
pub fn start_hotplug_thread(tx: mpsc::UnboundedSender<HotplugEvent>) {
    std::thread::spawn(move || {
        // Win32 message-only window + RegisterDeviceNotificationW + message pump
        // Send HotplugEvent::Arrived / Removed through `tx` on WM_DEVICECHANGE.
        unsafe { win32_hotplug_loop(tx); }
    });
}
```

---

## Part 6 — HRT real metrics (optional / deferred)

File: `crates/iris-hrt/service.rs`

The placeholder at line 127 emits zeroed USB bandwidth and health values.
Replace with actual measurements sourced from the running `CaptureHandle`:

- Query frames delivered / dropped from `CaptureHandle::stats()`.
- Compute rolling fps from a `VecDeque<Instant>` of the last N frame timestamps.
- Expose via the existing `HrtHandle` telemetry path so Prometheus picks them up.

This part may be deferred to Phase 7 if time is constrained; it does not block
functional camera capture.

---

## Build & Verification Steps

Run these in order from the workspace root (`C:\Users\Baxter\Desktop\Iris`).

### Step 1 — Zero-warning build

```powershell
cargo build --workspace 2>&1 | Select-String "warning::|error::"
```

Expected: no output (zero warnings, zero errors).

### Step 2 — All tests pass

```powershell
cargo test --workspace 2>&1 | Select-String "warning|error|FAILED|test result"
```

Expected: all lines end with `test result: ok`, no `FAILED`, no `warning`.

### Step 3 — Real camera enumeration smoke test

```powershell
cargo run -p iris-ui --release
```

- The `Detect Cameras` button (keyboard shortcut `R`) should list your physical
  USB webcam by its real name (e.g. `HD Pro Webcam C920`), not `Mock Camera`.

### Step 4 — Live frame preview

- After enumeration, click `Select` next to the real camera entry.
- Click `Start Capture`.
- The preview pane should show a live image from the webcam within 2 seconds.
- The status panel fps counter should be non-zero (e.g. `29.7 fps`).

### Step 5 — Window title

- Verify the title bar reads `Iris` (not `Iris — Mock UI`).

### Step 6 — Stop and re-select

- Click `Stop Capture`.
- Change the selected device to a different entry (or the same) and click `Start Capture` again.
- Verify the preview resumes without a crash.

### Step 7 — Unplug / re-plug hotplug (after Part 5 is done)

- Unplug the USB webcam.
- Verify the device list updates automatically (without pressing `Detect Cameras`).
- Re-plug the webcam; verify it reappears in the list within ~2 seconds.

---

## Human Testing Checklist

Use this checklist during a supervised test session. Mark each item pass/fail and
note any error messages or unexpected behavior.

### Environment setup
- [ ] Rust stable toolchain installed and `cargo --version` prints without error.
- [ ] Physical USB webcam plugged in before launching Iris.
- [ ] No `IRIS_BACKEND`, `IRIS_UI_HEADLESS`, or other override env vars set.
- [ ] Previous Iris processes not running (`Get-Process iris-ui` returns nothing).

### Launch
- [ ] `cargo run -p iris-ui --release` completes with zero compile errors.
- [ ] Window opens with title **Iris** (not "Mock UI").
- [ ] Window size is reasonable (no collapsed or oversized layout).
- [ ] Dark background theme applied (charcoal, not default egui grey).

### Camera detection
- [ ] Press `R` or click `Detect Cameras` button.
- [ ] Real camera name appears in device list (not "Mock Camera").
- [ ] Camera entry is highlighted in green with a `●` bullet.
- [ ] Telemetry / log panel shows a line like `ListDevices: WMF found 1 device(s)`.

### Capture start
- [ ] Click `Select` next to the real camera.
- [ ] Click `Start Capture` (or press `S`).
- [ ] Preview pane shows a live image from the webcam within 2 seconds.
- [ ] Status panel `fps` counter shows a non-zero value.
- [ ] `frame_count` in status panel increases over time.
- [ ] `device_name` in status panel matches the real camera name (not "Mock Camera").

### Capture stop
- [ ] Click `Stop Capture` (or press `S` again).
- [ ] Preview pane stops updating (last frame "frozen" or cleared).
- [ ] `fps` returns to 0.0 in status panel.
- [ ] No crash or panic visible in the terminal.

### Re-select and restart
- [ ] With capture stopped, click `Select` on the same device again.
- [ ] Click `Start Capture`.
- [ ] Preview resumes without error.

### Hotplug (if Part 5 implemented)
- [ ] While running, unplug the USB webcam.
- [ ] Device list auto-updates and shows the camera gone (no `Detect Cameras` click needed).
- [ ] Log panel shows a hotplug-removal event.
- [ ] Re-plugging the webcam causes it to reappear in the list within ~2 seconds.
- [ ] Clicking `Select` then `Start Capture` resumes capture after re-plug.

### Error resilience
- [ ] Launch Iris with no camera plugged in.
- [ ] `Detect Cameras` shows an empty list or a clear "no cameras found" message.
- [ ] No crash.  Plugging in the camera and pressing `R` recovers.

### Telemetry
- [ ] While capturing, tail the log panel.  Verify telemetry lines appear at ~1 Hz.
- [ ] Open `http://localhost:9090/metrics` (or whichever port Iris binds) in a browser.
- [ ] Verify `iris_encoder_rebase_total` and capture-related counters are present and
  incrementing.

### Cleanup
- [ ] Close the Iris window (× button or `Alt+F4`).
- [ ] Terminal returns to prompt cleanly (no hang, no unhandled panic).
- [ ] `Get-Process iris-ui` returns nothing.

---

## Commit Sequence (suggested)

```
feat(hal): WMF open_device + read_frame — real USB frame capture
feat(capture): WmfCaptureBackend wrapper implementing CaptureBackend
feat(ui/bootstrap): remove MockCaptureBackend default; wire WMF on Windows
feat(ui/bootstrap): SelectDevice switches active capture device
feat(ui/bootstrap): GetStatus reads live AppState (fps, frame count, device name)
fix(ui): remove "Mock UI" label from window header
feat(hal/hotplug): Win32 WM_DEVICECHANGE watcher — auto-refresh on plug/unplug
```

Each commit should be followed by `cargo test --workspace` to confirm the green build
is preserved at every step.

---

## Exit Criteria for Phase 6

Phase 6 is complete when:

1. A physical USB webcam streams live frames into the Iris UI preview pane without any
   environment variable override.
2. The status panel shows real fps, device name, and frame count.
3. `SelectDevice` stops and restarts capture on the chosen device.
4. Window title reads `Iris`.
5. `cargo test --workspace` — 0 failures, 0 warnings.
6. Every checkbox in the Human Testing Checklist above is marked pass.
7. Changes committed with the messages above and tagged `phase-6`.
