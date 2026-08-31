// SPDX-License-Identifier: MIT
// Iris — iris-stream

//! Multi-subscriber frame streaming — **not yet implemented**.
//!
//! This crate is a declared placeholder. Its specification (instruction block
//! G-1) calls for `mode`, `subscriber`, `ring_buffer`, `service` and
//! `telemetry` modules providing four output modes (Pull, Push, SharedMemory,
//! IPC) with subscriber management. None of that exists yet, and the gap is
//! declared in `ROADMAP.md`.
//!
//! **It deliberately exposes no API.** Until 2026-08-31 it exported
//!
//! ```ignore
//! pub fn stream_info() -> &'static str { "stream" }
//! ```
//!
//! a literal that told a caller nothing and was never called by anything.
//!
//! What Iris ships today covers part of the intent by other means, and that is
//! worth knowing before rebuilding it here: `iris-ipc` already broadcasts
//! telemetry to multiple subscribers over a `tokio::sync::broadcast` channel,
//! and `iris-capture`'s `CaptureService` already owns a bounded frame queue
//! with an explicit drop policy. The unmet part is **frame** fan-out to more
//! than one consumer, and the shared-memory and IPC transports.
