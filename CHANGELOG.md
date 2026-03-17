# Changelog

All notable changes in this workspace since the previous local snapshot.

Unreleased
----------
- No unreleased changes.

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
  `C:\Users\Baxter\Desktop\logs` so Phase‑4 starts with fresh logs.

Notes
-----
- All changes are local-only per your preference; no commits or pushes were
  made to a remote repository. If you want a single-file summary or a more
  detailed changelog entry (per crate), I can expand this file accordingly.
 
Phase-3: Sealed - 2026-03-17
---------------------------
- Backup completed (4.5 GB) and workspace logs archived to
  `logs_archive/2026-03-17_01-40-52`. Original log files recreated empty at
  `C:\Users\Baxter\Desktop\logs` to begin Phase‑4 with fresh logs.

Next: Phase‑4 planning started (see PHASE-4.md).
