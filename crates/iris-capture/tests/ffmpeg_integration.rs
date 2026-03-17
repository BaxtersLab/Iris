use iris_capture::backend::{CaptureConfig, DropPolicy, MockCaptureBackend};
use iris_capture::service::{CaptureCommand, CaptureService};
use iris_core::pipeline::RecordingPipeline;
use tokio::time::{timeout, Duration};

#[tokio::test]
async fn ffmpeg_integration_end_to_end_pts_keyframe() {
    // Gate ffmpeg integration tests to avoid CI flakiness unless explicitly enabled.
    // Enable by setting `CI_FFMPEG=1` or `RUN_FFMPEG_INTEGRATION=1` in the environment.
    let run_flag = std::env::var("CI_FFMPEG").is_ok() || std::env::var("RUN_FFMPEG_INTEGRATION").is_ok();
    if !run_flag {
        eprintln!("Skipping ffmpeg integration test: set CI_FFMPEG=1 or RUN_FFMPEG_INTEGRATION=1 to enable");
        return;
    }

    // Require ffmpeg on PATH; skip test early if missing.
    if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("Skipping ffmpeg integration test: ffmpeg not found on PATH");
        return;
    }

    // Start recording pipeline and obtain packet receiver
    let (pipeline, mut pkt_rx) = RecordingPipeline::start(8);
    let encoder_tx = pipeline.encoder_sender();

    // Create a mock capture backend and capture service wired to the encoder
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

    // Collect encoded packets for up to 20 seconds or until we see a keyframe
    // (longer timeout reduces CI timing flakiness)
    let mut got_any = false;
    let mut saw_keyframe = false;
    let deadline = Duration::from_secs(20);

    let start = tokio::time::Instant::now();
    while start.elapsed() < deadline {
        match timeout(Duration::from_secs(6), pkt_rx.recv()).await {
            Ok(Some(pkt)) => {
                got_any = true;
                // pts should be non-zero and reasonable
                assert!(pkt.pts > 0, "packet pts should be > 0");
                // update keyframe detection
                if pkt.keyframe {
                    saw_keyframe = true;
                    break;
                }
            }
            Ok(None) => break, // channel closed
            Err(_) => break,   // timeout
        }
    }

    // stop capture
    let _ = cmd_tx.send(CaptureCommand::Stop).await;
    let _ = svc_task.await;

    assert!(
        got_any,
        "expected to receive at least one encoded packet from ffmpeg"
    );
    assert!(
        saw_keyframe,
        "expected to observe at least one keyframe (IDR) in encoded packets"
    );
}
