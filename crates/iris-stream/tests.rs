// SPDX-License-Identifier: MIT
// Iris — iris-stream tests

use crate::mode::StreamMode;
use crate::ring_buffer::RingBuffer;
use crate::service::StreamService;
use iris_capture::frame::CaptureFrame;
use iris_hal::device::PixelFormat;
use iris_ipc::telemetry::{TelemetryEnvelope, TelemetryEvent};
use std::str::FromStr;
use tokio::sync::{broadcast, mpsc};

fn frame(sequence: u64, byte: u8) -> CaptureFrame {
    CaptureFrame {
        sequence,
        width: 4,
        height: 2,
        format: PixelFormat::Rgb24,
        data: vec![byte; 4 * 2 * 3],
        timestamp_us: CaptureFrame::now_us(),
        is_cropped: false,
    }
}

// ---- mode ----------------------------------------------------------------

#[test]
fn every_named_mode_parses_and_round_trips() {
    for (text, mode) in [
        ("pull", StreamMode::Pull),
        ("push", StreamMode::Push),
        ("shared_memory", StreamMode::SharedMemory),
        ("ipc", StreamMode::Ipc),
    ] {
        assert_eq!(StreamMode::from_str(text).unwrap(), mode);
        assert_eq!(mode.as_str(), text);
    }
    assert_eq!(StreamMode::from_str("SharedMemory").unwrap(), StreamMode::SharedMemory);
    assert!(StreamMode::from_str("carrier_pigeon").is_err());
}

/// The unimplemented modes parse deliberately: a config naming one should fail
/// at the service saying "not implemented", not here saying "no such mode".
/// Those are different problems and only one is the user's typo.
#[test]
fn only_the_built_modes_report_themselves_implemented() {
    assert!(StreamMode::Pull.is_implemented());
    assert!(StreamMode::Push.is_implemented());
    assert!(!StreamMode::SharedMemory.is_implemented());
    assert!(!StreamMode::Ipc.is_implemented());
}

// ---- ring buffer ---------------------------------------------------------

#[test]
#[should_panic(expected = "capacity must be >= 2")]
fn a_ring_of_one_is_refused() {
    RingBuffer::new(1);
}

#[test]
fn an_empty_ring_reads_nothing() {
    let rb = RingBuffer::new(4);
    assert!(rb.read_latest().is_none());
    assert!(rb.read_by_age(0).is_none());
    assert!(rb.is_empty());
    assert_eq!(rb.usage(), 0.0);
}

#[test]
fn the_latest_frame_is_the_last_written() {
    let mut rb = RingBuffer::new(4);
    for i in 1..=3 {
        rb.write(&frame(i, i as u8));
    }
    let latest = rb.read_latest().expect("a frame");
    assert_eq!(latest.sequence, 3);
    assert_eq!(latest.data[0], 3);
    assert_eq!(rb.len(), 3);
    assert_eq!(rb.total_written(), 3);
    assert_eq!(rb.overflow_count(), 0, "nothing was overwritten yet");
}

/// Age indexing, not slot indexing. The block-G1 spec indexed `slots[index]`
/// directly while documenting it as "0 = oldest available" — the same thing
/// only until the ring first wraps, after which a caller asking for the oldest
/// gets whatever happens to sit in slot zero.
#[test]
fn reading_by_age_survives_a_wrap() {
    let mut rb = RingBuffer::new(3);
    for i in 1..=5 {
        rb.write(&frame(i, i as u8));
    }
    // Capacity 3, five written: 3, 4 and 5 are held.
    assert_eq!(rb.read_by_age(0).expect("oldest").sequence, 3);
    assert_eq!(rb.read_by_age(1).expect("middle").sequence, 4);
    assert_eq!(rb.read_by_age(2).expect("newest").sequence, 5);
    assert_eq!(rb.read_latest().expect("latest").sequence, 5);
    assert!(rb.read_by_age(3).is_none(), "only three are held");
}

#[test]
fn overwriting_counts_as_overflow() {
    let mut rb = RingBuffer::new(2);
    rb.write(&frame(1, 1));
    rb.write(&frame(2, 2));
    assert_eq!(rb.overflow_count(), 0);
    rb.write(&frame(3, 3));
    assert_eq!(rb.overflow_count(), 1, "the third write displaced the first");
    assert_eq!(rb.usage(), 1.0);
}

// ---- the service ---------------------------------------------------------

