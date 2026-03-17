# iris-ui headless run summary

- Path: `iris-ui-headless.log` (workspace root)
- Command run: `RUST_LOG=debug IRIS_UI_HEADLESS=1 cargo run -p iris-ui > iris-ui-headless.log 2>&1`
- Outcome: Headless session completed. Mock device enumerated, subscribed, capture started and stopped.
- Observed telemetry: repeated `FrameCaptured` events for 3840×2160 with `size_bytes = 24883200` (example):

```
TELEMETRY: TelemetryEnvelope { timestamp: 2026-03-16T05:00:09.293934700Z, sequence: 0, event: FrameCaptured { sequence: 1, width: 3840, height: 2160, size_bytes: 24883200 } }
... (many subsequent frames with identical `size_bytes`)
```

- Session steps: ListDevices → Subscribe → StartCapture → many FrameCaptured events → StopCapture → Unsubscribe → GetStatus

- Notes: `size_bytes` is the authoritative frame size (in bytes) from the capture backend; integration/unit tests verify telemetry correctness.

