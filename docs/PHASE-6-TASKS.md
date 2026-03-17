Phase 6 — Task Checklist
=========================

Top-level tasks
---------------
- 1. Review Phase‑5 gap inventory and mark actionable items.
- 2. Implement high-priority features (list below).
- 3. Add/extend integration tests and Windows smoke tests.
- 4. Run real-device validation on representative hardware.
- 5. Package release artifacts and produce installable bundles.
- 6. Update user-facing docs and `HANDOFF_CAMERA.md` as needed.

Initial feature list (prioritized)
---------------------------------
1. UI: Finalize Start/Stop webcam controls and preview reliability.
2. IPC: Ensure telemetry ordering and rebase forwarding stability.
3. Capture: Improve ROI handling and NV12 alignment robustness.
4. Packaging: Create a portable Windows bundle and installer script.

Testing
-------
- Expand `tests/windows_smoke.rs` to exercise UI start/stop and telemetry.
- Add device validation steps and a simple harness script under `harness-output/`.

Deliverables
------------
- Updated `docs/PHASE-6.md` and `docs/PHASE-6-TASKS.md` (this file).
- A `harness-output/phase-6-validation/` folder with logs and test results.
- Packaging artifacts under `ci-artifacts/phase-6/`.

Seal notes (2026-03-17)
----------------------
- Acceptance: GUI smoke and real-device validation completed locally.
- GUI smoke: invoked `.github/scripts/gui-smoke.ps1` which triggered `/debug/force_rebase` and validated `iris_encoder_rebase_total == 1`.
- Real-device validation: ran `.github/scripts/real-device-validate.ps1` (started `iris-ui` with `IRIS_BACKEND=dxgi`, attempted to foreground the UI via `.github/scripts/bring-iris-front.ps1`, observed `FrameCaptured` telemetry in logs).

Artifacts (local paths)
----------------------
- Packaging artifact: `ci-artifacts/phase-6/iris-ui-windows.zip`
- Harness output: `harness-output/phase-6/real-device/` contains:
	- `iris-ui-real-device.log` (full runtime log)
	- `log-tail.txt` (last ~200 lines)
	- `metrics.txt` (Prometheus `/metrics` snapshot)

Next steps
----------
- Update `CHANGELOG.md` with Phase‑6 seal entry and artifact links.
- Finalize `HANDOFF_CAMERA.md` with instructions for reproducing the real-device run.
- Optionally push the local `phase-6` tag and create a PR (requires remote access).

Notes
-----
Keep changes minimal and test-driven. Run `cargo test --all --workspace` often.
