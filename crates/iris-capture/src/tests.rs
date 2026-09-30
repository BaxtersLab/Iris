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

#[cfg(test)]
mod mjpeg_roi_tests {
    use crate::backend::MockCaptureBackend;
    use crate::frame::{CaptureFrame, Roi};
    use crate::service::CaptureService;
    use iris_hal::device::PixelFormat;

    const TINY_JPEG: &[u8] = include_bytes!("../tests/fixtures/tiny16.jpg");

    fn mjpeg_frame(data: Vec<u8>) -> CaptureFrame {
        CaptureFrame {
            sequence: 1,
            width: 16,
            height: 16,
            format: PixelFormat::Mjpeg,
            data,
            timestamp_us: 0,
            is_cropped: false,
        }
    }

    /// ROI on MJPEG used to be a documented no-op (`is_cropped = false`).
    /// It must now decode and genuinely crop, which necessarily converts the
    /// frame to RGB24 — a compressed frame has no pixel grid to slice.
    #[test]
    fn roi_on_mjpeg_decodes_and_crops() {
        let mut frame = mjpeg_frame(TINY_JPEG.to_vec());
        CaptureService::<MockCaptureBackend>::apply_roi(
            &mut frame,
            Roi {
                x: 2,
                y: 2,
                width: 8,
                height: 8,
            },
        );

        assert_eq!(
            frame.format,
            PixelFormat::Rgb24,
            "cropping MJPEG must decode it; the frame is no longer compressed"
        );
        assert_eq!((frame.width, frame.height), (8, 8), "ROI geometry applied");
        assert!(frame.is_cropped, "frame must report itself cropped");
        assert_eq!(
            frame.data.len(),
            8 * 8 * 3,
            "cropped RGB24 must be tightly packed"
        );
    }

    /// A truncated or corrupt MJPEG frame must leave the frame completely
    /// untouched and uncropped, never half-decoded and never byte-sliced as if
    /// it were raw pixels.
    #[test]
    fn roi_on_corrupt_mjpeg_leaves_frame_untouched() {
        // SOI with no EOI: truncated.
        let corrupt = vec![0xFF, 0xD8, 0x11, 0x22, 0x33];
        let mut frame = mjpeg_frame(corrupt.clone());
        CaptureService::<MockCaptureBackend>::apply_roi(
            &mut frame,
            Roi {
                x: 2,
                y: 2,
                width: 8,
                height: 8,
            },
        );

        assert_eq!(frame.format, PixelFormat::Mjpeg, "format must not change");
        assert_eq!(frame.data, corrupt, "data must not be modified");
        assert_eq!((frame.width, frame.height), (16, 16), "geometry unchanged");
        assert!(!frame.is_cropped, "must report itself uncropped");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod mjpeg_hardware_tests {
    /// One camera, one test at a time — **across processes**.
    ///
    /// This was a `static Mutex`, which serialises only the tests inside this
    /// binary. `cargo test` runs each crate's tests in its own process, so
    /// `IRIS_USE_HW=1 cargo test --workspace` still let these run alongside
    /// `iris-hal`'s hardware tests and fail with `Device or resource busy`.
    /// `flock` reaches across processes and also serialises within one.
    ///
    /// See `iris_hal::device_lock` for why `flock` and not a pid file.
    fn camera_guard() -> iris_hal::device_lock::DeviceLock {
        iris_hal::device_lock::lock_capture_device()
    }

    /// Hardware: change the camera's **brightness control** — a uniform shift
    /// with nothing moving — and confirm the discriminator calls it light
    /// rather than motion.
    ///
    /// This is the case that motivated the correction, reproduced deliberately
    /// instead of waited for. Auto-exposure produced it by accident on
    /// 2026-09-04: opening the device took brightness 79.8 → 114.3 over ~2s
    /// while raw motion tracked it to 10, with an empty, still room. Anything
    /// thresholding on raw would have shouted at nobody.
    ///
    /// Driving the control gives the same shape on demand: the whole frame
    /// changes, so `raw` and `shift` rise together while `local` stays at the
    /// sensor-noise floor.
    #[tokio::test]
    async fn changing_camera_brightness_reads_as_light_not_motion() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping changing_camera_brightness_reads_as_light_not_motion (set IRIS_USE_HW=1)");
            return;
        }
        if !iris_hal::v4l2_backend::v4l2::V4l2UvcBackend::video_nodes_present() {
            eprintln!("SKIP: IRIS_USE_HW=1 but no /dev/video* node exists — no camera attached");
            return;
        }
        let _camera = camera_guard();
        use iris_hal::backend::UvcBackend as _;
        use iris_hal::v4l2_backend::v4l2::V4l2UvcBackend;
        use crate::snapshot::{luma_plane, motion_detail, to_rgb24_for_tests};

        const V4L2_CID_BRIGHTNESS: u32 = 0x0098_0900;

        let backend = V4l2UvcBackend::new();
        let devs = backend.enumerate_devices().await.expect("enumerate failed");
        assert!(!devs.is_empty(), "no capture device present");
        let id = devs[0].id.clone();

        // Is brightness even adjustable here? A camera without it cannot
        // demonstrate this, and saying so is better than a fabricated pass.
        let controls = backend.list_controls(&id).await.unwrap_or_default();
        let brightness = controls.iter().find(|c| c.id == V4L2_CID_BRIGHTNESS);
        let Some(bc) = brightness else {
            eprintln!("SKIP: this camera exposes no brightness control — cannot force a uniform shift");
            return;
        };
        let original = backend.get_control(&id, V4L2_CID_BRIGHTNESS).await.unwrap_or(bc.default);
        eprintln!("brightness control: {}..{} default {} now {}", bc.min, bc.max, bc.default, original);

        backend.open_device(&id, None).await.expect("open failed");
        // Let auto-exposure settle first, or its own ramp contaminates the test.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        async fn grab(
            b: &V4l2UvcBackend,
            id: &iris_hal::device::DeviceId,
        ) -> (Vec<u8>, u32, u32) {
            let raw = b.read_frame(id).await.expect("read_frame");
            let f = b.current_format(id).await.unwrap().unwrap();
            let frame = crate::frame::CaptureFrame {
                sequence: 0,
                width: f.width,
                height: f.height,
                format: f.pixel_format.clone(),
                data: raw,
                timestamp_us: crate::frame::CaptureFrame::now_us(),
                is_cropped: false,
            };
            let (w, h, rgb) = to_rgb24_for_tests(&frame).expect("decode");
            (luma_plane(&rgb, w, h), w, h)
        }

