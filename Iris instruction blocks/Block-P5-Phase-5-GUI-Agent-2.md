# Block P5 — Phase‑5: Clean GUI (Agent 2)

Purpose
-------
This instruction block gives Agent 2 a precise, actionable plan to implement, verify, and harden the Phase‑5 GUI work described in `docs/PHASE-5-GUI.md`. Focus: make the `iris-ui` main window reliable on Windows, expose Start/Stop webcam controls, ensure preview and telemetry integration, and add a Windows-smoke test.

Owner
-----
Agent 2 (you) is responsible for writing code, tests, and documentation. Coordinate with Agent 1 for telemetry and CI gating decisions if needed.

Prerequisites
-------------
- Windows development environment with Rust toolchain installed and `cargo` on PATH.
- `ffmpeg` available locally for manual capture tests (optional for UI-only work).
- Workspace is at: `C:\Users\Baxter\Desktop\Iris`.
- You have local Git commit rights (we are committing directly to `master` locally per project policy).

Files of interest
-----------------
- `crates/iris-ui/main.rs`
- `crates/iris-ui/bootstrap.rs`
- `crates/iris-ui/ui_app.rs`
- `crates/iris-ipc/command.rs`
- `docs/PHASE-5-GUI.md`
- `docs/PHASE-5-GUI.md` (reference)
- Add tests under `crates/iris-ui/tests/` and scripts under `.github/scripts/`.

Agent 2 Tasks — Exact Steps (do them in order)
---------------------------------------------
1) Local checkout & safety
   - Ensure workspace up-to-date locally and on `master` branch.
   - Command:
     ```powershell
     cd "C:\Users\Baxter\Desktop\Iris"
     git status --porcelain
     ```
   - If dirty, stash or commit only UI-related work you own.

2) Implement robust Bring‑to‑Front helper (Windows)
   - Add dependency `windows-sys = "0.48"` and `widestring = "0.5"` to `crates/iris-ui/Cargo.toml` under `[dependencies]`.
   - Create helper in `crates/iris-ui/src/win32.rs`:
     - Implement `pub fn bring_iris_to_front()` using `FindWindowW`, `ShowWindow(SW_RESTORE)`, and `SetForegroundWindow`.
     - Use `U16CString` or `widestring` to build the wide string for title `Iris`.
   - Update `crates/iris-ui/bootstrap.rs` to call `bring_iris_to_front()` from the `IpcCommand::ShowUi` handler (only on `#[cfg(windows)]`).

3) Make window title deterministic
   - Confirm `eframe::run_native("Iris", ...)` is called in `crates/iris-ui/main.rs`.
   - If not, change it to exactly `"Iris"` to match the Win32 helper.

4) Improve `ui_app.rs` controls and resilience
   - Add explicit disabled state for `Start Capture` when no device selected.
   - Improve `try_recv` handling: log lag/dropped messages and avoid silently dropping `capture_rx` on transient errors; preserve receiver for at least 1 second before closing.
   - Add tooltips and keyboard focus order for top-panel buttons.

5) Add Windows-only smoke test & PowerShell script
   - Add test file `crates/iris-ui/tests/gui_smoke_windows.rs` (gated with `#[cfg(windows)]`) that:
     - Starts `iris-ui` in a subprocess (or assumes it's running), connects via `IpcHandle` as client, sends `IpcCommand::ShowUi`, `StartCapture`, waits for telemetry for up to 5s, then `StopCapture`.
     - Use `tokio::time::timeout` to bound waits and fail clearly when telemetry missing.
   - Add `.github/scripts/gui-smoke.ps1` for local runs that:
     - Ensures `iris-ui` is running (starts it if not), waits for process, calls local IPC `ShowUi` and `/metrics` checks, and prints success/failure.

6) Manual verification
   - Build and run locally:
     ```powershell
     cargo build -p iris-ui
     cargo run -p iris-ui
     ```
   - From a separate PowerShell session, run:
     ```powershell
     # call ShowUi via IPC (example client code or use test harness)
     # run the smoke script
     .\.github\scripts\gui-smoke.ps1
     ```
   - Confirm the window title is `Iris`, the window is restored and focused, and pressing `Start Capture` produces telemetry frames visible in the UI log and `/metrics`.

7) Commit, tag, and document
   - Commit changes locally with clear messages:
     ```powershell
     git add crates/iris-ui Cargo.toml .github/scripts/gui-smoke.ps1 crates/iris-ui/tests/gui_smoke_windows.rs
     git commit -m "feat(ui): robust bring-to-front helper; UI resilience and GUI smoke test"
     ```
   - Leave Phase‑4 sealed and create Phase‑5 branch locally if you want to isolate:
     ```powershell
     git checkout -b phase-5/gui-polish
     ```

Acceptance criteria
-------------------
- Running `cargo run -p iris-ui` displays a window with title `Iris`.
- From another process, `IpcCommand::ShowUi` restores and focuses the `Iris` window on Windows.
- `Start Capture` and `Stop Capture` buttons trigger IPC commands and produce capture telemetry visible in the UI log and `/metrics`.
- Windows smoke script returns success on a local dev machine within 10 seconds.

Notes, caveats & troubleshooting
--------------------------------
- Bringing a window to the foreground can fail when caller and target run at different privilege levels; document this in `docs/PHASE-5-GUI.md` and in the smoke script.
- If `target\\debug\\iris-ui.exe` is locked during rebuild, close running processes or kill via `Get-Process -Name iris-ui | Stop-Process`.

Communication
-------------
- On completion, push local branch or keep changes local per release policy and attach test results (screenshot and `/metrics` output) to the Phase‑5 notes.

If you want, I can implement steps 2 and 3 (add `windows-sys` helper and update `bootstrap.rs`) now and run a local build/test. Reply "Implement helper" to proceed.
