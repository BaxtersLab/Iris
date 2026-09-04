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
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, oneshot};

/// How often `RingBufferRecycled` may be emitted.
///
/// One second: fast enough that a ring churning hard is visible within a
/// glance at the log, slow enough that normal running does not bury every
/// other telemetry event under it.
const DEFAULT_RECYCLE_REPORT_INTERVAL: Duration = Duration::from_secs(1);

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
    /// Frames the ring has overwritten since the service started.
    /// Normal and unbounded in steady state — see `RingBufferRecycled`.
    pub ring_buffer_overwrites: u64,
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
    /// Total overwrites already accounted for in a `RingBufferRecycled` event,
    /// so the next one can report a **delta** rather than the lifetime total.
    overwrites_reported: u64,
    /// When the last `RingBufferRecycled` went out. `None` until the first.
    last_recycle_report: Option<Instant>,
    /// `telemetry.enabled`: when false, nothing is emitted at all.
    telemetry_enabled: bool,
    /// `telemetry.max_events_per_second`: a ceiling on emission, 0 = unlimited.
    max_events_per_second: u32,
    /// Start of the current one-second budget window, and what has been spent.
    budget_started: Option<Instant>,
    budget_spent: u32,
    /// Events dropped in the current window, reported once when it rolls over
    /// so a silent gap is never mistaken for a quiet system.
    budget_dropped: u64,
    /// Minimum gap between `RingBufferRecycled` events.
    ///
    /// Without this the event fires on **every frame** once the ring is full,
    /// because a full ring overwrites something every time — roughly 27 events
    /// a second at 1080p30, each one a line in the UI's telemetry log. The
    /// recycling rate is worth knowing; it is not worth knowing 27 times a
    /// second.
    recycle_report_interval: Duration,
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
        Self::with_recycle_interval(
            frame_rx,
            telemetry_tx,
            mode,
            ring_buffer_capacity,
            max_subscribers,
            DEFAULT_RECYCLE_REPORT_INTERVAL,
        )
    }

    /// `new`, with the recycle-report throttle passed in.
    ///
    /// Split out **for the tests**, which need to observe both sides of the
    /// throttle without spending a real second per assertion: `Duration::ZERO`
    /// makes every overwrite report (proving the delta), and a long interval
    /// makes only the first report (proving the throttle). `new` is the one
    /// caller that uses the production interval.
    pub(crate) fn with_recycle_interval(
        frame_rx: mpsc::Receiver<CaptureFrame>,
        telemetry_tx: broadcast::Sender<TelemetryEnvelope>,
        mode: StreamMode,
        ring_buffer_capacity: usize,
        max_subscribers: usize,
        recycle_report_interval: Duration,
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
            telemetry_enabled: true,
            max_events_per_second: 0,
            budget_started: None,
            budget_spent: 0,
            budget_dropped: 0,
            overwrites_reported: 0,
            last_recycle_report: None,
            recycle_report_interval,
        };
        (svc, handle)
    }

    /// Report ring recycling as a throttled **delta**.
    ///
    /// This replaces a per-frame emit of the lifetime total, which was wrong in
    /// three ways at once. The field was called `dropped_frames` and carried
    /// every overwrite since process start, so `164` read as "164 frames were
    /// just lost" when it meant "this is the 164th overwrite" — reached in
    /// about six seconds on an 8-slot ring at 30fps. It fired on every frame
    /// once the ring filled, because a full ring always overwrites something,
    /// so it was ~27 log lines a second describing normal operation. And
    /// "dropped" was the wrong word: a pull consumer asks for the newest frame,
    /// so recycling an older one costs nobody anything.
    ///
    /// What is actually diagnostic is the **rate**, which is what a delta over
    /// a known interval gives you.
    fn report_recycled(&mut self, total: u64) {
        if total <= self.overwrites_reported {
            return; // the ring has not wrapped since the last report
        }
        let now = Instant::now();
        let due = match self.last_recycle_report {
            // Report the first recycle immediately: the moment a ring starts
            // wrapping is the interesting one, and waiting a full interval to
            // mention it would hide it inside the steady state that follows.
            None => true,
            Some(last) => now.duration_since(last) >= self.recycle_report_interval,
        };
        if !due {
            return;
        }
        let overwritten = total - self.overwrites_reported;
        self.overwrites_reported = total;
        self.last_recycle_report = Some(now);
        self.emit(TelemetryEvent::RingBufferRecycled {
            overwritten,
            total_overwritten: total,
        });
    }

    /// Apply `[telemetry]` from the config.
    ///
    /// Both fields reached nothing before 2026-09-04: `enabled` and
    /// `max_events_per_second` were serialised, documented and validated, and no
    /// code outside `iris-core` ever read them. Setting `enabled = false` left
    /// telemetry running at full rate.
    pub fn set_telemetry_policy(&mut self, enabled: bool, max_events_per_second: u32) {
        self.telemetry_enabled = enabled;
        self.max_events_per_second = max_events_per_second;
    }

    /// Is there budget for one more event this second?
    ///
    /// A plain per-second ceiling rather than a smoothed rate: the thing being
    /// bounded is how much a human or a log file has to read, and "at most N
    /// lines a second" is what that means. **A window that drops anything
    /// reports how much when it rolls over** — a rate limiter that silently
    /// discards turns a busy system into one that looks idle, which is the same
    /// failure as a camera that returns a blank frame instead of an error.
    fn budget_allows(&mut self) -> bool {
        if self.max_events_per_second == 0 {
            return true;
        }
        let now = Instant::now();
        let rolled = match self.budget_started {
            None => true,
            Some(t) => now.duration_since(t) >= Duration::from_secs(1),
        };
        if rolled {
            let dropped = std::mem::take(&mut self.budget_dropped);
            self.budget_started = Some(now);
            self.budget_spent = 0;
            if dropped > 0 {
                // Emitted directly, bypassing the budget it is reporting on —
                // otherwise the message about dropping would itself be dropped.
                let envelope = TelemetryEnvelope {
                    timestamp: chrono::Utc::now(),
                    sequence: self.sequence,
                    event: TelemetryEvent::TelemetryRateLimited { dropped },
                };
                self.sequence += 1;
                self.budget_spent += 1;
                let _ = self.telemetry_tx.send(envelope);
            }
        }
        if self.budget_spent < self.max_events_per_second {
            self.budget_spent += 1;
            true
        } else {
            self.budget_dropped += 1;
            false
        }
    }

    fn emit(&mut self, event: TelemetryEvent) {
        if !self.telemetry_enabled || !self.budget_allows() {
            return;
        }
        let envelope = TelemetryEnvelope {
            timestamp: chrono::Utc::now(),
            sequence: self.sequence,
            event,
        };
        self.sequence += 1;
        let _ = self.telemetry_tx.send(envelope);
    }

    fn stats(&self) -> StreamStats {
        let (usage, overwrites) = match self.ring_buffer.lock() {
            Ok(rb) => (rb.usage(), rb.overwrite_count()),
            Err(_) => (0.0, 0),
        };
        StreamStats {
            frames_received: self.frames_received,
            frames_delivered: self.total_delivered,
            frames_dropped: self.total_dropped,
            subscriber_count: self.subscribers.len(),
            ring_buffer_usage: usage,
            ring_buffer_overwrites: overwrites,
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
        //
        // One lock acquisition, not three. The previous version took the lock to
        // read the count, released it to write, and took it a third time to read
        // the count back — so another writer could interleave and make the
        // difference wrong. Reading the count inside the same critical section
        // as the write is both correct and a third of the lock traffic.
        let total_overwritten = match self.ring_buffer.lock() {
            Ok(mut rb) => {
                rb.write(&frame);
                Some(rb.overwrite_count())
            }
            // A poisoned lock means some other thread panicked mid-write. The
            // frame is not stored and nothing is reported, which is the same
            // thing the previous version did — surfacing it belongs to whatever
            // owns the poisoning, not to the per-frame path.
            Err(_) => None,
        };
        if let Some(total) = total_overwritten {
            self.report_recycled(total);
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
