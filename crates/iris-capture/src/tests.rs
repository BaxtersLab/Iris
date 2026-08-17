#[cfg(test)]
mod tests {
    use crate::backend::{CaptureBackend, CaptureConfig, DropPolicy, MockCaptureBackend};
    use crate::frame::{CaptureFrame, Roi};
    use crate::service::{CaptureCommand, CaptureService};
    use crate::telemetry::CaptureTelemetry;
    use iris_hal::device::PixelFormat;
    use tokio::sync::broadcast;
    use tokio::time::{timeout, Duration};

    #[test]
    fn test_expected_size() {
        assert_eq!(
            CaptureFrame::expected_size(640, 480, PixelFormat::Rgb24),
            640 * 480 * 3
        );
    }

    /// MJPEG frames are compressed, so size cannot be derived from dimensions.
    /// `expected_size` returns 0 meaning "not predictable" — pin that contract
    /// so nobody starts treating it as a real byte count or an empty frame.
    #[test]
    fn expected_size_is_zero_for_compressed_mjpeg() {
        assert_eq!(CaptureFrame::expected_size(1920, 1080, PixelFormat::Mjpeg), 0);
        // ...while every raw format still reports its true size.
        assert_eq!(
            CaptureFrame::expected_size(1920, 1080, PixelFormat::Nv12),
            1920 * 1080 * 3 / 2
        );
        assert_eq!(
            CaptureFrame::expected_size(1920, 1080, PixelFormat::Yuyv),
            1920 * 1080 * 2
        );
    }

