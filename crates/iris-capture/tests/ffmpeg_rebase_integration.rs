use iris_capture::backend::{CaptureConfig, DropPolicy, MockCaptureBackend};
use iris_capture::service::{CaptureCommand, CaptureService};
use iris_core::pipeline::RecordingPipeline;
use tokio::time::{timeout, Duration};

#[tokio::test]
async fn ffmpeg_integration_rebase_end_to_end() {
    // Gate ffmpeg integration tests to avoid CI flakiness unless explicitly enabled.
    // Enable by setting `CI_FFMPEG=1` or `RUN_FFMPEG_INTEGRATION=1` in the environment.
    let run_flag = std::env::var("CI_FFMPEG").is_ok() || std::env::var("RUN_FFMPEG_INTEGRATION").is_ok();
    if !run_flag {
        eprintln!("Skipping ffmpeg rebase integration test: set CI_FFMPEG=1 or RUN_FFMPEG_INTEGRATION=1 to enable");
        return;
    }

    // Require ffmpeg on PATH; skip test early if missing.
    if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("Skipping ffmpeg rebase integration test: ffmpeg not found on PATH");
        return;
    }

    // Use a conservative drift threshold but allow more overall wait time to
    // reduce the impact of timing variance in CI.
    let drift_threshold_us = 5_000_000i64; // 5 seconds

    // Start recording pipeline with telemetry channel to collect rebase events
    let (rebase_tx, mut rebase_rx) = tokio::sync::broadcast::channel::<iris_core::pipeline::EncoderRebaseEvent>(8);
    let (pipeline, mut pkt_rx) = RecordingPipeline::start_with_telemetry(8, drift_threshold_us, Some(rebase_tx.clone()));
    let encoder_tx = pipeline.encoder_sender();

    // Configure a small mock capture to drive the encoder
    let cap_cfg = CaptureConfig {
        width: 64,
        height: 48,
        target_fps: 10,
        format: iris_hal::device::PixelFormat::Bgr24,
        max_queue_depth: 8,
        drop_policy: DropPolicy::Oldest,
        roi: None,
    };

    let (tx, _rx) = tokio::sync::broadcast::channel(16);
    let backend = MockCaptureBackend::new(cap_cfg.clone());

    let (svc, _handle) = CaptureService::new(backend, cap_cfg, tx.clone(), encoder_tx.clone());

    // spawn the service
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(8);
    let svc_task = tokio::spawn(async move { svc.run(cmd_rx).await });

    // start capturing
    let _ = cmd_tx.send(CaptureCommand::Resume).await;

    // Wait up to 45 seconds for a rebase event to appear (longer to accommodate CI)
    let wait = Duration::from_secs(45);
    match timeout(wait, rebase_rx.recv()).await {
        Ok(Ok(ev)) => {
            // basic sanity checks on fields
            assert!(ev.new_capture >= 0);
            assert!(ev.new_raw >= 0);
            // we observed a rebase — success
        }
        Ok(Err(e)) => panic!("rebase receiver error: {:?}", e),
        Err(_) => panic!("timeout waiting for encoder rebase event (ffmpeg integration)"),
    }

    // stop capture
    let _ = cmd_tx.send(CaptureCommand::Stop).await;
    let _ = svc_task.await;

    // drain a few encoded packets to ensure pipeline was active
    let mut got_any = false;
    let start = tokio::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        match timeout(Duration::from_secs(2), pkt_rx.recv()).await {
            Ok(Some(pkt)) => {
                got_any = true;
                if pkt.keyframe {
                    break;
                }
            }
            _ => break,
        }
    }

    assert!(got_any, "expected at least one encoded packet from ffmpeg");
}
