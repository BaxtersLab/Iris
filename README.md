# Iris — Running FFmpeg-dependent Tests

This project contains several integration tests that rely on FFmpeg being
available on the PATH. To avoid flaky CI runs these tests are gated — only
executed when FFmpeg is present and an explicit environment flag is set.

How to run the FFmpeg tests locally

- Ensure `ffmpeg` is installed and available on your PATH.
  - macOS: `brew install ffmpeg`
  - Ubuntu: `sudo apt-get install -y ffmpeg`
  - Windows (Chocolatey): `choco install ffmpeg -y`

- To run all tests (including FFmpeg-gated ones) locally set either
  `RUN_FFMPEG_INTEGRATION=1` or `CI_FFMPEG=1` in your environment before
  running `cargo test`.

Examples

- Bash / CI runner:

```bash
export RUN_FFMPEG_INTEGRATION=1
cargo test --all
```

- PowerShell (Windows):

```powershell
$env:RUN_FFMPEG_INTEGRATION = '1'
cargo test --all
```

- To run a single FFmpeg integration test (keeps run time small):

```bash
export RUN_FFMPEG_INTEGRATION=1
cargo test -p iris-capture ffmpeg_integration_end_to_end_pts_keyframe -- --exact --nocapture
```

CI Notes

- The included GitHub Actions workflow (`.github/workflows/ci.yml`) installs
  FFmpeg on the runner and writes `CI_FFMPEG=1` to `GITHUB_ENV` only if the
  `ffmpeg` binary is available. This means the FFmpeg-gated tests will run
  automatically on runners where FFmpeg was successfully installed.

Tips to reduce flakiness

- If CI runners are slow or variable, increase the integration test timeouts
  (tests in `crates/iris-capture/tests/*`) or run only the single targeted
  ffmpeg test as shown above.
- For a fully deterministic CI-friendly approach, consider using the
  `crates/iris-harness` (Phase‑3) deterministic replay harness instead of
  running FFmpeg on CI; FFmpeg tests can remain gated for specialized runners.

If you want me to add instructions to project docs elsewhere, or create a
small CI snippet for your CI provider, tell me where and I'll add it.
# Phase‑4 (deterministic CI and harness)

We're entering Phase‑4 to make CI deterministic and reduce reliance on system
FFmpeg. Planned next steps:

- Scaffold `crates/iris-harness` to provide deterministic encoded-stream
  replays that the test-suite can consume without FFmpeg.
- Add a CI harness step that runs deterministic harness tests on all runners
  (falling back to FFmpeg-gated tests only when the harness is not present).
- Collect `/metrics` output and test artifacts from CI runs for post-mortem
  analysis.

To run the harness locally (once added):

```bash
# from workspace root
cargo test -p iris-harness -- --nocapture
```

If you'd like, I can scaffold `crates/iris-harness` now and add the CI step
to `.github/workflows/ci.yml` so Phase‑4 runs automatically.
# Iris — 4K Vision Interface

A stand-alone, deterministic, telemetry-rich Rust subsystem providing high-fidelity
visual input to agentic systems. USB 4K webcam capture, zero-inference, full
outbound telemetry.

Iris is a tool, not a brain.

## Crates
- iris-core: config, app state, logging
- iris-ipc: commands, responses, telemetry, JSON envelope
- iris-hrt: health, runtime, thermal monitoring
- iris-hal: USB UVC hardware abstraction, device enumeration
- iris-capture: async frame pipeline, FPS pacing, ROI
- iris-control: camera controls, capability queries, profiles
- iris-stream: multi-subscriber output, ring buffer, IPC delivery
- iris-ui: egui viewer, charcoal theme, diagnostics

## Build
```
cargo build --release
```

## License
MIT
