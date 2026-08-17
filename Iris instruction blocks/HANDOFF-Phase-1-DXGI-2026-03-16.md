# HANDOFF — Phase 1: DXGI Capture Backend
Date: 2026-03-16

> **Redaction note (2026-08-16).** Local filesystem paths and references to a
> separate internal project were replaced with placeholders when this repository
> was made public. No technical content was changed.

Summary
- Phase: 1 (DXGI desktop duplication capture) — COMPLETE.
- Owner: Baxter (local workspace). Implementation added to `crates/iris-capture` and exercised in `iris-ui` headless.

What was delivered
- `crates/iris-capture/src/dxgi_backend.rs` — `DxgiCaptureBackend` implementing `CaptureBackend` (start/stop/next_frame/is_capturing).
- `crates/iris-capture/examples/dxgi_test.rs` — small example to run a hardware capture smoke test.
- `crates/iris-ui/bootstrap.rs` — temporary wiring used to exercise DXGI in headless runs (can be reverted to runtime selection).
- Tests: `cargo test --workspace` ran and passed on this machine.
- Logs: headless run output saved to `<workspace-root>\iris-ui-headless.log`.

Key files & locations
- Implementation: `crates/iris-capture/src/dxgi_backend.rs`
- Example test: `crates/iris-capture/examples/dxgi_test.rs`
- Headless UI logs: `iris-ui-headless.log` (workspace root)
- Phase instruction blocks: `Iris instruction blocks/` (this folder). The DXGI
  backend was adapted from a screen-recorder project maintained separately and
  not distributed with Iris.

How to reproduce (local Windows machine)
1. Ensure native toolchain and env:
   - `VCPKG_ROOT` and `LIBCLANG_PATH` set as on this machine (example: `C:\tools\vcpkg`, `C:\tools\LLVM\bin`).
2. Run unit tests (workspace):
```powershell
$env:VCPKG_ROOT='C:\tools\vcpkg'; $env:LIBCLANG_PATH='C:\tools\LLVM\bin'; cargo test --workspace
```
3. Run DXGI example (captures 5 frames):
```powershell
$env:VCPKG_ROOT='C:\tools\vcpkg'; $env:LIBCLANG_PATH='C:\tools\LLVM\bin'; cargo run -p iris-capture --example dxgi_test
```
4. Run headless UI (exercises integration):
```powershell
$env:VCPKG_ROOT='C:\tools\vcpkg'; $env:LIBCLANG_PATH='C:\tools\LLVM\bin'; $env:RUST_LOG='debug'; $env:IRIS_UI_HEADLESS='1'; cargo run -p iris-ui > iris-ui-headless.log 2>&1
Get-Content iris-ui-headless.log -Tail 200
```

Notes, caveats, and decisions
- The DXGI code uses `D3D11` + Desktop Duplication API (IDXGIOutput1::DuplicateOutput) and produces `PixelFormat::Bgr24` by stripping alpha from BGRA8.
- Minor adjustments were made to match `windows` crate (v0.54) binding idioms (out-params, Option usage, driver type selection).
- `iris-ui` was temporarily wired to use `DxgiCaptureBackend` for validation. Revert or make runtime-selectable when ready.
- Hardware validation succeeded here (captured frames reported). If running on a different machine, device/adapter indexes may differ.

Next recommended steps
1. Revert `iris-ui` bootstrap to runtime-selectable backend (use `IRIS_BACKEND` env var or config). See `Iris instruction blocks/Phase-2-Pipeline-Wiring.md` for pipeline wiring.
2. Implement WMF backend in `iris-hal` if camera capture required (Phase 1 alternative for UVC).
3. Phase 2: integrate frame conversion, buffering, and pipeline (pipeline wiring). Run integration tests and end-to-end validation.

Contact & provenance
- At the time of writing, all edits were in-place in a local `Iris` workspace
  with no remote configured. The project has since been published to a git
  remote; this line is retained as part of the original dated record.
- The originating screen-recorder artifacts are maintained separately and are
  not distributed with Iris.

Appendix — quick diff notes
- Added `tracing` dependency and `windows-core` tweaks in `crates/iris-capture/Cargo.toml` during iteration.