struct Harness {
    frame_tx: mpsc::Sender<CaptureFrame>,
    handle: crate::service::StreamHandle,
    telemetry: broadcast::Receiver<TelemetryEnvelope>,
}

fn harness(mode: StreamMode, ring: usize, max_subs: usize) -> Harness {
    let (frame_tx, frame_rx) = mpsc::channel(16);
    let (tel_tx, telemetry) = broadcast::channel(256);
    let (svc, handle) = StreamService::new(frame_rx, tel_tx, mode, ring, max_subs);
    tokio::spawn(svc.run());
    Harness {
        frame_tx,
        handle,
        telemetry,
    }
}

fn drain(rx: &mut broadcast::Receiver<TelemetryEnvelope>) -> Vec<TelemetryEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e.event);
    }
    out
}

/// The gap this crate exists to fill: one capture source, several consumers,
/// each getting the frame.
#[tokio::test]
async fn every_subscriber_receives_the_same_frame() {
    let h = harness(StreamMode::Push, 4, 8);
    let mut a = h.handle.subscribe().await.expect("subscribe a");
    let mut b = h.handle.subscribe().await.expect("subscribe b");
    let mut c = h.handle.subscribe().await.expect("subscribe c");

    h.frame_tx.send(frame(1, 7)).await.expect("send");

    for (name, sub) in [("a", &mut a), ("b", &mut b), ("c", &mut c)] {
        let f = tokio::time::timeout(std::time::Duration::from_secs(2), sub.next_frame())
            .await
            .unwrap_or_else(|_| panic!("subscriber {name} timed out"))
            .unwrap_or_else(|| panic!("subscriber {name} got no frame"));
        assert_eq!(f.sequence, 1);
        assert_eq!(f.data[0], 7);
    }
}

/// The reason for per-subscriber channels: a consumer that never reads must
/// not cost the others frames. Its drops are attributed to it alone.
#[tokio::test]
async fn a_slow_subscriber_drops_only_its_own_frames() {
    let h = harness(StreamMode::Push, 4, 8);
    let _slow = h.handle.subscribe().await.expect("slow");   // never read
    let mut fast = h.handle.subscribe().await.expect("fast");

    // Well past the 4-deep per-subscriber queue.
    for i in 1..=12 {
        h.frame_tx.send(frame(i, i as u8)).await.expect("send");
        // Keep the fast consumer drained so only the slow one backs up.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        while fast.try_next_frame().is_some() {}
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let stats = h.handle.stats().await.expect("stats");
    let slow_stats = stats.subscribers.iter().find(|s| s.id.0 == 1).expect("slow");
    let fast_stats = stats.subscribers.iter().find(|s| s.id.0 == 2).expect("fast");
    assert!(slow_stats.dropped > 0, "the slow subscriber must record drops");
    assert_eq!(
        fast_stats.dropped, 0,
        "the fast subscriber must lose nothing to the slow one: {fast_stats:?}"
    );
    assert!(fast_stats.delivered >= 10, "got {}", fast_stats.delivered);
}

/// Dropping the subscription is how a consumer leaves. A consumer that panics
/// must not leak a subscription the service keeps feeding forever.
#[tokio::test]
async fn a_dropped_subscription_removes_itself() {
    let h = harness(StreamMode::Push, 4, 8);
    let a = h.handle.subscribe().await.expect("a");
    let _b = h.handle.subscribe().await.expect("b");
    assert_eq!(h.handle.stats().await.unwrap().subscriber_count, 2);

    drop(a);
    // The service notices on its next delivery attempt.
    h.frame_tx.send(frame(1, 1)).await.expect("send");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let stats = h.handle.stats().await.expect("stats");
    assert_eq!(stats.subscriber_count, 1, "the dropped subscriber must be gone");
}

#[tokio::test]
async fn the_subscriber_limit_is_enforced_and_explained() {
    let h = harness(StreamMode::Push, 4, 2);
    let _a = h.handle.subscribe().await.expect("a");
    let _b = h.handle.subscribe().await.expect("b");
    let err = h.handle.subscribe().await.unwrap_err();
    assert!(format!("{err}").contains("subscriber limit"), "{err}");
}

#[tokio::test]
async fn pull_mode_fills_the_ring_for_readers() {
    let h = harness(StreamMode::Pull, 4, 8);
    for i in 1..=3 {
        h.frame_tx.send(frame(i, i as u8)).await.expect("send");
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let rb = h.handle.ring_buffer.lock().expect("lock");
    assert_eq!(rb.read_latest().expect("latest").sequence, 3);
    assert_eq!(rb.len(), 3);
}

/// A mode that silently behaves like a different one is the undeclared stub
/// this crate was rewritten to remove — so it is refused, loudly.
#[tokio::test]
async fn an_unimplemented_mode_is_refused_not_silently_substituted() {
    let h = harness(StreamMode::Pull, 4, 8);
    for mode in [StreamMode::SharedMemory, StreamMode::Ipc] {
        let err = h.handle.set_mode(mode).await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not implemented"), "{msg}");
        assert!(msg.contains("refusing"), "{msg}");
    }
    // The mode must not have changed.
    assert_eq!(h.handle.stats().await.unwrap().mode, StreamMode::Pull);
}

#[tokio::test]
async fn switching_between_implemented_modes_is_allowed() {
    let h = harness(StreamMode::Pull, 4, 8);
    h.handle.set_mode(StreamMode::Push).await.expect("to push");
    assert_eq!(h.handle.stats().await.unwrap().mode, StreamMode::Push);
}

#[tokio::test]
async fn subscribe_and_remove_are_reported() {
    let mut h = harness(StreamMode::Push, 4, 8);
    let a = h.handle.subscribe().await.expect("a");
    let id = a.id;
    h.handle.unsubscribe(id).await.expect("unsubscribe");
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let events = drain(&mut h.telemetry);
    assert!(events.iter().any(|e| matches!(
        e, TelemetryEvent::SubscriberAdded { id: i, total: 1 } if *i == id.0
    )), "{events:?}");
    assert!(events.iter().any(|e| matches!(
        e, TelemetryEvent::SubscriberRemoved { id: i, total: 0 } if *i == id.0
    )), "{events:?}");
}

#[tokio::test]
async fn delivery_is_reported_with_the_frames_own_sequence() {
    let mut h = harness(StreamMode::Push, 4, 8);
    let mut a = h.handle.subscribe().await.expect("a");
    h.frame_tx.send(frame(42, 1)).await.expect("send");
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), a.next_frame()).await;

    let events = drain(&mut h.telemetry);
    assert!(
        events.iter().any(|e| matches!(
            e, TelemetryEvent::StreamDelivery { frame_sequence: 42, .. }
        )),
        "{events:?}"
    );
}

