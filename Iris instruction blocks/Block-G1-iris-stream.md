Block G-1 — iris-stream
=======================

Objective
---------
Implement the iris-stream crate: multi-subscriber frame streaming with four output
modes (Pull, Push, SharedMemory, IPC), ring buffer, subscriber management, and
telemetry. Replaces BSR's bsr-encode + bsr-mux combination with a more general
streaming architecture (no encoding required for raw frames).

Prerequisites
-------------
Blocks A-1, A-2, B-1, and E-1 must be complete.

File: crates/iris-stream/lib.rs
---------------------------------
Public modules: mode, subscriber, ring_buffer, service, telemetry.

```rust
pub mod mode;
pub mod subscriber;
pub mod ring_buffer;
pub mod service;
pub mod telemetry;
```

File: crates/iris-stream/mode.rs
---------------------------------

```rust
use serde::{Deserialize, Serialize};

/// Output mode for frame delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamMode {
    /// Consumers call next_frame() to pull frames on demand.
    Pull,
    /// Frames are pushed to subscribers via channels.
    Push,
    /// Frames are written to a shared-memory ring buffer.
    SharedMemory,
    /// Frames are serialized and sent over IPC (named pipe).
    Ipc,
}

impl StreamMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "pull" => Some(Self::Pull),
            "push" => Some(Self::Push),
            "shared_memory" | "sharedmemory" => Some(Self::SharedMemory),
            "ipc" => Some(Self::Ipc),
            _ => None,
        }
    }
}
```

File: crates/iris-stream/subscriber.rs
----------------------------------------

```rust
use tokio::sync::mpsc;
use iris_capture::frame::CaptureFrame;
use std::sync::atomic::{AtomicU64, Ordering};

/// Unique subscriber identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriberId(pub u64);

/// A frame subscription — receives frames pushed by the stream service.
pub struct FrameSubscription {
    pub id: SubscriberId,
    /// Channel to receive frames.
    pub frame_rx: mpsc::Receiver<CaptureFrame>,
    /// Frames delivered to this subscriber.
    pub delivered: AtomicU64,
    /// Frames dropped for this subscriber (slow consumer).
    pub dropped: AtomicU64,
}

impl FrameSubscription {
    /// Receive the next frame. Returns None if the stream is closed.
    pub async fn next_frame(&mut self) -> Option<CaptureFrame> {
        self.frame_rx.recv().await
    }
}

/// Internal record for managing a subscriber.
pub(crate) struct SubscriberRecord {
    pub id: SubscriberId,
    pub frame_tx: mpsc::Sender<CaptureFrame>,
    pub delivered: u64,
    pub dropped: u64,
}
```

File: crates/iris-stream/ring_buffer.rs
-----------------------------------------
Shared-memory ring buffer for zero-copy frame delivery.

