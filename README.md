# Iris — 4K Vision Interface

A stand-alone, deterministic, telemetry-rich Rust subsystem providing
high-fidelity visual input to agentic systems (the RemoteDexter ecospace).
USB camera capture, zero-inference, full outbound telemetry.

Iris is a tool, not a brain.

## Crates

- iris-core: config, app state, logging, pipeline metrics (/metrics)
- iris-ipc: commands, responses, telemetry, JSON envelope
- iris-hrt: health, runtime, thermal monitoring
- iris-hal: UVC hardware abstraction — WMF backend (Windows), V4L2 backend (Linux), mock
- iris-capture: capture pipeline — CaptureService, UvcCaptureBackend adapter, DXGI screen backend (Windows)
- iris-control: camera controls, capability queries, profiles
- iris-stream: multi-subscriber output, ring buffer, IPC delivery
- iris-ui: the camera app (egui, charcoal theme) + bootstrap runtime
- iris-harness: test-stream generator (ffmpeg MPEG-TS / deterministic JSONL)
- iris-ipc-pipe-bridge (standalone): JSON-lines envelope bridge — named pipe (Windows) / unix socket (Linux)

## Prerequisites

Rust (stable) plus a few native dependencies. **`DEPENDENCIES.md` has the
per-platform detail** — the short version:

- **Windows** — FFmpeg *development libraries* (vcpkg is the recommended route)
  plus the MSVC build tools and CMake, plus `LIBCLANG_PATH` for bindgen.
- **Linux** — nothing extra to *build*. To run the **test suite** you need the
  `ffmpeg` **CLI**, which `iris-harness` shells out to:
  `sudo apt-get install -y ffmpeg`. Camera access uses V4L2 via `uvcvideo`,
  already in-kernel on mainstream distributions.

Those two are different things and neither implies the other: Windows needs the
FFmpeg *libraries* at link time, Linux needs the *binary* at test time.

```sh
git clone https://github.com/BaxtersLab/Iris
cd Iris
cargo build --release
```

## Run

```sh
cargo run -p iris-ui                     # mock backend (default)
IRIS_BACKEND=wmf  cargo run -p iris-ui   # real camera via Windows Media Foundation
IRIS_BACKEND=v4l2 cargo run -p iris-ui   # Linux V4L2 (enumeration/probe today)
IRIS_BACKEND=dxgi cargo run -p iris-ui   # Windows screen capture
IRIS_UI_HEADLESS=1 ...                   # scripted headless run (no window)
IRIS_DEVICE=<id-or-name-substring>       # pick a specific camera
```

## Test

```sh
cargo test --workspace                # no camera needed; requires the ffmpeg CLI
IRIS_USE_HW=1 cargo test -p iris-hal  # hardware-gated: real camera enumerate + capture
```

## Status

Windows: **finished v1 loop** — enumerate → open → real frames →
authoritative telemetry (frame-true resolution/format/size) → live NV12/YUYV
preview in the UI. Verified against real hardware (USB camera, 1920×1080 NV12).
Linux: builds and tests clean on Ubuntu 26.04 — `cargo test --workspace` is
60/60 with zero warnings. V4L2 device enumeration and format probing are
implemented; full V4L2 frame capture is the next block. See `ROADMAP.md`.

## License

MIT
