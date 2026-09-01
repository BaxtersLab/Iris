// SPDX-License-Identifier: MIT
// Iris — iris-stream

//! A fixed-capacity ring of recent frames, for `Pull` consumers.
//!
//! **In-process, and it copies.** Each write clones the frame's bytes into a
//! slot. That is what makes it safe to hand to several readers at their own
//! pace, and it is why this is not the `SharedMemory` transport: nothing here
//! is mapped between processes and nothing is zero-copy. The distinction is
//! kept explicit because the block-G1 specification called this module
//! "shared-memory ring buffer for zero-copy frame delivery", which it is not.

use iris_capture::frame::CaptureFrame;
use iris_hal::device::PixelFormat;
use std::sync::{Arc, Mutex};

/// One frame's worth of the ring.
#[derive(Debug, Clone)]
pub struct RingSlot {
    /// Frame sequence. **0 means empty** — the capture service numbers frames
    /// from 1, so zero is unambiguous.
    pub sequence: u64,
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// The pixel format of `data`.
    ///
    /// Stored because a frame's bytes are meaningless without it: the same
    /// buffer is a JPEG, a plane pair or an RGB grid depending on this, and a
    /// reader that has to guess will guess wrong on the camera that matters.
    /// The ring originally dropped it, which made every stored frame ambiguous
    /// the moment anything other than the UI wanted to read one.
    pub format: PixelFormat,
    pub timestamp_us: u64,
}

impl Default for RingSlot {
    fn default() -> Self {
        Self {
            sequence: 0,
            data: Vec::new(),
            width: 0,
            height: 0,
            format: PixelFormat::Rgb24,
            timestamp_us: 0,
        }
    }
}

impl RingSlot {
    pub fn is_empty(&self) -> bool {
        self.sequence == 0
    }
}

/// A ring of the most recent frames.
#[derive(Debug)]
pub struct RingBuffer {
    slots: Vec<RingSlot>,
    write_idx: usize,
    total_written: u64,
    overflow_count: u64,
}

impl RingBuffer {
    /// Capacity must be at least 2: a ring of one cannot hold a frame while
    /// the next is being written, which is the only thing a ring is for.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity >= 2, "ring buffer capacity must be >= 2, got {capacity}");
        Self {
            slots: vec![RingSlot::default(); capacity],
            write_idx: 0,
            total_written: 0,
            overflow_count: 0,
        }
    }

    /// Write a frame, overwriting the oldest slot once the ring is full.
    pub fn write(&mut self, frame: &CaptureFrame) {
        let capacity = self.slots.len();
        let slot = &mut self.slots[self.write_idx];
        if !slot.is_empty() {
            self.overflow_count += 1;
        }
        slot.sequence = frame.sequence;
        slot.data.clear();
        slot.data.extend_from_slice(&frame.data);
        slot.width = frame.width;
        slot.height = frame.height;
        slot.format = frame.format.clone();
        slot.timestamp_us = frame.timestamp_us;
        self.write_idx = (self.write_idx + 1) % capacity;
        self.total_written += 1;
    }

    /// The most recently written frame.
    pub fn read_latest(&self) -> Option<&RingSlot> {
        if self.total_written == 0 {
            return None;
        }
        let idx = if self.write_idx == 0 {
            self.slots.len() - 1
        } else {
            self.write_idx - 1
        };
        Some(&self.slots[idx]).filter(|s| !s.is_empty())
    }

    /// Read by age: **0 is the oldest frame still held**, not slot zero.
    ///
    /// The specification indexed `slots[index]` directly and documented it as
    /// "0 = oldest available". Those are the same thing only before the ring
    /// first wraps; afterwards raw slot 0 is an arbitrary position and the
    /// caller reading "oldest" gets whatever happens to live there. Age is
    /// what a caller can reason about, so age is what this takes.
    pub fn read_by_age(&self, index: usize) -> Option<&RingSlot> {
        let capacity = self.slots.len();
        if index >= capacity {
            return None;
        }
        let held = self.len();
        if index >= held {
            return None;
        }
        // The oldest held frame sits `held` slots behind the write cursor.
        let start = (self.write_idx + capacity - held) % capacity;
        let slot = &self.slots[(start + index) % capacity];
        Some(slot).filter(|s| !s.is_empty())
    }

    /// How many slots currently hold a frame.
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| !s.is_empty()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Occupancy, 0.0 to 1.0.
    pub fn usage(&self) -> f32 {
        self.len() as f32 / self.slots.len() as f32
    }

    /// How many frames have been overwritten before anyone read them.
    pub fn overflow_count(&self) -> u64 {
        self.overflow_count
    }

    pub fn total_written(&self) -> u64 {
        self.total_written
    }

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }
}

pub type SharedRingBuffer = Arc<Mutex<RingBuffer>>;

pub fn shared_ring_buffer(capacity: usize) -> SharedRingBuffer {
    Arc::new(Mutex::new(RingBuffer::new(capacity)))
}
