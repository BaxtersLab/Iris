# Phase‑5: Clean GUI Implementation

Goal
----
Ship a clean, usable Iris GUI that reliably appears on user machines, exposes Start/Stop webcam controls, and integrates with existing telemetry and IPC. Phase‑5 focuses on UX polish, window behavior, robust capture control, accessibility, and tests so Iris is useful for everyday use.

Assumptions
-----------
- Telemetry and the `iris_encoder_rebase_total` metric are already implemented and exposed via `/metrics`.
- `IpcCommand::ShowUi` exists and `iris-ipc` can send commands to a running `iris-ui` process.
- Capture service and IPC `StartCapture`/`StopCapture` commands are implemented (already present in codebase).

Deliverables
------------
- `iris-ui` shows a main window with a clear title bar and a top panel with:
  - `Start Capture` button
  - `Stop Capture` button
  - `Device` refresh and select UI
  - Live preview area that renders latest frames
- Reliable Show/Bring‑to‑Front behavior on Windows (restore minimized and set foreground).
- Integration test that sends `IpcCommand::ShowUi` and validates the window appears (or an equivalent smoke script on Windows).
- Accessibility improvements: keyboard focus for controls and high-contrastability.
- CI job / local smoke script that exercises show UI and Start/Stop commands (headless-mode tolerant).

Milestones & Exact Steps
------------------------
1) Audit existing UI startup and headless selection
   - Files: `crates/iris-ui/main.rs`, `crates/iris-ui/bootstrap.rs`, `crates/iris-ui/ui_app.rs`.
   - Confirm `IRIS_UI_HEADLESS` behavior remains intact for CI.

2) Make window creation deterministic and titled
   - Ensure `eframe::run_native("Iris", ...)` is called with the expected title string `Iris`.
   - If needed, set a stable window class/title so Win32 `FindWindow` can locate it reliably.
   - Files to edit: `crates/iris-ui/main.rs` (title), optionally `crates/iris-ui/ui_app.rs`.
   - Test: build and run locally, ensure `iris-ui` window title equals `Iris`.
   - Commands:
     ```powershell
     cargo run -p iris-ui
     ```

3) Implement robust Show/Bring‑to‑Front behavior (Windows)
   - Replace ad‑hoc `FindWindow` logic with small helper using `windows-sys` or `winapi`:
     - Use `FindWindowW` (title wide-char) and `SetForegroundWindow` + `ShowWindow(SW_RESTORE)`.
     - Consider `AllowSetForegroundWindow` fallback for elevated processes.
   - File: `crates/iris-ui/src/bootstrap.rs` (dispatcher `ShowUi` handler).
   - Test: from a separate Powershell session, send `ShowUi` via IPC or run `.github/scripts/ci-smoke.ps1` equivalent.

4) Make Start/Stop and device controls fully functional
   - Ensure `Start Capture` sends `IpcCommand::StartCapture` and `Stop Capture` sends `StopCapture`.
   - Make sure capture preview uses a stable `tokio::sync::mpsc::Receiver` and does not drop frames silently.
   - File: `crates/iris-ui/ui_app.rs` — add error handling/logging for `try_recv` paths and a disabled UI state while no device selected.
   - Test: press buttons locally and verify capture telemetry lines appear; confirm preview updates.

5) Add integration/smoke tests
   - Add a Windows-only integration test under `crates/iris-ui/tests/` that:
     - Starts `iris-ui` in background (spawn) or assumes it's running.
     - Uses `IpcHandle` client to send `ShowUi` then `StartCapture` and `StopCapture`.
     - Validates via `/metrics` or `ipc.subscribe_telemetry()` that frame telemetry arrived.
   - Alternatively provide `.github/scripts/gui-smoke.ps1` for local runs that uses `curl` to call `/debug/force_rebase` and uses PowerShell `Get-Process -Name iris-ui` + screen capture check.

6) Accessibility and UX polish
   - Ensure controls have keyboard focus order and accessible labels.
   - Add tooltips for `Start`/`Stop` and disabled states.
   - Size the preview to a reasonable default and support scaling.

7) CI and release integration
   - Add an optional, gated workflow `gui-smoke` that runs on self-hosted Windows runners if available. Gate behind `RUN_GUI_SMOKE` env var.
   - Keep headless CI steps unchanged for Linux/macOS.

8) Packaging and documentation
   - Add `docs/PHASE-5-GUI.md` (this file) to repo and reference in `README.md`.
   - Add a short user guide describing Start/Stop and how to bring the UI to front using IPC.

Implementation Notes — Code snippets
---------------------------------
- Win32 helper (Rust, suggest using `windows-sys`):

```rust
#[cfg(windows)]
fn bring_iris_to_front() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE};
    use widestring::U16CString;
    if let Ok(title) = U16CString::from_str("Iris") {
        unsafe {
            let h = FindWindowW(std::ptr::null(), title.as_ptr());
            if h != 0 {
                ShowWindow(h, SW_RESTORE);
                SetForegroundWindow(h);
            }
        }
    }
}
```

Testing & Verification
----------------------
- Manual: run `cargo run -p iris-ui`, open UI, click `Start Capture` → verify capture telemetry logs and preview.
- Automated (local): run `.github/scripts/gui-smoke.ps1` which:
  - ensures `iris-ui` running; calls `ShowUi` via IPC; hits `/metrics` to validate telemetry; presses Start/Stop via IPC.

Risks & Mitigations
-------------------
- Elevated process restrictions: bring-to-front may fail when processes have different integrity levels. Mitigation: document limitation and recommend same privilege level for UI and callers.
- Headless CI: GUI tests must be gated and optional; do not block main CI.

Timeline & Owners
-----------------
- Week 1: Audit and implement stable windowing + ShowUi handler (owner: dev A)
- Week 2: Hook up Start/Stop robustly and add preview resilience (owner: dev B)
- Week 3: Accessibility, tests, and CI gating (owner: dev A/B)

---
If you want, I can now: (A) implement the `windows-sys` helper and update `bootstrap.rs` with the improved call, (B) add the Windows-only integration test and a PowerShell smoke script, or (C) start by running the manual verification steps and capturing results. Which should I do next?
