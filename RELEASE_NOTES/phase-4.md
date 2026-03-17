**Phase 4 — Release Draft**

Release tag: `phase-4`

Summary:
- Deterministic harness (`crates/iris-harness`) now emits a real MPEG-TS `harness-output/stream.ts` using `ffmpeg`.
- Added `tools/print-metrics` and in-process metrics helper to observe `iris_encoder_rebase_total`.
- Added local smoke-test script `.github/scripts/ci-smoke.ps1` to start `iris-ui`, invoke `/debug/force_rebase`, and assert `iris_encoder_rebase_total == 1`.
- Wired `IpcCommand::ForceRebase` and an HTTP `/debug/force_rebase` endpoint in `iris-ui` to force an in-process rebase for diagnostics.
- CI/workflow updated locally to produce and collect `harness-output` artifacts (packaged at `ci-artifacts/phase-4.zip` when run locally).

Artifacts (local):
- `ci-artifacts/phase-4.zip` — contains `harness-output/stream.ts` and telemetry output produced by the deterministic harness run.
- `harness-output/stream.ts` — MPEG-TS stream produced by the harness (ffmpeg).

How to validate locally:
1. Build and run harness and tests (from repo root):
   ```powershell
   Set-Location 'C:\Users\Baxter\Desktop\Iris'
   cargo build --workspace
   cargo run -p iris-harness --quiet
   cargo test --workspace
   ```
2. Run the local smoke test (PowerShell):
   ```powershell
   Set-Location 'C:\Users\Baxter\Desktop\Iris'
   & '.\.github\scripts\ci-smoke.ps1'
   ```
   The script starts `iris-ui`, calls `/debug/force_rebase`, fetches `/metrics`, and asserts the rebase metric equals `1`.

Publishing notes:
- To push the tag to remote: `git push origin phase-4`.
- Draft release on GitHub using the `phase-4` tag, and attach `ci-artifacts/phase-4.zip` if desired.

Next steps before public release:
- Finalize `CHANGELOG.md` entries and Phase‑4 docs.
- Verify CI artifact upload in remote CI and optionally enable smoke test on runners.
- Draft announcement / release notes with stakeholder summary and instructions.

Contact: Baxter (local maintainer) — this repo is maintained locally; CI smoke script is intended for local verification.
