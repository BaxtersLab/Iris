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

On Linux use the launcher. It scrubs the snap-contaminated environment a
terminal inside a snapped editor exports — eighteen variables measured on the
reference box, `LOCPATH` among them, which drags a host binary onto a different
glibc — and it reports which `iris.toml` was picked up:

```sh
./run.sh                                 # Linux launcher (does the env scrub)
```

Running the binary directly works too, and is what `run.sh` ends up doing:

```sh
cargo run -p iris-ui                     # mock backend (default)
IRIS_BACKEND=wmf  cargo run -p iris-ui   # real camera via Windows Media Foundation
IRIS_BACKEND=v4l2 cargo run -p iris-ui   # Linux V4L2 (enumeration/probe today)
IRIS_BACKEND=dxgi cargo run -p iris-ui   # Windows screen capture
IRIS_UI_HEADLESS=1 ...                   # scripted headless run (no window)
IRIS_DEVICE=<id-or-name-substring>       # pick a specific camera
```

### Configuration

`iris.toml` is read from **the directory containing the executable** — so
`target/release/iris.toml` for a release build, not the repository root.
`run.sh` prints the path it looked at. Anything missing or invalid falls back to
the built-in defaults, with the reason printed; `capture.width`,
`capture.height`, `capture.target_fps`, `capture.pixel_format`
(`rgb24`/`bgr24`/`nv12`/`yuyv`/`mjpeg`, with `yuy2` accepted for `yuyv`),
`capture.max_queue_depth` and `capture.drop_policy` (`oldest`/`newest`) all
drive capture.

## Test

```sh
cargo test --workspace                # no camera needed; requires the ffmpeg CLI
IRIS_USE_HW=1 cargo test -p iris-hal  # hardware-gated: real camera enumerate + capture
```

## Status

**Windows** — finished v1 loop: enumerate → open → real frames → authoritative
telemetry (frame-true resolution/format/size) → live NV12/YUYV preview in the
UI. Verified against real hardware (USB camera, 1920×1080 NV12).

**Linux** — at parity, verified against real hardware: full V4L2 streaming
capture (REQBUFS/QUERYBUF/mmap/QBUF/DQBUF/STREAMON plus the control ioctls) with
MJPEG decoded for both the preview and ROI cropping, proven at 1920×1080 @30 fps
MJPEG. Enumeration reports all of a camera's modes, not just the uncompressed
ones.

`cargo test --workspace` is **102 passing, 0 failing, zero build warnings** on
Ubuntu 26.04. Remaining declared work is in `ROADMAP.md`; the only open item is
a duplicate Windows Media Foundation backend, which needs a Windows box to
resolve.

## License

MIT