```rust
use std::sync::{Arc, Mutex};

/// A slot in the ring buffer.
#[derive(Debug, Clone)]
pub struct RingSlot {
    /// Frame sequence number occupying this slot (0 = empty).
    pub sequence: u64,
    /// Raw frame data.
    pub data: Vec<u8>,
    /// Frame width.
    pub width: u32,
    /// Frame height.
    pub height: u32,
    /// Timestamp in microseconds.
    pub timestamp_us: u64,
}

/// Fixed-capacity ring buffer for frame storage.
pub struct RingBuffer {
    slots: Vec<RingSlot>,
    capacity: usize,
    /// Index of the next slot to write.
    write_idx: usize,
    /// Total frames written (wraps around the ring).
    total_written: u64,
    /// Total frames overwritten (overflow count).
    overflow_count: u64,
}

impl RingBuffer {
    /// Create a new ring buffer with the given capacity.
    /// Capacity must be >= 2.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity >= 2, "ring buffer capacity must be >= 2");
        let empty_slot = RingSlot {
            sequence: 0,
            data: Vec::new(),
            width: 0,
            height: 0,
            timestamp_us: 0,
        };
        Self {
            slots: vec![empty_slot; capacity],
            capacity,
            write_idx: 0,
            total_written: 0,
            overflow_count: 0,
        }
    }

    /// Write a frame into the ring buffer.
    /// If the buffer is full, overwrites the oldest slot (and increments overflow_count).
    pub fn write(&mut self, frame: &iris_capture::frame::CaptureFrame) {
        let slot = &mut self.slots[self.write_idx];
        if slot.sequence > 0 {
            self.overflow_count += 1;
        }
        slot.sequence = frame.sequence;
        slot.data = frame.data.clone();
        slot.width = frame.width;
        slot.height = frame.height;
        slot.timestamp_us = frame.timestamp_us;
        self.write_idx = (self.write_idx + 1) % self.capacity;
        self.total_written += 1;
    }

    /// Read the most recent frame (the last one written).
    pub fn read_latest(&self) -> Option<&RingSlot> {
        if self.total_written == 0 {
            return None;
        }
        let idx = if self.write_idx == 0 {
            self.capacity - 1
        } else {
            self.write_idx - 1
        };
        let slot = &self.slots[idx];
        if slot.sequence > 0 { Some(slot) } else { None }
    }

    /// Read a specific slot by index (0 = oldest available).
    pub fn read_slot(&self, index: usize) -> Option<&RingSlot> {
        if index >= self.capacity {
            return None;
        }
        let slot = &self.slots[index];
        if slot.sequence > 0 { Some(slot) } else { None }
    }

    /// Current usage as a fraction (0.0 - 1.0).
    pub fn usage(&self) -> f32 {
        let occupied = self.slots.iter().filter(|s| s.sequence > 0).count();
        occupied as f32 / self.capacity as f32
    }

    /// Total overflow count.
    pub fn overflow_count(&self) -> u64 {
        self.overflow_count
    }

    /// Total frames written.
    pub fn total_written(&self) -> u64 {
        self.total_written
    }

    /// Buffer capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// Thread-safe wrapper around the ring buffer.
pub type SharedRingBuffer = Arc<Mutex<RingBuffer>>;

/// Create a new shared ring buffer.
pub fn shared_ring_buffer(capacity: usize) -> SharedRingBuffer {
    Arc::new(Mutex::new(RingBuffer::new(capacity)))
}
```

File: crates/iris-stream/service.rs
-------------------------------------

```rust
use tokio::sync::{mpsc, broadcast, watch};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Commands for the stream service.
#[derive(Debug)]
pub enum StreamCommand {
    /// Add a new subscriber. Returns SubscriberId via oneshot.
    Subscribe {
        reply: tokio::sync::oneshot::Sender<FrameSubscription>,
    },
    /// Remove a subscriber.
    Unsubscribe {
        id: SubscriberId,
    },
    /// Change stream mode.
    SetMode {
        mode: StreamMode,
    },
    /// Get stream statistics.
    GetStats {
        reply: tokio::sync::oneshot::Sender<StreamStats>,
    },
    /// Shutdown.
    Shutdown,
}

/// Stream statistics.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StreamStats {
    pub frames_delivered: u64,
    pub frames_dropped: u64,
    pub subscriber_count: usize,
    pub ring_buffer_usage: f32,
    pub mode: StreamMode,
}

/// The stream service — receives captured frames and dispatches them to subscribers.
pub struct StreamService {
    /// Incoming frames from CaptureService.
    frame_rx: mpsc::Receiver<iris_capture::frame::CaptureFrame>,
    /// Commands.
    cmd_rx: mpsc::Receiver<StreamCommand>,
    /// Telemetry bridge.
    telemetry_tx: broadcast::Sender<iris_ipc::telemetry::TelemetryEnvelope>,
    /// Active mode.
    mode: StreamMode,
    /// Active subscribers (for Push mode).
    subscribers: Vec<SubscriberRecord>,
    /// Shared ring buffer (for SharedMemory mode).
    ring_buffer: SharedRingBuffer,
    /// Next subscriber ID.
    next_sub_id: AtomicU64,
    /// Total frames delivered across all subscribers.
    total_delivered: u64,
    /// Total frames dropped across all subscribers.
    total_dropped: u64,
    /// Max subscribers allowed.
    max_subscribers: usize,
}

impl StreamService {
    pub fn new(
        frame_rx: mpsc::Receiver<iris_capture::frame::CaptureFrame>,
        telemetry_tx: broadcast::Sender<iris_ipc::telemetry::TelemetryEnvelope>,
        mode: StreamMode,
        ring_buffer_capacity: usize,
        max_subscribers: usize,
    ) -> (Self, StreamHandle) { ... }

    /// Run the stream service loop:
    /// 1. Select on frame_rx and cmd_rx
    /// 2. On frame received:
    ///    - SharedMemory mode: write to ring buffer
    ///    - Push mode: try_send to each subscriber's channel
    ///      - If subscriber channel full: drop frame for that subscriber, increment dropped
    ///      - Emit StreamDelivery telemetry per subscriber
    ///    - Pull mode: write to ring buffer (consumers pull from it)
    ///    - Ipc mode: serialize frame metadata as JSON to named pipe (stub for now)
    /// 3. On command:
    ///    - Subscribe: create new SubscriberRecord + FrameSubscription, emit SubscriberAdded
    ///    - Unsubscribe: remove record, emit SubscriberRemoved
    ///    - SetMode: update mode
    ///    - GetStats: return current stats
    ///    - Shutdown: exit
    /// 4. If ring buffer overflows: emit RingBufferOverflow telemetry
    pub async fn run(mut self) { ... }
}

/// Handle for interacting with the stream service.
pub struct StreamHandle {
    cmd_tx: mpsc::Sender<StreamCommand>,
    /// Direct access to the shared ring buffer (for Pull/SharedMemory consumers).
    pub ring_buffer: SharedRingBuffer,
}

impl StreamHandle {
    pub async fn subscribe(&self) -> IrisResult<FrameSubscription> { ... }
    pub async fn unsubscribe(&self, id: SubscriberId) -> IrisResult<()> { ... }
    pub async fn set_mode(&self, mode: StreamMode) -> IrisResult<()> { ... }
    pub async fn stats(&self) -> IrisResult<StreamStats> { ... }
    pub async fn shutdown(&self) -> IrisResult<()> { ... }

    /// Read the latest frame from the ring buffer (for Pull mode consumers).
    pub fn latest_frame(&self) -> Option<RingSlot> {
        let buf = self.ring_buffer.lock().ok()?;
        buf.read_latest().cloned()
    }
}
```