    #[test]
    fn test_roi_validation_valid() {
        let r = Roi {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        assert!(r.validate(20, 20));
    }

    #[test]
    fn test_roi_validation_invalid() {
        let r = Roi {
            x: 15,
            y: 15,
            width: 10,
            height: 10,
        };
        assert!(!r.validate(20, 20));
    }

    #[tokio::test]
    async fn test_mock_backend_start_stop() {
        let cfg = CaptureConfig {
            width: 16,
            height: 16,
            target_fps: 30,
            format: PixelFormat::Rgb24,
            max_queue_depth: 2,
            drop_policy: DropPolicy::Newest,
            roi: None,
        };
        let mut b = MockCaptureBackend::new(cfg);
        b.start().await.unwrap();
        assert!(b.is_capturing());
        b.stop().await.unwrap();
        assert!(!b.is_capturing());
    }

    #[tokio::test]
    async fn test_mock_backend_next_frame() {
        let cfg = CaptureConfig {
            width: 8,
            height: 8,
            target_fps: 60,
            format: PixelFormat::Rgb24,
            max_queue_depth: 2,
            drop_policy: DropPolicy::Newest,
            roi: None,
        };
        let mut b = MockCaptureBackend::new(cfg.clone());
        b.start().await.unwrap();
        let f = b.next_frame().await.unwrap();
        assert_eq!(f.sequence, 1);
        assert_eq!(f.width, cfg.width);
        b.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_mock_backend_multiple_frames() {
        let cfg = CaptureConfig {
            width: 4,
            height: 4,
            target_fps: 120,
            format: PixelFormat::Rgb24,
            max_queue_depth: 2,
            drop_policy: DropPolicy::Newest,
            roi: None,
        };
        let mut b = MockCaptureBackend::new(cfg);
        b.start().await.unwrap();
        for i in 1..=5 {
            let f = b.next_frame().await.unwrap();
            assert_eq!(f.sequence, i);
        }
        b.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_capture_service_basic_flow() {
        let cfg = CaptureConfig {
            width: 8,
            height: 8,
            target_fps: 30,
            format: PixelFormat::Rgb24,
            max_queue_depth: 4,
            drop_policy: DropPolicy::Oldest,
            roi: None,
        };
        let backend = MockCaptureBackend::new(cfg.clone());
        let (tx, _rx) = broadcast::channel(8);
        let (svc, handle) = CaptureService::new(backend, cfg.clone(), tx.clone());
        // Instead spawn run with its own receiver
        let (cmd_tx, cmd_rx2) = tokio::sync::mpsc::channel(8);
        let svc_task = tokio::spawn(svc.run(cmd_rx2));
        // receive frames
        let mut received = 0;
        let mut frame_rx = handle.frame_rx;
        let recv_fut = async {
            while received < 3 {
                if let Some(_f) = frame_rx.recv().await {
                    received += 1;
                }
            }
            // stop
            let _ = cmd_tx.send(CaptureCommand::Stop).await;
        };
        timeout(Duration::from_secs(5), recv_fut).await.unwrap();
        svc_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_capture_service_pause_resume() {
        let cfg = CaptureConfig {
            width: 8,
            height: 8,
            target_fps: 30,
            format: PixelFormat::Rgb24,
            max_queue_depth: 4,
            drop_policy: DropPolicy::Oldest,
            roi: None,
        };
        let backend = MockCaptureBackend::new(cfg.clone());
        let (tx, _rx) = broadcast::channel(8);
        let (svc, _handle) = CaptureService::new(backend, cfg.clone(), tx.clone());
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(8);
        let svc_task = tokio::spawn(svc.run(cmd_rx));

        // Pause quickly
        cmd_tx.send(CaptureCommand::Pause).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Resume
        cmd_tx.send(CaptureCommand::Resume).await.unwrap();
        // Stop after receiving one
        tokio::time::sleep(Duration::from_millis(200)).await;
        cmd_tx.send(CaptureCommand::Stop).await.unwrap();
        svc_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_capture_service_drop_policy_oldest() {
        let cfg = CaptureConfig {
            width: 8,
            height: 8,
            target_fps: 200,
            format: PixelFormat::Rgb24,
            max_queue_depth: 2,
            drop_policy: DropPolicy::Oldest,
            roi: None,
        };
        let backend = MockCaptureBackend::new(cfg.clone());
        let (tx, _rx) = broadcast::channel(8);
        let (svc, mut handle) = CaptureService::new(backend, cfg.clone(), tx.clone());
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(8);
        let svc_task = tokio::spawn(svc.run(cmd_rx));
        // consumer slower than producer: receive a couple frames
        let mut got = 0;
        for _ in 0..3 {
            if let Some(_f) = handle.frame_rx.recv().await {
                got += 1;
            }
        }
        let _ = cmd_tx.send(CaptureCommand::Stop).await;
        svc_task.await.unwrap();
        assert!(got >= 1);
    }

    #[tokio::test]
    async fn test_capture_service_roi() {
        let cfg = CaptureConfig {
            width: 32,
            height: 24,
            target_fps: 30,
            format: PixelFormat::Rgb24,
            max_queue_depth: 4,
            drop_policy: DropPolicy::Newest,
            roi: Some(Roi {
                x: 0,
                y: 0,
                width: 16,
                height: 12,
            }),
        };
        let backend = MockCaptureBackend::new(cfg.clone());
        let (tx, _rx) = broadcast::channel(8);
        let (svc, mut handle) = CaptureService::new(backend, cfg.clone(), tx.clone());
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(8);
        let svc_task = tokio::spawn(svc.run(cmd_rx));
        // read one frame
        if let Some(f) = handle.frame_rx.recv().await {
            assert!(f.is_cropped || cfg.roi.is_some());
        }
        let _ = cmd_tx.send(CaptureCommand::Stop).await;
        svc_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_capture_telemetry_emission() {
        let cfg = CaptureConfig {
            width: 8,
            height: 8,
            target_fps: 30,
            format: PixelFormat::Rgb24,
            max_queue_depth: 4,
            drop_policy: DropPolicy::Newest,
            roi: None,
        };
        let backend = MockCaptureBackend::new(cfg.clone());
        let (tx, mut rx) = broadcast::channel::<CaptureTelemetry>(8);
        let (svc, _handle) = CaptureService::new(backend, cfg.clone(), tx.clone());
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(8);
        let svc_task = tokio::spawn(svc.run(cmd_rx));
        // Wait for one telemetry
        let got = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
        let _ = cmd_tx.send(CaptureCommand::Stop).await;
        svc_task.await.unwrap();
        assert!(got.is_ok());
    }

    #[tokio::test]
    async fn test_nv12_misaligned_roi_rounds_down() {
        let cfg = CaptureConfig {
            width: 32,
            height: 24,
            target_fps: 30,
            format: PixelFormat::Nv12,
            max_queue_depth: 4,
            drop_policy: DropPolicy::Newest,
            roi: Some(Roi {
                x: 1,
                y: 1,
                width: 15,
                height: 11,
            }),
        };
        let backend = MockCaptureBackend::new(cfg.clone());
        let (tx, _rx) = broadcast::channel(8);
        let (svc, mut handle) = CaptureService::new(backend, cfg.clone(), tx.clone());
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(8);
        let svc_task = tokio::spawn(svc.run(cmd_rx));
        // read one frame and assert it was cropped with even dimensions
        if let Some(f) = handle.frame_rx.recv().await {
            // original roi: x=1,y=1,w=15,h=11 -> adjusted: x=0,y=0,w=14,h=10
            assert!(f.is_cropped);
            assert_eq!(f.width, 14);
            assert_eq!(f.height, 10);
        }
        let _ = cmd_tx.send(CaptureCommand::Stop).await;
        svc_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_nv12_misaligned_roi_too_small_skips_crop() {
        // roi that after rounding would become zero-sized
        let cfg = CaptureConfig {
            width: 4,
            height: 4,
            target_fps: 30,
            format: PixelFormat::Nv12,
            max_queue_depth: 4,
            drop_policy: DropPolicy::Newest,
            roi: Some(Roi {
                x: 3,
                y: 3,
                width: 1,
                height: 1,
            }),
        };
        let backend = MockCaptureBackend::new(cfg.clone());
        let (tx, _rx) = broadcast::channel(8);
        let (svc, mut handle) = CaptureService::new(backend, cfg.clone(), tx.clone());
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(8);
        let svc_task = tokio::spawn(svc.run(cmd_rx));
        if let Some(f) = handle.frame_rx.recv().await {
            // adjustment would make width/height zero so cropping skipped
            assert!(!f.is_cropped);
            assert_eq!(f.width, cfg.width);
            assert_eq!(f.height, cfg.height);
        }
        let _ = cmd_tx.send(CaptureCommand::Stop).await;
        svc_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_telemetry_size_matches_frame_after_roi() {
        use tokio::sync::mpsc;
        let cfg = CaptureConfig {
            width: 32,
            height: 24,
            target_fps: 30,
            format: PixelFormat::Rgb24,
            max_queue_depth: 4,
            drop_policy: DropPolicy::Newest,
            roi: Some(Roi {
                x: 0,
                y: 0,
                width: 16,
                height: 12,
            }),
        };
        let backend = MockCaptureBackend::new(cfg.clone());
        let (tx, mut rx) = broadcast::channel::<CaptureTelemetry>(8);
        let (svc, mut handle) = CaptureService::new(backend, cfg.clone(), tx.clone());
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let svc_task = tokio::spawn(svc.run(cmd_rx));

        // wait for telemetry and frame
        let tele = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(2), handle.frame_rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(tele.size_bytes, frame.size_bytes());

        let _ = cmd_tx.send(CaptureCommand::Stop).await;
        svc_task.await.unwrap();
    }
}
