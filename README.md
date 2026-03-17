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