/// When the capture side goes away the service must stop, not idle forever
/// holding subscribers open.
#[tokio::test]
async fn the_service_stops_when_the_frame_source_closes() {
    let h = harness(StreamMode::Push, 4, 8);
    let mut a = h.handle.subscribe().await.expect("a");
    drop(h.frame_tx);

    let got = tokio::time::timeout(std::time::Duration::from_secs(2), a.next_frame())
        .await
        .expect("the subscription must close, not hang");
    assert!(got.is_none(), "a closed source must end the subscription");
}

/// The ring is the "what do you see right now" surface, so it must be filled
/// whatever the mode. Making it conditional on Pull meant a UI subscribed for
/// push and an agent asking for the current frame could not both be served —
/// which is the ordinary case, not an exotic one.
#[tokio::test]
async fn the_ring_is_filled_in_push_mode_too() {
    let h = harness(StreamMode::Push, 4, 8);
    let mut sub = h.handle.subscribe().await.expect("subscribe");
    h.frame_tx.send(frame(7, 3)).await.expect("send");

    let pushed = tokio::time::timeout(std::time::Duration::from_secs(2), sub.next_frame())
        .await
        .expect("push must still work")
        .expect("a frame");
    assert_eq!(pushed.sequence, 7, "the subscriber still receives");

    let rb = h.handle.ring_buffer.lock().expect("lock");
    let latest = rb.read_latest().expect("the ring must hold it too");
    assert_eq!(latest.sequence, 7, "a pull reader sees the same frame");
    assert_eq!(latest.data[0], 3);
}

/// A frame's bytes are meaningless without its format — the same buffer is a
/// JPEG, a plane pair or an RGB grid depending on it. The ring dropped it
/// originally, which made every stored frame ambiguous to anything but the UI,
/// and a reader that guesses will guess wrong on the camera that matters.
#[test]
fn the_ring_preserves_the_pixel_format() {
    use iris_hal::device::PixelFormat;
    let mut rb = RingBuffer::new(2);
    let mut f = frame(1, 5);
    f.format = PixelFormat::Mjpeg;
    rb.write(&f);
    assert_eq!(rb.read_latest().expect("a frame").format, PixelFormat::Mjpeg);
}