        let (before, w, h) = grab(&backend, &id).await;

        // Drive brightness hard toward one end — a uniform change to every pixel.
        let target = if (original - bc.min).abs() > (bc.max - original).abs() { bc.min } else { bc.max };
        let set_ok = backend.set_control(&id, V4L2_CID_BRIGHTNESS, target).await.is_ok();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let readback = backend
            .get_control(&id, V4L2_CID_BRIGHTNESS)
            .await
            .unwrap_or(original);
        eprintln!("  set {target} (ok={set_ok}), device now reports {readback}");

        let (after, w2, h2) = grab(&backend, &id).await;

        // Restore before asserting, so a failure does not leave the camera
        // sitting at an extreme for the next test or the operator.
        backend.set_control(&id, V4L2_CID_BRIGHTNESS, original).await.ok();
        backend.close_device(&id).await.ok();

        assert_eq!((w, h), (w2, h2), "geometry must not change mid-test");
        let m = motion_detail(&after, &before).expect("same size");
        eprintln!("  raw {:.3}   local {:.3}   shift {:.3}", m.raw, m.local, m.shift);

        if m.raw < 2.0 {
            // Distinguish the two reasons, because they are different facts and
            // only one of them is about this camera's control surface.
            if readback != target {
                eprintln!(
                    "SKIP: the driver refused the brightness change (asked {target}, \
                     device reports {readback}) — cannot force a uniform shift here"
                );
            } else {
                eprintln!(
                    "SKIP: brightness was set to {target} and accepted, but the picture \
                     moved only {:.3} — the camera's AUTO-EXPOSURE compensated for it. \
                     Forcing a uniform shift on this device needs auto-exposure disabled \
                     first (V4L2_CID_EXPOSURE_AUTO, camera class 0x009a09xx, which \
                     list_controls does not scan).",
                    m.raw
                );
            }
            return;
        }
        assert!(
            m.shift.abs() > m.local,
            "a uniform brightness change must read mainly as light, not movement: \
             raw {:.3} local {:.3} shift {:.3}",
            m.raw, m.local, m.shift
        );
        assert!(
            m.local < m.raw * 0.6,
            "the correction must remove most of a uniform shift: raw {:.3} local {:.3}",
            m.raw, m.local
        );
    }

    /// End-to-end: pull a real frame off the camera through the V4L2 backend
    /// and decode it. The fixture tests prove the decoder works on a 16x16
    /// image; this proves it works on what the hardware actually emits —
    /// 1080p MJPEG with the driver's padding byte still attached.
    #[tokio::test]
    async fn real_camera_mjpeg_frame_decodes() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping real_camera_mjpeg_frame_decodes (set IRIS_USE_HW=1)");
            return;
        }
        // IRIS_USE_HW=1 says "exercise the hardware", not "a camera is
        // guaranteed". Tell the two apart: with no /dev/video* node at all
        // nothing is plugged in, which is an environment fact and a skip. With
        // a node present but nothing enumerated, that IS the regression this
        // test exists to catch, and the assert below fires.
        if !iris_hal::v4l2_backend::v4l2::V4l2UvcBackend::video_nodes_present() {
            eprintln!(
                "SKIP {}: IRIS_USE_HW=1 but no /dev/video* node exists — no camera attached",
                "real_camera_mjpeg_frame_decodes"
            );
            return;
        }
        let _camera = camera_guard();
        use iris_hal::backend::UvcBackend as _;
        use iris_hal::device::PixelFormat;
        use iris_hal::v4l2_backend::v4l2::V4l2UvcBackend;

        let backend = V4l2UvcBackend::new();
        let devs = backend.enumerate_devices().await.expect("enumerate failed");
        assert!(!devs.is_empty(), "no capture device present");
        let id = devs[0].id.clone();

        backend.open_device(&id, None).await.expect("open failed");
        let fmt = backend
            .current_format(&id)
            .await
            .expect("current_format failed")
            .expect("open device must report a format");

        if fmt.pixel_format != PixelFormat::Mjpeg {
            eprintln!("camera is delivering {:?}, not MJPEG — skipping", fmt.pixel_format);
            backend.close_device(&id).await.ok();
            return;
        }

        let raw = backend.read_frame(&id).await.expect("read_frame failed");
        backend.close_device(&id).await.expect("close failed");

        let decoded = crate::mjpeg::decode_to_rgb24(&raw)
            .unwrap_or_else(|e| panic!("real camera frame failed to decode: {e}"));

        eprintln!(
            "decoded real frame: {}x{} from {} compressed bytes -> {} RGB bytes",
            decoded.width,
            decoded.height,
            raw.len(),
            decoded.rgb24.len()
        );

        assert_eq!(
            (decoded.width, decoded.height),
            (fmt.width, fmt.height),
            "decoded geometry must match what the driver reported"
        );
        assert_eq!(decoded.rgb24.len(), (fmt.width * fmt.height * 3) as usize);

        // A real scene is never a single flat colour; this catches a decode
        // that "succeeds" into an empty buffer.
        let first = decoded.rgb24[0];
        assert!(
            decoded.rgb24.iter().any(|&b| b != first),
            "decoded frame is a uniform buffer — not a real image"
        );
    }
}

