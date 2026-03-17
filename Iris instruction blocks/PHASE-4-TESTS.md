PHASE‑4 — Human Test Instructions
=================================

Purpose
-------
These instructions let a human verify Phase‑4 functionality locally: deterministic harness output, consumer parsing (PES/PTS), telemetry forwarding, and the `/metrics` endpoint. Follow the checks below in order to confirm the workspace is ready to seal Phase‑4.

Prerequisites
-------------
- A machine with `ffmpeg` on PATH for real-stream tests (optional: harness falls back to JSONL).
- Rust toolchain installed (stable).
- From the workspace root: `cargo` available.
- Recommended environment variables for verbose debug logging (PowerShell example):

```powershell
$env:RUST_LOG = "debug"
```

Quick smoke (recommended order)
------------------------------
1. Produce deterministic harness outputs (runs `ffmpeg` when present):

```powershell
# From workspace root
cargo run -p iris-harness --bin runner --quiet
# Output appears in harness-output/ (stream.ts and/or telemetry.jsonl)
```

2. Run the consumer test (uses harness-output/stream.ts if present):

```powershell
# Run just the consumer test in iris-core
cargo test -p iris-core test_consume_stream_ts_into_packets -- --nocapture --exact
```

Expected: test passes and reports it consumed at least one `EncodedPacket`.

3. Run workspace tests (optional for full check):

```powershell
cargo test --workspace -- --nocapture
```

4. Start headless UI (verifies telemetry forwarding and `/metrics`):

```powershell
# Run iris-ui in headless mode (logs to iris-ui-headless.log)
$env:IRIS_UI_HEADLESS='1'
cargo run -p iris-ui --quiet > iris-ui-headless.log 2>&1 &
# Wait a few seconds, then tail
Get-Content iris-ui-headless.log -Tail 200
```

What to look for in the headless UI log:
- Messages indicating the RecordingPipeline or harness runner wrote `stream.ts`.
- Telemetry forwarding logs: `TelemetryEnvelope::EncoderRebase` or similar messages.
- `/metrics` server start and any `iris_encoder_rebase_total` metric exposure.

5. Verify `/metrics` endpoint (default: UI exposes a text exporter).
Open a browser or use curl against the UI host (if running on local machine). Example using curl (PowerShell):

```powershell
# If UI binds to localhost:9500 (or check headless log for actual port):
curl http://127.0.0.1:9500/metrics
```

Expected: Prometheus text output including `iris_encoder_rebase_total`.

File/Artifact checks
--------------------
- `harness-output/stream.ts` — a real MPEG-TS file produced by `ffmpeg` (if available).
- `harness-output/telemetry.jsonl` — deterministic JSONL fallback events.
- `logs/*` — runtime logs; check for `telemetry_integration` related entries.
- CI artifacts: in GitHub Actions, look for `harness-output` artifact which contains the `stream.ts` and `telemetry.jsonl` files.

Troubleshooting
---------------
- If the consumer test skips because `stream.ts` is missing, run the harness runner first (`cargo run -p iris-harness --bin runner`).
- If `ffmpeg` fails on CI or locally, check PATH and install instructions in `README.md`.
- If tests fail intermittently on CI, inspect the uploaded `harness-output/stream.ts` artifact to reproduce locally.

Acceptance Checks (human)
-------------------------
- Harness runner writes `harness-output/stream.ts` or `telemetry.jsonl`.
- Consumer test (`test_consume_stream_ts_into_packets`) passes when `stream.ts` is present.
- Headless UI starts and publishes `/metrics` containing `iris_encoder_rebase_total`.
- CI artifacts include the `harness-output` bundle for post-mortem investigation.

Next steps for sealing Phase‑4
-----------------------------
- Confirm human tests pass on at least one clean machine.
- Optionally, run CI and confirm artifacts are uploaded and consumer tests pass on the runner.
- When satisfied, ask me to mark Phase‑4 sealed in `CHANGELOG.md` and create a release note.

Contact
-------
If anything is unclear or you hit a blocking error, paste the failing command and the last 50 lines of the relevant log and I will triage it.