File: crates/iris-stream/telemetry.rs
---------------------------------------

```rust
use serde::{Deserialize, Serialize};

/// Telemetry snapshot for the stream subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamTelemetry {
    pub mode: String,
    pub subscriber_count: usize,
    pub frames_delivered: u64,
    pub frames_dropped: u64,
    pub ring_buffer_usage: f32,
    pub ring_buffer_overflow_count: u64,
}
```

Unit Tests
----------
File: crates/iris-stream/tests.rs

### Required Tests

1. `test_ring_buffer_write_read` — write 3 frames, read_latest returns last one
2. `test_ring_buffer_overflow` — capacity=2, write 5 frames, overflow_count=3
3. `test_ring_buffer_usage` — capacity=4, write 2 frames, usage=0.5
4. `test_ring_buffer_empty` — new buffer, read_latest=None, usage=0.0
5. `test_subscriber_receive_frame` — create subscription channel, send frame, receive it
6. `test_stream_mode_from_str` — "pull"→Pull, "push"→Push, "shared_memory"→SharedMemory, "ipc"→Ipc, "bad"→None
7. `test_stream_service_subscribe` — create service, subscribe, verify subscriber count=1
8. `test_stream_service_push_delivery` — subscribe in Push mode, send frames from capture, verify subscriber receives them
9. `test_stream_service_unsubscribe` — subscribe, unsubscribe, verify subscriber count=0
10. `test_stream_service_shared_memory` — set SharedMemory mode, send frames, verify ring buffer contains them
11. `test_stream_service_stats` — get stats, verify fields populated
12. `test_stream_service_slow_consumer` — subscriber with small channel, send many frames, verify dropped count > 0

Acceptance Criteria
-------------------
1. `cargo check -p iris-stream` passes
2. `cargo test -p iris-stream` — all 12 tests pass
3. Ring buffer correctly handles writes, reads, and overflow
4. Push mode delivers frames to all subscribers
5. SharedMemory mode writes frames to ring buffer
6. Slow consumers don't block the service (frames dropped, not stalled)
7. Subscriber add/remove works with telemetry emission
8. Stats accurately reflect current state
9. Max subscriber limit enforced