#[cfg(test)]
mod no_camera_tests {
    // --- No camera --------------------------------------------------------------
    // With no camera, Iris used to fall back to MockCaptureBackend: flat grey
    // frames, shown in the window and handed to agents as if they were pictures.
    // NoCameraBackend produces nothing and says why (operator ruling 2026-09-29).

    #[tokio::test]
    async fn no_camera_backend_never_produces_a_frame() {
        use crate::backend::{CaptureBackend, NoCameraBackend, NO_CAMERA};
        let mut b = NoCameraBackend;
        let start = b.start().await.expect_err("a backend with no camera must not start");
        assert!(start.to_string().contains(NO_CAMERA), "start error: {start}");
        let frame = b.next_frame().await.expect_err("a backend with no camera must not yield a frame");
        assert!(frame.to_string().contains(NO_CAMERA), "frame error: {frame}");
        assert!(!b.is_capturing());
    }

    #[tokio::test]
    async fn a_service_with_no_camera_says_so_and_sends_nothing() {
        use crate::backend::{CaptureConfig, DropPolicy, NoCameraBackend, NO_CAMERA};
        use crate::service::{CaptureCommand, CaptureService, CaptureServiceState};
        use iris_hal::device::PixelFormat;
        use tokio::sync::broadcast;
        use tokio::time::{timeout, Duration};
        let cfg = CaptureConfig {
            width: 8,
            height: 8,
            target_fps: 30,
            format: PixelFormat::Rgb24,
            max_queue_depth: 4,
            drop_policy: DropPolicy::Oldest,
            roi: None,
        };
        let (tx, _rx) = broadcast::channel(8);
        let (svc, handle) = CaptureService::new(NoCameraBackend, cfg, tx);
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(8);
        let svc_task = tokio::spawn(svc.run(cmd_rx));
        // Asking it to capture changes nothing: there is nothing to capture.
        let _ = cmd_tx.send(CaptureCommand::Resume).await;
        timeout(Duration::from_secs(5), svc_task)
            .await
            .expect("the service must stop, not wait for frames that cannot come")
            .unwrap();
        match handle.state() {
            CaptureServiceState::Error(e) => assert!(e.contains(NO_CAMERA), "state: {e}"),
            other => panic!("state should name the missing camera, was {other:?}"),
        }
        let mut frame_rx = handle.frame_rx;
        assert!(
            timeout(Duration::from_millis(300), frame_rx.recv()).await.map(|f| f.is_none()).unwrap_or(true),
            "a frame arrived from a machine with no camera"
        );
    }

}

