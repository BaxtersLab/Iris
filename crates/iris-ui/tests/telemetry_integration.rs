use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn telemetry_assertions() {
    // Pin the MOCK backend explicitly.
    //
    // This comment used to say "(mock backends)" and rely on that being the
    // default. On 2026-09-01 the default became "use the camera if there is
    // one", because defaulting a camera application to a synthetic grey image
    // was the wrong behaviour for users — and this test silently started
    // running against real hardware, where frame sizes vary per frame and ROI
    // on MJPEG decodes to RGB24 rather than shrinking. Its subject is
    // telemetry, not backend selection, so it needs the deterministic backend
    // by name rather than by assumption.
    //
    // Safe here: an integration test is its own process, so this affects
    // nothing else.
    std::env::set_var("IRIS_BACKEND", "mock");

    let cfg = iris_core::config::IrisConfig::default();
    let rt = iris_ui::bootstrap::IrisRuntime::bootstrap(cfg)
        .await
        .expect("bootstrap runtime");

    let ipc = rt.ipc_handle;

    // Subscribe to global telemetry envelope stream
    let mut sub = ipc.subscribe_telemetry();
    println!(
        "telemetry_integration: subscribe_telemetry returned subscriber id={}",
        sub.id()
    );

    use iris_ipc::command::IpcCommand;

    // Start capture
    let _ = ipc.send_command(IpcCommand::StartCapture).await;

    // Expect to receive several FrameCaptured telemetry envelopes within a short time
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut last_seq: Option<u64> = None;
    let mut received = 0usize;
    let mut start_instant: Option<tokio::time::Instant> = None;
    let mut last_instant: Option<tokio::time::Instant> = None;
    let mut original_width: Option<u32> = None;
    let mut original_height: Option<u32> = None;
    let mut original_size_bytes: Option<usize> = None;

    while tokio::time::Instant::now() < deadline && received < 8 {
        let to = deadline - tokio::time::Instant::now();
        match tokio::time::timeout(to, sub.recv()).await {
            Ok(Ok(env)) => {
                use iris_ipc::telemetry::TelemetryEvent;
                if let TelemetryEvent::FrameCaptured {
                    sequence,
                    width,
                    height,
                    size_bytes,
                } = env.event
                {
                    // Basic assertions
                    assert!(width > 0, "width must be > 0");
                    assert!(height > 0, "height must be > 0");
                    // ensure size_bytes is populated for mock formats
                    assert!(size_bytes > 0, "size_bytes must be > 0 for mock formats");

                    if original_width.is_none() {
                        original_width = Some(width);
                        original_height = Some(height);
                        original_size_bytes = Some(size_bytes);
                    }

                    if let Some(prev) = last_seq {
                        assert!(sequence > prev, "sequence must increase");
                    }

                    last_seq = Some(sequence);
                    received += 1;
                    if start_instant.is_none() {
                        start_instant = Some(tokio::time::Instant::now());
                    }
                    last_instant = Some(tokio::time::Instant::now());
                }
            }
            _ => break,
        }
    }

    println!(
        "telemetry_integration: collected {} pre-ROI frames",
        received
    );

    // compute observed fps
    if let (Some(s), Some(l)) = (start_instant, last_instant) {
        let elapsed = (l - s).as_secs_f64();
        if elapsed > 0.0 && received > 1 {
            let fps = (received as f64 - 1.0) / elapsed;
            assert!(fps > 1.0, "observed fps too low: {}", fps);
        }
    }

    // size_bytes may be zero in mock backends; ensure non-negative at least
    assert!(
        received >= 3,
        "expected at least 3 telemetry frames, got {}",
        received
    );

    // Now test ROI behavior: set an ROI and ensure subsequent frames have dimensions
    // that are <= the original frame dimensions and observe at least one post-ROI frame.
    let ow = original_width.expect("original width");
    let oh = original_height.expect("original height");
    let roi_w = (ow / 2).max(1);
    let roi_h = (oh / 2).max(1);

    let _ = ipc
        .send_command(IpcCommand::SetRoi {
            x: 0,
            y: 0,
            width: roi_w,
            height: roi_h,
        })
        .await;

    // Diagnostic probe: create a fresh subscription after ROI to verify whether
    // newly-subscribed receivers observe post-ROI frames (helps narrow root cause).
    let mut probe_sub = ipc.subscribe_telemetry();
    println!(
        "telemetry_integration: probe subscriber id={}",
        probe_sub.id()
    );
    match tokio::time::timeout(Duration::from_secs(2), probe_sub.recv()).await {
        Ok(Ok(env)) => println!(
            "telemetry_integration: probe recv ok post-ROI env={:?}",
            env
        ),
        Ok(Err(e)) => println!("telemetry_integration: probe recv error after ROI: {:?}", e),
        Err(_) => println!("telemetry_integration: probe timeout waiting for a post-ROI frame"),
    }

    // collect a few frames after ROI
    let post_deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    let mut post_received = 0usize;
    let mut post_smaller = false;

    while tokio::time::Instant::now() < post_deadline && post_received < 5 {
        let to = post_deadline - tokio::time::Instant::now();
        match tokio::time::timeout(to, sub.recv()).await {
            Ok(Ok(env)) => {
                use iris_ipc::telemetry::TelemetryEvent;
                if let TelemetryEvent::FrameCaptured {
                    width,
                    height,
                    size_bytes,
                    ..
                } = env.event
                {
                    assert!(width <= ow, "post-ROI width should not exceed original");
                    assert!(height <= oh, "post-ROI height should not exceed original");
                    if let Some(orig) = original_size_bytes {
                        if size_bytes < orig {
                            post_smaller = true;
                        }
                    }
                    post_received += 1;
                }
            }
            Ok(Err(e)) => {
                println!(
                    "telemetry_integration: recv returned error after ROI: {:?}",
                    e
                );
                break;
            }
            Err(_) => {
                println!("telemetry_integration: timeout waiting for post-ROI frame");
                break;
            }
        }
    }

    // Stop capture
    let _ = ipc.send_command(IpcCommand::StopCapture).await;

    assert!(
        post_received >= 1,
        "expected at least 1 telemetry frame after ROI, got {}",
        post_received
    );
    assert!(
        post_smaller,
        "expected at least one post-ROI frame with smaller size_bytes"
    );
    // It's acceptable if mock backend doesn't change resolution to ROI exactly.
}
