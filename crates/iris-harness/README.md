iris-harness
============

A small deterministic test harness for Phase-4 CI. It writes a deterministic
set of telemetry events to a JSONL file so CI can validate telemetry handling
without depending on FFmpeg.

Usage
-----

From workspace root:

```bash
cargo test -p iris-harness -- --nocapture
```

Running the runner (produces real ffmpeg output when `ffmpeg` is available):

```bash
# produce harness-output/stream.ts (or fallback telemetry.jsonl)
cargo run -p iris-harness --bin runner --quiet
```
