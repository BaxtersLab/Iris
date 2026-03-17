Phase‑4 — 2026-03-17
=====================

Objective
---------
Move from Phase‑3 stabilization to Phase‑4: deterministic CI, harnessing, and production-ready integrations.

Primary Goals
-------------
- Scaffold a deterministic harness crate (`crates/iris-harness`) to simulate encoder/decoder inputs so CI can run without ffmpeg.
- Harden CI: add deterministic harness runs, tighten timeouts, and improve artifact collection.
- Implement deterministic telemetry replay tests to validate rebases and telemetry forwarding.
- Add an optional archive-and-purge policy for logs (configurable retention).
- Prepare release notes and a final Phase‑4 acceptance checklist.

Acceptance Criteria
-------------------
- CI runs deterministic harness tests successfully on GitHub runners without ffmpeg.
- Telemetry integration tests pass reliably (> 95% on CI for 10 consecutive runs).
- `/metrics` endpoint verified and Prometheus export text present in CI artifacts.
- Logs rotation/retention implemented or documented.

Next Steps
----------
- Confirm priority: scaffold harness, or update CI first.
- If scaffolded: I'll create `crates/iris-harness` with a small runner and tests.
- If CI-first: I'll update `.github/workflows/ci.yml` to add harness steps and artifact upload.
