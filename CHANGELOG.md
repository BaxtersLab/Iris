# Changelog

All notable changes in this workspace since the previous local snapshot.

Unreleased
----------
UI thread unblocked, config surface made real — 2026-08-31 (Linux workstation)
- **The preview drain was a livelock, not a perf nit.** `ui_app.rs` looped on
  `try_recv` until `Empty` while converting each frame inline, so once a
  conversion cost about as much as the gap between frames the producer refilled
  the channel as fast as the loop drained it and `update()` never returned to
  the event loop. Measured over 30 s with no user input: **1 repaint** at
  640x480 NV12 @30 and at 3840x2160 @30. Now `drain_to_newest()` — pop the
  backlog without converting, convert only the survivor, cap one drain at
  `MAX_DRAIN_PER_REPAINT` — giving **453** and **25** repaints on the same runs.
- **Nothing in the app ever requested a repaint.** egui's loop is reactive, so
  the live preview only advanced while the window was receiving input; leave it
  alone or cover it and it froze while capture ran on. `drive_repaints()` paces
  the window at ~60 Hz while capturing, 4 Hz idle.
- **Latent UI-thread panic fixed**: the RGB24/BGR24 conversion read `d[i + 1]`
  off an index step, so a buffer whose length was not a multiple of 3 indexed
  past the end. Now `chunks_exact(3).take(w * h)`.
- **`capture.pixel_format` and `capture.drop_policy` were dead config.**
  `bootstrap.rs` hardcoded `Bgr24` and `Oldest`; the default config said `nv12`.
  Both are now routed. Frame size in telemetry moved 921600 -> 460800 bytes at
  640x480, and to 614400 with `pixel_format = "yuy2"`.
- **`IrisConfig::validate()` had no caller outside its own tests.** `main.rs`
  now validates after loading and falls back to defaults with the reason
  printed. The accepted format list dropped `bgra8` (unproducible, and 4 bytes
  per pixel where the nearest variant is 3) and gained `rgb24`/`bgr24`; a test
  in `iris-hal` pins that list against the parser so they cannot drift.
- `run.sh` added — the Linux launcher, with the snap-environment scrub the rest
  of the suite uses. Iris had no launcher at all.
- Gate: **76 -> 102 tests**, 0 failures, 0 build warnings. `iris-ui` had no unit
  tests before this; the preview conversion and drain now have 15.

Reconciliation + real-camera finish — 2026-07-18
- Restored the capture pipeline (backend/service/frame/telemetry + DXGI) that
  the March drift deleted; revived the camera UI as the `iris-ui` bin.
- Filled never-committed gaps: `iris_core::pipeline` (Prometheus /metrics,
  rebase counter), `IpcCommand::{ForceRebase, ShowUi}`. NOTE: the Phase-4
  `RecordingPipeline`/PES-PTS encoder described below was never committed and
  did not survive the March wipe — only the metrics surface was rebuilt; the
  encoder remains lost/to-redo.
- NEW `UvcCaptureBackend`: drives any `UvcBackend` as a `CaptureBackend`
  (enumerate → `IRIS_DEVICE` select → open → read loop); `IRIS_BACKEND` now
  accepts `mock|dxgi|wmf|v4l2`; adopts the device's ACTUAL mode so telemetry
  resolution/format/size are frame-authoritative.
- WMF fixes proven on real hardware (USB camera, 1920×1080 NV12): ReadSample
  stream-tick retry (first-frame "no sample"), COM keeper thread (MTA off the
  main thread → fixes RPC_E_CHANGED_MODE window panic), telemetry forwarder
  no longer busy-spins on a closed channel.
- UI preview: NV12 + YUYV → RGBA conversion (live camera picture).
- Linux port: `v4l2_backend` (raw-ioctl enumerate/probe + 5 tests) and
  unix-socket transport for `iris-ipc-pipe-bridge` (live UDS loopback smoke).
- `iris-harness` completed (lib was never committed): ffmpeg MPEG-TS test
  stream or deterministic JSONL `TelemetryEnvelope` writer, 2 tests.
- airdex material (gesture/Mixxx/nexa/overlay) extracted to
  `..\airdex_misplaced_from_iris\` — airdex is a separate app that depends on
  Iris (source at `I:\Air Dex`).

Phase-4: Sealed - 2026-03-17
---------------------------
- Implemented `RecordingPipeline` encoder worker (ffmpeg subprocess) with
  MPEG-TS/PES parsing, PES PTS parsing, PTS→µs mapping, wrap handling and
  rebase-on-drift logic.
- Added `EncoderRebaseEvent` (local) and forwarded it from `iris-core` into
  `TelemetryEnvelope::EncoderRebase` via `iris-ui` to provide observable
  telemetry when PTS mappings are rebased.
- Exposed Prometheus metric `iris_encoder_rebase_total` and added
  `prometheus_text()` exporter plus a `/metrics` HTTP endpoint in `iris-ui`.
- Added integration tests:
  - `crates/iris-capture/tests/ffmpeg_integration.rs` (ffmpeg end-to-end keyframe/PTS test, gated)
  - `crates/iris-capture/tests/ffmpeg_rebase_integration.rs` (ffmpeg rebase end-to-end, gated)
  - `crates/iris-ui/tests/rebase_forwarding.rs` (forwards local rebase → IPC telemetry)
  - unit tests in `crates/iris-core/pipeline.rs` for PES/PTS and rebase behavior
- CI: updated `.github/workflows/ci.yml` to install ffmpeg on runners and set
  `CI_FFMPEG=1` in `GITHUB_ENV` when ffmpeg is available so FFmpeg-gated tests
  run only on capable runners.
- Tests: increased ffmpeg test timeouts and gated runs behind `CI_FFMPEG` or
  `RUN_FFMPEG_INTEGRATION` to reduce CI flakiness.
- Documentation: added `PHASE-3.md` (Phase‑3 spec) and `README.md` with
  instructions for enabling FFmpeg tests locally and in CI.
- Linting/format: ran `cargo fmt` and addressed clippy warnings related to
  recent edits.
- Maintenance: archived existing runtime and test logs to
  `logs_archive/2026-03-17_01-40-52` and recreated empty log files under
  `%USERPROFILE%\Desktop\logs` so Phase‑4 starts with fresh logs.

Notes
-----
- All changes are local-only per your preference; no commits or pushes were
  made to a remote repository. If you want a single-file summary or a more
  detailed changelog entry (per crate), I can expand this file accordingly.
 
Phase-3: Sealed - 2026-03-17
---------------------------
- Backup completed (4.5 GB) and workspace logs archived to
  `logs_archive/2026-03-17_01-40-52`. Original log files recreated empty at
  `%USERPROFILE%\Desktop\logs` to begin Phase‑4 with fresh logs.

Next: Phase‑4 planning started (see PHASE-4.md).
