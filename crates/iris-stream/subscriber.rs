// SPDX-License-Identifier: MIT
// Iris — iris-stream

//! Subscribers, and the accounting that makes a slow one visible.

use iris_capture::frame::CaptureFrame;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SubscriberId(pub u64);

impl std::fmt::Display for SubscriberId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "subscriber {}", self.0)
    }
}

/// A consumer's end of a `Push` subscription.
///
/// Dropping this is how a subscriber leaves: the service notices the closed
/// channel on its next delivery and removes the record. An explicit
/// `unsubscribe` is available but not required, so a consumer that panics or
/// simply goes away cannot leak a subscription that the service keeps feeding.
pub struct FrameSubscription {
    pub id: SubscriberId,
    frame_rx: mpsc::Receiver<CaptureFrame>,
}

// Hand-written rather than derived: a Receiver is not Debug, and the useful
// thing to see in a log is which subscriber this is, not the channel's guts.
impl std::fmt::Debug for FrameSubscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameSubscription")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl FrameSubscription {
    pub(crate) fn new(id: SubscriberId, frame_rx: mpsc::Receiver<CaptureFrame>) -> Self {
        Self { id, frame_rx }
    }

    /// Wait for the next frame. `None` once the service has stopped.
    pub async fn next_frame(&mut self) -> Option<CaptureFrame> {
        self.frame_rx.recv().await
    }

    /// Take a frame if one is waiting, without blocking.
    pub fn try_next_frame(&mut self) -> Option<CaptureFrame> {
        self.frame_rx.try_recv().ok()
    }

    /// Unwrap to the bare receiver.
    ///
    /// For a consumer that already drains an `mpsc::Receiver<CaptureFrame>` —
    /// the UI does — so that becoming a subscriber costs it no changes.
    /// Dropping the receiver still closes the channel, which is how the service
    /// notices the subscriber has gone.
    pub fn into_receiver(self) -> mpsc::Receiver<CaptureFrame> {
        self.frame_rx
    }
}

/// Per-subscriber counters, reported by `GetStats`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriberStats {
    pub id: SubscriberId,
    pub delivered: u64,
    /// Frames this subscriber missed because its channel was full.
    ///
    /// Counted **per subscriber**, which is the point of the design: one slow
    /// consumer must not cost every other consumer frames, and the only way to
    /// know it is slow is to attribute the drops to it.
    pub dropped: u64,
}

/// The service's record of a subscriber.
pub(crate) struct SubscriberRecord {
    pub id: SubscriberId,
    pub frame_tx: mpsc::Sender<CaptureFrame>,
    pub delivered: u64,
    pub dropped: u64,
}

impl SubscriberRecord {
    pub(crate) fn stats(&self) -> SubscriberStats {
        SubscriberStats {
            id: self.id,
            delivered: self.delivered,
            dropped: self.dropped,
        }
    }
}
