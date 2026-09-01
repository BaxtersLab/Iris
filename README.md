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
- iris-control: camera controls by name, capability validation, named profiles,
  and a serialising `ControlService`. Profiles are keyed by control **name**, so
  one saved on Linux applies on Windows where the ids differ entirely.
  **Not yet wired into the UI** — see `ROADMAP.md`
- iris-stream: frame fan-out to several consumers — `Push` (a bounded channel
  per subscriber, so a slow one drops only its own frames) and `Pull` (a ring of
  recent frames). `SharedMemory` and `Ipc` are named but **refused**, not
  silently substituted. **Not yet wired into the UI** — see `ROADMAP.md`
- iris-ui: the camera app (egui, charcoal theme) + bootstrap runtime
- iris-harness: test-stream generator (ffmpeg MPEG-TS / deterministic JSONL)
- iris-ipc-pipe-bridge (standalone): JSON-lines envelope bridge — named pipe
  (Windows) / unix socket (Linux). **Outside the Cargo workspace**, so
  `cargo build --workspace` and `cargo test --workspace` do not cover it; build
  it with `--manifest-path crates/iris-ipc-pipe-bridge/Cargo.toml`

## A note on `Iris instruction blocks/`

That folder holds the **March 2026 build instructions**, kept as a record of how
Iris was specified. They are phrased as objectives and read like a to-do list;
they are not one — see the README inside it. `ROADMAP.md` is the only
authoritative statement of what is not yet built.

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
Ubuntu 26.04, and **91 passing, 0 failing, zero warnings** on Windows 10 — the
difference is 14 Linux-only tests (the V4L2 backend and its hardware paths)
against 2 Windows-only ones.

**Camera controls work on both platforms** — V4L2 `G_CTRL`/`S_CTRL`/`QUERYCTRL`
on Linux (11 controls on the reference camera), `IAMVideoProcAmp` and
`IAMCameraControl` on Windows (10 on the same camera, through a different API).
`control_id` is platform-defined; read `list_controls`, which reports id, name
and real min/max/step/default, rather than hardcoding a number.

**Only one Iris runs at a time on either platform** — `flock` on unix, a named
mutex on Windows. Both were chosen for the same property: the kernel releases
them however the process dies, so a hard kill leaves nothing stale behind.

Remaining declared work is in `ROADMAP.md`, and it is two unbuilt crates
(`iris-control`, `iris-stream`) that expose no API and are documented as such.

### Cross-checking the other platform

`cargo check --target x86_64-pc-windows-gnu` from Linux catches a surprising
amount — unix-only assumptions in tests, `PathBuf::join` separators, ungated
functions that warn as unused. It covers `iris-core`, `iris-hal` and
`iris-ipc-pipe-bridge`; `iris-ui` cannot be cross-checked because `reqwest`
pulls `ring`, whose build script needs a Windows C toolchain.

## Install

```sh
cargo build --workspace --release
./build_deb.sh                       # -> dist/baxters-iris_<version>_amd64.deb
sudo dpkg -i dist/baxters-iris_*.deb
```

Installs to `/opt/baxters/iris/` with a desktop entry, and launches through the
same `run.sh`. `Depends` are taken from the libraries a **running** instance
maps, not from `ldd` alone — the binary links only libc and libgcc while
eframe/winit `dlopen` EGL, Wayland and xkbcommon at startup.

**Only one Iris runs at a time.** A second launch exits immediately saying so:
two instances contend for the same camera and the same metrics port. The guard
is an `flock` on `$XDG_RUNTIME_DIR/iris.lock`, released by the kernel on exit,
so there is no stale lock to clean up after a crash.

## License

MIT
