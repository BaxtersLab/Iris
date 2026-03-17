use std::time::Duration;

#[tokio::test]
async fn rebase_forwarding_to_telemetry() {
    use iris_core::pipeline::EncoderRebaseEvent;
    use iris_ipc::telemetry::TelemetryEvent;
    use tokio::sync::broadcast;

    // Create IPC server and handle (we don't need to run the full server loop)
    let (_server, handle, telemetry_tx) = iris_ipc::server::IpcServer::new(8);

    // Create a local rebase channel and forwarder similar to bootstrap
    let (rebase_tx, mut rebase_rx) = broadcast::channel::<EncoderRebaseEvent>(8);
    let telemetry_tx_clone = telemetry_tx.clone();

    let _fwd = tokio::spawn(async move {
        while let Ok(ev) = rebase_rx.recv().await {
            let event = TelemetryEvent::EncoderRebase {
                prev_raw: ev.prev_raw,
                prev_capture: ev.prev_capture,
                new_raw: ev.new_raw,
                new_capture: ev.new_capture,
                reason: ev.reason.clone(),
            };
            let envelope = iris_ipc::telemetry::TelemetryEnvelope {
                timestamp: chrono::Utc::now(),
                sequence: 0,
                event,
            };
            let _ = telemetry_tx_clone.send(envelope);
        }
    });

    // Subscribe to telemetry via the IPC handle
    let mut sub = handle.subscribe_telemetry();

    // Send a test rebase event
    let ev = EncoderRebaseEvent {
        prev_raw: 123,
        prev_capture: 1000,
        new_raw: 456,
        new_capture: 2000,
        reason: "test".to_string(),
    };
    let _ = rebase_tx.send(ev);

    // Expect to receive a TelemetryEnvelope::EncoderRebase
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let to = deadline - tokio::time::Instant::now();
    match tokio::time::timeout(to, sub.recv()).await {
        Ok(Ok(env)) => {
            match env.event {
                TelemetryEvent::EncoderRebase { prev_raw, prev_capture, new_raw, new_capture, reason } => {
                    assert_eq!(prev_raw, 123);
                    assert_eq!(prev_capture, 1000);
                    assert_eq!(new_raw, 456);
                    assert_eq!(new_capture, 2000);
                    assert_eq!(reason, "test");
                }
                _ => panic!("expected EncoderRebase event"),
            }
        }
        Ok(Err(e)) => panic!("telemetry recv error: {:?}", e),
        Err(_) => panic!("timeout waiting for telemetry envelope"),
    }
}
