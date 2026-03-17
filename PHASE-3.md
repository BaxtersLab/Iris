# Phase 3 — Productionization & Observability

Date: 2026-03-17

## Goal
- Harden and productionize the encoder/capture pipeline introduced in Phase 2. Make the system observable, controllable, and safe to run in long‑lived deployments while keeping all work local to this repository.

## High-level objectives
- Expose human‑friendly access to runtime metrics and diagnostics from the UI.
- Emit telemetry events for important encoder lifecycle events (rebases, encoder restarts, error rates).
- Provide a deterministic harness for replay and regression testing in CI/local runs.
- Ensure safe defaults, minimal resource leakage, and clear operational guidance.

## Scope
- `crates/iris-ui`: add a UI element (menu/button) which opens the `/metrics` endpoint or displays metrics in a dialog.
- `crates/iris-core`: emit a telemetry envelope when PTS rebase events occur (include prior mapping, new mapping, reason, and counts).
- `crates/iris-harness` (new): scaffold a harness crate to replay captured frames and assert end‑to‑end properties deterministically.
- Tests: add deterministic integration tests that exercise common failure modes (PTS wrap, drift, encoder restart).
- Docs: add runbook snippets and `PHASE-3.md` (this file) and update README with `How to run metrics` and `How to replay harness`.

## Acceptance criteria
- UI exposes a clear way for operators to retrieve or view Prometheus metrics (`/metrics`) from the running instance.
- `iris_core` emits a `TelemetryEnvelope::EncoderRebase` (or similar) whenever a PTS rebase occurs; unit/integration tests assert this envelope is produced.
- `crates/iris-harness` can replay a recorded capture stream and verify PTS→µs alignment and keyframe flags deterministically; harness tests run headless in CI.
- All existing tests still pass; new Phase‑3 tests pass locally (`cargo test --all`) and in CI (if enabled).
- No increase in nightly warnings or clippy failures; formatting enforced (`cargo fmt`, `cargo clippy -- -D warnings`).

## Tasks (initial)
1. Add `PHASE-3.md` (this file).
2. Add a small UI action in `crates/iris-ui` that opens the metrics URL in the user's browser and a fallback dialog for headless mode.
3. Add telemetry envelope emission in `crates/iris-core` for encoder rebase events; add unit tests asserting envelope contents.
4. Scaffold `crates/iris-harness` with a minimal replay runner and a simple assertion harness; add `Cargo.toml`, `src/lib.rs`, and a smoke test.
5. Add deterministic integration tests under `crates/iris-capture/tests/` that the harness can run against.
6. Update documentation: README and runbook snippets showing how to run `cargo test --all`, run the harness, and expose metrics.

## Testing plan
- Unit tests for `iris-core` PTS mapping/rebase behavior (already present — extend if needed).
- Harness smoke test: replay a short captured frame file and assert that one keyframe and sensible PTS mapping are produced.
- CI: run `cargo test --all` with harness tests gated by `--features=ffmpeg` or `CI_FFMPEG=true` (FFmpeg required) to avoid false failures on minimal runners.

## Observability & telemetry
- Prometheus metric `iris_encoder_rebase_total` (already added) must remain and be scraped by `/metrics`.
- Add `TelemetryEnvelope::EncoderRebase { prev_raw, prev_capture, new_raw, new_capture, reason }` to `iris-ipc::telemetry` and emit from `iris-core` when a rebase occurs.
- UI should optionally link to `/metrics` and show a human‑readable badge if rebase count > 0.

## Rollout & safety
- Default drift threshold remains conservative; make rebase threshold configurable via environment or config only.
- Add clear log lines for encoder start/stop/rebase and include telemetry envelopes so external systems can integrate later.

## Risks
- Running FFmpeg integration tests on CI requires FFmpeg installed — gate those tests.
- Exposing `/metrics` in public deployments must be documented; default binding is loopback.

## Next step (implementation)
- Implement Task 2 (UI metrics link/button) and Task 3 (emit encoder rebase telemetry envelope) locally in this workspace.

---
File: [PHASE-3.md](PHASE-3.md)
