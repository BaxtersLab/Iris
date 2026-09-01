// SPDX-License-Identifier: MIT
// Iris — iris-stream

//! The stream service: one frame source, many consumers.

use crate::mode::StreamMode;
use crate::ring_buffer::{shared_ring_buffer, SharedRingBuffer};
use crate::subscriber::{FrameSubscription, SubscriberId, SubscriberRecord, SubscriberStats};
use iris_capture::frame::CaptureFrame;
use iris_core::error::{IrisError, IrisResult};
use iris_ipc::telemetry::{TelemetryEnvelope, TelemetryEvent};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot};

/// Requests to the stream service.
#[derive(Debug)]
pub enum StreamCommand {
    Subscribe {
        reply: oneshot::Sender<IrisResult<FrameSubscription>>,
    },
    Unsubscribe {
        id: SubscriberId,
    },
    SetMode {
        mode: StreamMode,
        reply: oneshot::Sender<IrisResult<()>>,
    },
    GetStats {
        reply: oneshot::Sender<StreamStats>,
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamStats {
    pub frames_received: u64,
    pub frames_delivered: u64,
    pub frames_dropped: u64,
    pub subscriber_count: usize,
    pub ring_buffer_usage: f32,
    pub ring_buffer_overflows: u64,
    pub mode: StreamMode,
    pub subscribers: Vec<SubscriberStats>,
}

pub struct StreamService {
    frame_rx: mpsc::Receiver<CaptureFrame>,
    cmd_rx: mpsc::Receiver<StreamCommand>,
    telemetry_tx: broadcast::Sender<TelemetryEnvelope>,
    mode: StreamMode,
    subscribers: Vec<SubscriberRecord>,
    ring_buffer: SharedRingBuffer,
    next_sub_id: u64,
    sequence: u64,
    frames_received: u64,
    total_delivered: u64,
    total_dropped: u64,
    max_subscribers: usize,
    /// Per-subscriber channel depth. Small on purpose: a deep queue does not
    /// help a consumer that is persistently slow, it just delays the frames it
    /// eventually gets and hides the problem from the drop counter.
    subscriber_queue_depth: usize,
}

#[derive(Clone, Debug)]
pub struct StreamHandle {
    cmd_tx: mpsc::Sender<StreamCommand>,
    /// Direct read access for `Pull` consumers.
    pub ring_buffer: SharedRingBuffer,
}

impl StreamService {
    pub fn new(
        frame_rx: mpsc::Receiver<CaptureFrame>,
        telemetry_tx: broadcast::Sender<TelemetryEnvelope>,
        mode: StreamMode,
        ring_buffer_capacity: usize,
        max_subscribers: usize,
    ) -> (Self, StreamHandle) {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let ring_buffer = shared_ring_buffer(ring_buffer_capacity.max(2));
        let handle = StreamHandle {
            cmd_tx,
            ring_buffer: ring_buffer.clone(),
        };
        let svc = Self {
            frame_rx,
            cmd_rx,
            telemetry_tx,
            mode,
            subscribers: Vec::new(),
            ring_buffer,
            next_sub_id: 1,
            sequence: 0,
            frames_received: 0,
            total_delivered: 0,
            total_dropped: 0,
            max_subscribers,
            subscriber_queue_depth: 4,
        };
        (svc, handle)
    }

    fn emit(&mut self, event: TelemetryEvent) {
        let envelope = TelemetryEnvelope {
            timestamp: chrono::Utc::now(),
            sequence: self.sequence,
            event,
        };
        self.sequence += 1;
        let _ = self.telemetry_tx.send(envelope);
    }

    fn stats(&self) -> StreamStats {
        let (usage, overflows) = match self.ring_buffer.lock() {
            Ok(rb) => (rb.usage(), rb.overflow_count()),
            Err(_) => (0.0, 0),
        };
        StreamStats {
            frames_received: self.frames_received,
            frames_delivered: self.total_delivered,
            frames_dropped: self.total_dropped,
            subscriber_count: self.subscribers.len(),
            ring_buffer_usage: usage,
            ring_buffer_overflows: overflows,
            mode: self.mode,
            subscribers: self.subscribers.iter().map(|s| s.stats()).collect(),
        }
    }

    fn on_frame(&mut self, frame: CaptureFrame) {
        self.frames_received += 1;

        // The ring is maintained in EVERY mode, not only in Pull.
        //
        // It is the "what do you see right now" surface, and a pull consumer —
        // an agent asking for the current frame — must be able to ask at any
        // time without the service having been configured for it in advance.
        // Making that conditional on the mode meant the UI (a push subscriber)
        // and an agent (a pull reader) could not both be served, which is the
        // ordinary case rather than an exotic one.
        //
        // The cost is one copy per frame into a fixed ring, which is bounded
        // and small beside the fan-out it sits next to.
        let before = self.ring_buffer.lock().map(|r| r.overflow_count()).unwrap_or(0);
        if let Ok(mut rb) = self.ring_buffer.lock() {
            rb.write(&frame);
        }
        let after = self
            .ring_buffer
            .lock()
            .map(|r| r.overflow_count())
            .unwrap_or(before);
        if after > before {
            self.emit(TelemetryEvent::RingBufferOverflow {
                dropped_frames: after,
            });
        }

        match self.mode {
            // Pull consumers read the ring directly; nothing further to do.
            StreamMode::Pull => {}
            StreamMode::Push => self.push_to_subscribers(&frame),
            // Refused at SetMode and at construction; unreachable in practice,
            // and a silent fall-through is exactly what must not happen.
            StreamMode::SharedMemory | StreamMode::Ipc => {
                tracing::error!(
                    "stream mode {} is not implemented; frame {} not fanned out",
                    self.mode,
                    frame.sequence
                );
            }
        }
    }

    fn push_to_subscribers(&mut self, frame: &CaptureFrame) {
        let mut gone = Vec::new();
        let mut events = Vec::new();

        for record in self.subscribers.iter_mut() {
            match record.frame_tx.try_send(frame.clone()) {
                Ok(()) => {
                    record.delivered += 1;
                    self.total_delivered += 1;
                    // Latency from the frame's own capture timestamp, so it
                    // measures the pipeline rather than this function.
                    let now = CaptureFrame::now_us();
                    let latency_us = now.saturating_sub(frame.timestamp_us);
                    events.push(TelemetryEvent::StreamDelivery {
                        subscriber_id: record.id.0,
                        frame_sequence: frame.sequence,
                        latency_us,
                    });
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    // This subscriber is behind. Drop ITS frame only — the
                    // whole point of per-subscriber channels is that one slow
                    // consumer cannot slow or starve the others.
                    record.dropped += 1;
                    self.total_dropped += 1;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => gone.push(record.id),
            }
        }

        for event in events {
            self.emit(event);
        }
        // A subscriber that dropped its FrameSubscription is removed here
        // rather than requiring an explicit unsubscribe, so a consumer that
        // panicked cannot leak a subscription the service keeps feeding.
        for id in gone {
            self.remove_subscriber(id);
        }
    }

    fn remove_subscriber(&mut self, id: SubscriberId) {
        let before = self.subscribers.len();
        self.subscribers.retain(|s| s.id != id);
        if self.subscribers.len() != before {
            let total = self.subscribers.len();
            self.emit(TelemetryEvent::SubscriberRemoved { id: id.0, total });
        }
    }

    fn subscribe(&mut self) -> IrisResult<FrameSubscription> {
        if self.subscribers.len() >= self.max_subscribers {
            return Err(IrisError::Stream(format!(
                "at the subscriber limit ({}); unsubscribe one first",
                self.max_subscribers
            )));
        }
        let id = SubscriberId(self.next_sub_id);
        self.next_sub_id += 1;
        let (tx, rx) = mpsc::channel(self.subscriber_queue_depth);
        self.subscribers.push(SubscriberRecord {
            id,
            frame_tx: tx,
            delivered: 0,
            dropped: 0,
        });
        let total = self.subscribers.len();
        self.emit(TelemetryEvent::SubscriberAdded { id: id.0, total });
        Ok(FrameSubscription::new(id, rx))
    }

    fn set_mode(&mut self, mode: StreamMode) -> IrisResult<()> {
        if !mode.is_implemented() {
            return Err(IrisError::Stream(format!(
                "stream mode {mode} is not implemented — \
                 refusing rather than silently running as another mode"
            )));
        }
        self.mode = mode;
        Ok(())
    }

    /// Run until shut down, or until the frame source and every handle are gone.
    pub async fn run(mut self) {
        if !self.mode.is_implemented() {
            tracing::error!(
                "stream service started in unimplemented mode {}; \
                 no frames will be delivered until SetMode is called",
                self.mode
            );
        }
        loop {
            tokio::select! {
                maybe_frame = self.frame_rx.recv() => {
                    match maybe_frame {
                        Some(frame) => self.on_frame(frame),
                        // The capture side has gone away; nothing more will
                        // arrive, so stop rather than idle forever.
                        None => break,
                    }
                }
                maybe_cmd = self.cmd_rx.recv() => {
                    match maybe_cmd {
                        Some(StreamCommand::Subscribe { reply }) => {
                            let r = self.subscribe();
                            let _ = reply.send(r);
                        }
                        Some(StreamCommand::Unsubscribe { id }) => self.remove_subscriber(id),
                        Some(StreamCommand::SetMode { mode, reply }) => {
                            let r = self.set_mode(mode);
                            let _ = reply.send(r);
                        }
                        Some(StreamCommand::GetStats { reply }) => {
                            let _ = reply.send(self.stats());
                        }
                        Some(StreamCommand::Shutdown) | None => break,
                    }
                }
            }
        }
    }
}

impl StreamHandle {
    pub async fn subscribe(&self) -> IrisResult<FrameSubscription> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(StreamCommand::Subscribe { reply: tx })
            .await
            .map_err(|_| IrisError::Stream("stream service is not running".into()))?;
        rx.await
            .map_err(|_| IrisError::Stream("stream service dropped the request".into()))?
    }

    pub async fn unsubscribe(&self, id: SubscriberId) -> IrisResult<()> {
        self.cmd_tx
            .send(StreamCommand::Unsubscribe { id })
            .await
            .map_err(|_| IrisError::Stream("stream service is not running".into()))
    }

    pub async fn set_mode(&self, mode: StreamMode) -> IrisResult<()> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(StreamCommand::SetMode { mode, reply: tx })
            .await
            .map_err(|_| IrisError::Stream("stream service is not running".into()))?;
        rx.await
            .map_err(|_| IrisError::Stream("stream service dropped the request".into()))?
    }

    pub async fn stats(&self) -> IrisResult<StreamStats> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(StreamCommand::GetStats { reply: tx })
            .await
            .map_err(|_| IrisError::Stream("stream service is not running".into()))?;
        rx.await
            .map_err(|_| IrisError::Stream("stream service dropped the request".into()))
    }

    pub async fn shutdown(&self) -> IrisResult<()> {
        self.cmd_tx
            .send(StreamCommand::Shutdown)
            .await
            .map_err(|_| IrisError::Stream("stream service is not running".into()))
    }
}