#[cfg(test)]
mod open_camera_tests {
    use crate::backend::{CaptureBackend, CaptureConfig, DropPolicy};
    use crate::service::{CaptureCommand, CaptureService};
    use iris_hal::device::PixelFormat;
    use tokio::sync::broadcast;
    use tokio::time::{timeout, Duration};
    // --- which camera is open ----------------------------------------------
    // Nothing reported which device capture had opened, so the window marked
    // whichever camera it had asked for (the IPC dispatcher ignored the request)
    // and GetStatus named the first camera found, even when IRIS_DEVICE chose
    // another.

    fn tiny_cfg() -> CaptureConfig {
        CaptureConfig {
            width: 8,
            height: 8,
            target_fps: 30,
            format: PixelFormat::Rgb24,
            max_queue_depth: 4,
            drop_policy: DropPolicy::Oldest,
            roi: None,
        }
    }

    #[tokio::test]
    async fn a_uvc_backend_names_the_camera_it_opened_until_it_stops() {
        use crate::backend::{OpenedCamera, UvcCaptureBackend};
        let mut backend = UvcCaptureBackend::new(iris_hal::backend::MockUvcBackend::new(), tiny_cfg());
        assert_eq!(backend.device(), None, "nothing is open before start");
        backend.start().await.unwrap();
        let mock0 = OpenedCamera { id: "mock-0".into(), name: "Mock Camera".into() };
        assert_eq!(backend.device(), Some(mock0.clone()));
        // Boxed, as the application holds every backend.
        let boxed: Box<dyn CaptureBackend + Send + Sync> = Box::new(backend);
        assert_eq!(boxed.device(), Some(mock0));
        let mut boxed = boxed;
        boxed.stop().await.unwrap();
        assert_eq!(boxed.device(), None, "a stopped backend has nothing open");
    }

    #[tokio::test]
    async fn the_service_publishes_the_open_camera_and_withdraws_it_on_stop() {
        use crate::backend::{OpenedCamera, UvcCaptureBackend};
        let backend = UvcCaptureBackend::new(iris_hal::backend::MockUvcBackend::new(), tiny_cfg());
        let (tx, _rx) = broadcast::channel(64);
        let (svc, handle) = CaptureService::new(backend, tiny_cfg(), tx);
        let mut device = handle.device_rx.clone();
        assert_eq!(*device.borrow(), None);
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(8);
        let svc_task = tokio::spawn(svc.run(cmd_rx));
        let opened = timeout(Duration::from_secs(5), device.wait_for(|d| d.is_some()))
            .await
            .expect("the open camera was never published")
            .unwrap()
            .clone();
        assert_eq!(opened, Some(OpenedCamera { id: "mock-0".into(), name: "Mock Camera".into() }));
        cmd_tx.send(CaptureCommand::Stop).await.unwrap();
        timeout(Duration::from_secs(5), svc_task).await.expect("Stop must end the service").unwrap();
        // Read after the service has gone: the receiver keeps the last value.
        assert_eq!(*handle.device_rx.borrow(), None, "a stopped capture still names its camera");
    }

    #[tokio::test]
    async fn no_camera_is_ever_published_when_there_is_none() {
        use crate::backend::NoCameraBackend;
        let (tx, _rx) = broadcast::channel(8);
        let (svc, handle) = CaptureService::new(NoCameraBackend, tiny_cfg(), tx);
        let (_cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(8);
        timeout(Duration::from_secs(5), tokio::spawn(svc.run(cmd_rx))).await.unwrap().unwrap();
        assert_eq!(*handle.device_rx.borrow(), None);
    }
}
