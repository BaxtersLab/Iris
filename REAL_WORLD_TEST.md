# Real‑World Test Plan — Iris

Goal: Get Iris running with a real camera backend and validate end‑to‑end capture, telemetry (`size_bytes`), ROI behavior, and UI presentation so you can perform real‑world tests.

Prerequisites
- Install device drivers required by your camera (platform specific).
- Grant permissions (Windows: camera access for the user account). Ensure no other app holds the device.
- Have Rust toolchain installed (stable) and `cargo` on PATH.

Quick commands
- Build (debug):

```powershell
cargo build -p iris-ui
```

- Build (release):

```powershell
cargo build -p iris-ui --release
```

- Headless run (capture logs):

```powershell
$env:RUST_LOG = "debug"
$env:IRIS_UI_HEADLESS = "1"
$env:IRIS_USE_HW = "1"   # set if codebase reads this to enable real backend
cargo run -p iris-ui > iris-ui-real-headless.log 2>&1
```

- Interactive run:

```powershell
$env:RUST_LOG = "info"
cargo run -p iris-ui
```

Enable hardware backend
- Edit your runtime config (if present) to select a real backend instead of `mock`. Example flags/file: `config.toml` or env `IRIS_BACKEND=hardware`.
- If the project uses feature flags, enable the hardware feature in the `Cargo.toml` invocation or ensure the backend crate is compiled.

Validation checklist (pass/fail criteria)
- Device enumeration: `ListDevices` shows the real camera entry (vendor & model present).
- Capture: `StartCapture` succeeds and `fps > 0` in status.
- Telemetry: `FrameCaptured.size_bytes > 0` on each frame and equals `frame.data.len()` in capture backend unit tests.
- UI: The app shows live frames without panics; interactive ROI changes reduce `size_bytes` as expected.
- Stability: No crashes, memory leaks, or repeated dropped frames beyond acceptable threshold.

Smoke tests (manual steps)
1. Run interactive UI. Confirm device appears in device list and `Start` begins streaming.
2. Toggle ROI on/off; observe `size_bytes` in telemetry or status and verify it shrinks when ROI active.
3. Change resolution/format; verify `size_bytes` updates accordingly and frame displays correctly.
4. Run headless capture for ~30s and inspect `iris-ui-real-headless.log` for repeated `FrameCaptured` events and no errors.

Hardware integration tests (automated)
- Run `cargo test -p iris-capture` to run unit tests that assert `size_bytes` equals calculated `expected_size`.
- Consider adding a short integration test that opens the real device, captures N frames, and asserts `frame.data.len() == size_bytes` (requires hardware access in CI or local run).

Troubleshooting notes
- Device busy: close other camera apps, reboot if driver stuck.
- Permission denied: check Windows privacy camera settings and run with elevated permissions if necessary.
- Pixel format mismatch: if UI shows corruption, verify pixel format and expected size calculation (RGB24 vs NV12 etc.).
- If `size_bytes` is zero, ensure the backend populates `CaptureFrame.data` before emitting telemetry.

Acceptance criteria for "functional for real‑world testing"
- Reproducible: device enumerates and captures on multiple runs.
- Telemetry: `size_bytes` authoritative and matches actual frame bytes for >99% of frames in a 30s run.
- UI: Frames displayed at >= 5 FPS for the device capability; ROI toggling behaves as documented.
- Tests: Unit/integration tests that require hardware run locally and pass.

Next steps I can take now
- Try enabling the hardware backend in config and run a short device enumeration (requires physical camera connected and permissions).
- Add/prepare an automated integration test that runs only when `IRIS_USE_HW=1` is set.

If you want me to proceed now, confirm whether I should try to enable the hardware backend and run a short enumeration (`ListDevices`).
