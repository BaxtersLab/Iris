// SPDX-License-Identifier: MIT
// Iris — iris-stream

//! Frame fan-out: one capture source, several consumers.
//!
//! What Iris already had covers part of this and is worth knowing before
//! reaching for anything here. `iris-ipc` broadcasts **telemetry** to many
//! subscribers, and `iris-capture`'s `CaptureService` owns a bounded frame
//! queue with an explicit drop policy — but that queue has exactly **one**
//! consumer. The gap this crate fills is frames reaching more than one place at
//! once without the slowest consumer dictating the rate for everybody.
//!
//! Two modes are implemented — `Pull` (a ring of recent frames, read at the
//! consumer's own pace) and `Push` (per-subscriber channels, where a slow
//! subscriber drops only its own frames). `SharedMemory` and `Ipc` are named in
//! the vocabulary and **refused by the service**, because a mode that silently
//! behaves like a different one is worse than a mode that says it is not built.
//!
//! Until 2026-08-31 this crate was one function returning the string
//! `"stream"`, while the README advertised "multi-subscriber output, ring
//! buffer, IPC delivery".

pub mod mode;
pub mod ring_buffer;
pub mod service;
pub mod subscriber;

#[cfg(test)]
mod tests;

pub use mode::StreamMode;
pub use ring_buffer::{shared_ring_buffer, RingBuffer, RingSlot, SharedRingBuffer};
pub use service::{StreamCommand, StreamHandle, StreamService, StreamStats};
pub use subscriber::{FrameSubscription, SubscriberId, SubscriberStats};
