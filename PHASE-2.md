# Phase 2 — Noise‑Free, Agent/User Controlled

Goal
- Move to Phase‑2: achieve deterministic, agent/user-controlled operation with zero unidentified noise and zero unidentified signals. This is a local-only phase; backups are managed externally.

Acceptance criteria
- All telemetry and signal sources are identifiable and labeled.
- Agent control over capture/forwarding/dispatching and UI actions is available and testable.
- No flaky integration tests; phase-2 tests pass 10/10 across repeated runs.
- Automated noise‑free test harness can run headless and produce reproducible results.
- Changelog and release notes drafted for Phase‑2.

Scope
- Implement agent/user control interfaces and APIs to toggle sources and capture policies.
- Create a deterministic test harness to inject controlled inputs and assert outputs.
- Add telemetry labeling and source attribution across pipeline.
- Harden dispatcher/forwarder for deterministic priority handling (already improved in Phase‑1).
- Add documentation, changelog, and acceptance tests.

Initial Tasks
1. Create a `phase-2` working branch locally (optional but recommended).
2. Draft detailed acceptance tests in `crates/iris-ui/tests/phase2_spec.rs`.
3. Implement control interfaces in `crates/iris-control` and UI bindings in `crates/iris-ui`.
4. Build a noise-free harness: a minimal harness crate `crates/iris-harness` that can run captured-input replay and assertions.
5. Add telemetry source labels in `crates/iris-ipc/telemetry.rs` and emit source metadata.
6. Run `cargo test -p iris-ui --test phase2_spec -- --nocapture` repeatedly until stable.

Checklist
- [ ] Create local `phase-2` branch
- [x] Draft Phase‑2 spec (this file)
- [ ] Implement harness crate `iris-harness`
- [ ] Implement agent control endpoints
- [ ] Add telemetry source attribution
- [ ] Add deterministic test cases and 10x sweep
- [ ] Prepare Phase‑2 changelog and release notes

Notes
- Phase‑1 is sealed in the repository as `phase-1-stable` tag and local commits; do not modify Phase‑1 artifacts.
- All work is local; pushing to a remote is optional and user-controlled.

Next steps (suggested)
- Confirm you want a `phase-2` git branch created locally; if yes I will create it now.
- If you'd prefer, tell me which task to start first (harness, control APIs, or telemetry labeling).