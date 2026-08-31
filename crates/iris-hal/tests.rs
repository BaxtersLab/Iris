#[cfg(test)]
mod tests {
    use crate::backend::{MockUvcBackend, UvcBackend};
    use crate::device::DeviceId;

    #[tokio::test]
    async fn enumerate_returns_mock_device() {
        let backend = MockUvcBackend::new();
        let devs = backend.enumerate_devices().await.unwrap();
        assert!(!devs.is_empty());
        assert_eq!(devs[0].id.0, "mock-0");
    }

    #[tokio::test]
    async fn probe_capabilities_succeeds() {
        let backend = MockUvcBackend::new();
        let caps = backend
            .probe_capabilities(&DeviceId("mock-0".into()))
            .await
            .unwrap();
        assert!(!caps.formats.is_empty());
    }

    #[tokio::test]
    async fn open_close_device_flow() {
        let backend = MockUvcBackend::new();
        let id = DeviceId("mock-0".into());
        backend.open_device(&id).await.unwrap();
        let frame = backend.read_frame(&id).await.unwrap();
        assert!(!frame.is_empty());
        backend.close_device(&id).await.unwrap();
    }

    #[tokio::test]
    async fn control_get_set() {
        let backend = MockUvcBackend::new();
        let id = DeviceId("mock-0".into());
        let list = backend.list_controls(&id).await.unwrap();
        assert!(!list.is_empty());
        let val = backend.get_control(&id, list[0].id).await.unwrap();
        assert!(val >= list[0].min && val <= list[0].max);
        backend.set_control(&id, list[0].id, 200).await.unwrap();
        let val2 = backend.get_control(&id, list[0].id).await.unwrap();
        assert_eq!(val2, 200);
    }

    #[tokio::test]
    async fn read_frame_requires_open() {
        let backend = MockUvcBackend::new();
        let id = DeviceId("mock-0".into());
        let res = backend.read_frame(&id).await;
        assert!(res.is_err());
    }

    /// Hardware-gated WMF test — only runs when IRIS_USE_HW=1.
    /// Requires a real webcam plugged in.
    #[cfg(windows)]
    #[tokio::test]
    async fn wmf_enumerate_real_devices() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping wmf_enumerate_real_devices (set IRIS_USE_HW=1)");
            return;
        }
        let backend = crate::backend::new_wmf_backend().expect("WmfBackend::new() failed");
        let devs = backend
            .enumerate_devices()
            .await
            .expect("enumerate_devices failed");
        eprintln!("WMF devices found: {}", devs.len());
        for d in &devs {
            eprintln!("  {} — {}", d.id, d.name);
        }
        assert!(!devs.is_empty(), "No video capture devices found");
    }

    /// The enumeration path `bootstrap.rs` actually uses must keep working now
    /// that `WmfUvcBackend` is no longer a `UvcBackend`.
    ///
    /// The Linux mirror of this concern is
    /// `v4l2_unopened_device_reports_not_open_not_unimplemented`, which proves
    /// the V4L2 stubs are gone by asserting real state. The WMF stubs were
    /// removed rather than filled, so the equivalent guard is different: that
    /// the one call site still resolves and still returns real devices.
    ///
    /// Hardware-gated — it needs a camera Windows can see.
    #[cfg(windows)]
    #[test]
    fn wmf_enumeration_survives_removing_the_backend_impl() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping wmf_enumeration_survives_removing_the_backend_impl (set IRIS_USE_HW=1)");
            return;
        }
        let devs = crate::wmf_backend::wmf::WmfUvcBackend::enumerate_sync()
            .expect("enumerate_sync failed");
        eprintln!("WMF enumerate_sync found {} device(s):", devs.len());
        for d in &devs {
            eprintln!("  {} - {}", d.id, d.name);
        }
        assert!(
            !devs.is_empty(),
            "enumerate_sync returned no devices; bootstrap.rs would fall back to the mock backend"
        );
    }

    /// Hardware-gated WMF CAPTURE test — IRIS_USE_HW=1 + a real webcam.
    /// Proves the full deep-backend path: enumerate → open → read frames.
    #[cfg(windows)]
    #[tokio::test]
    async fn wmf_capture_real_frames() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping wmf_capture_real_frames (set IRIS_USE_HW=1)");
            return;
        }
        let backend = crate::backend::new_wmf_backend().expect("WmfBackend::new() failed");
        let devs = backend.enumerate_devices().await.expect("enumerate failed");
        assert!(!devs.is_empty(), "no camera found");
        let id = &devs[0].id;
        eprintln!("capturing from: {} ({})", devs[0].name, id);

        let caps = backend.probe_capabilities(id).await.expect("probe failed");
        eprintln!("formats reported: {}", caps.formats.len());
        assert!(!caps.formats.is_empty(), "camera reported no formats");

        backend.open_device(id).await.expect("open failed");
        let mut sizes = Vec::new();
        for i in 0..5 {
            let frame = backend.read_frame(id).await.expect("read_frame failed");
            eprintln!("frame {}: {} bytes", i + 1, frame.len());
            assert!(!frame.is_empty(), "empty frame");
            sizes.push(frame.len());
        }
        backend.close_device(id).await.expect("close failed");
        // frames of a fixed mode should be consistently sized
        assert!(sizes.iter().all(|s| *s == sizes[0]),
                "inconsistent frame sizes: {sizes:?}");
    }

    // ---- V4L2 backend (Linux mirror of the WMF tests) ----------------------

    #[cfg(target_os = "linux")]
    #[test]
    fn v4l2_fourcc_maps_known_formats() {
        use crate::device::PixelFormat;
        use crate::v4l2_backend::v4l2::fourcc_to_pixel_format;
        assert_eq!(fourcc_to_pixel_format(0x5659_5559), Some(PixelFormat::Yuyv)); // YUYV
        assert_eq!(fourcc_to_pixel_format(0x3231_564e), Some(PixelFormat::Nv12)); // NV12
        assert_eq!(fourcc_to_pixel_format(0x3342_4752), Some(PixelFormat::Rgb24)); // RGB3
        assert_eq!(fourcc_to_pixel_format(0x3352_4742), Some(PixelFormat::Bgr24)); // BGR3
        // MJPG is now enumerated (2026-08-01). Previously this asserted None,
        // which meant a 1080p-capable USB 2.0 UVC camera reported as 640x480-only
        // on Linux: every mode above ~640x480 on such cameras is MJPEG-only, and
        // unmapped fourccs are skipped entirely by the ENUM_FMT loop.
        assert_eq!(fourcc_to_pixel_format(0x4750_4a4d), Some(PixelFormat::Mjpeg)); // MJPG
        assert_eq!(fourcc_to_pixel_format(0xDEAD_BEEF), None); // genuinely unknown
    }

    /// MJPEG is compressed: it must be flagged as non-raw so callers never try
    /// to crop, stride or index it as if it were a pixel grid.
    #[test]
    fn mjpeg_is_not_raw_but_others_are() {
        use crate::device::PixelFormat;
        assert!(!PixelFormat::Mjpeg.is_raw());
        assert!(PixelFormat::Yuyv.is_raw());
        assert!(PixelFormat::Nv12.is_raw());
        assert!(PixelFormat::Rgb24.is_raw());
        assert!(PixelFormat::Bgr24.is_raw());
    }

    #[test]
    fn pixel_format_display_includes_mjpeg() {
        use crate::device::PixelFormat;
        assert_eq!(PixelFormat::Mjpeg.to_string(), "MJPEG");
        assert_eq!(PixelFormat::Yuyv.to_string(), "YUYV");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn v4l2_video_node_filter() {
        use crate::v4l2_backend::v4l2::is_video_node;
        assert!(is_video_node("video0"));
        assert!(is_video_node("video12"));
        assert!(!is_video_node("video"));
        assert!(!is_video_node("radio0"));
        assert!(!is_video_node("videoX"));
        assert!(!is_video_node("subdev0"));
    }

    /// The hand-coded ioctl request numbers encode the V4L2 struct sizes —
    /// this guards against struct-layout regressions.
    #[cfg(target_os = "linux")]
    #[test]
    fn v4l2_ioctl_numbers_match_abi() {
        use crate::v4l2_backend::v4l2::*;
        assert_eq!(VIDIOC_QUERYCAP, 0x8068_5600);
        assert_eq!(VIDIOC_ENUM_FMT, 0xc040_5602);
        assert_eq!(VIDIOC_ENUM_FRAMESIZES, 0xc02c_564a);
        assert_eq!(VIDIOC_ENUM_FRAMEINTERVALS, 0xc034_564b);
    }

    /// Enumeration must never error on a machine with no camera — it returns
    /// an empty list (mirrors WMF behaviour). Runs everywhere, incl. containers.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn v4l2_enumerate_no_camera_is_ok() {
        use crate::v4l2_backend::v4l2::V4l2UvcBackend;
        let backend = V4l2UvcBackend::new();
        let devs = backend.enumerate_devices().await.expect("enumerate must not error");
        eprintln!("V4L2 devices found: {}", devs.len());
    }

    /// Hardware-gated V4L2 test — only runs when IRIS_USE_HW=1.
    /// Requires a real webcam plugged in.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn v4l2_enumerate_real_devices() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping v4l2_enumerate_real_devices (set IRIS_USE_HW=1)");
            return;
        }
        // IRIS_USE_HW=1 says "exercise the hardware", not "a camera is
        // guaranteed". Tell the two apart: with no /dev/video* node at all
        // nothing is plugged in, which is an environment fact and a skip. With
        // a node present but nothing enumerated, that IS the regression this
        // test exists to catch, and the assert below fires.
        if !crate::v4l2_backend::v4l2::V4l2UvcBackend::video_nodes_present() {
            eprintln!(
                "SKIP {}: IRIS_USE_HW=1 but no /dev/video* node exists — no camera attached",
                "v4l2_enumerate_real_devices"
            );
            return;
        }
        use crate::backend::UvcBackend as _;
        use crate::v4l2_backend::v4l2::V4l2UvcBackend;
        let backend = V4l2UvcBackend::new();
        let devs = backend.enumerate_devices().await.expect("enumerate failed");
        eprintln!("V4L2 devices found: {}", devs.len());
        for d in &devs {
            eprintln!("  {} — {}", d.id, d.name);
        }
        assert!(!devs.is_empty(), "No video capture devices found");
        let caps = backend.probe_capabilities(&devs[0].id).await.expect("probe failed");
        eprintln!("formats: {:?}", caps.formats);
        assert!(!caps.formats.is_empty(), "capture device reported no supported formats");
    }

    /// The five formerly-`NotImplemented` methods must report real state.
    /// Needs no camera: an un-opened backend has to answer `DeviceNotOpen`,
    /// which is only possible once the stubs are actually gone. If any of these
    /// regress to `NotImplemented` the match fails.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn v4l2_unopened_device_reports_not_open_not_unimplemented() {
        use crate::backend::UvcBackend as _;
        use crate::device::DeviceId;
        use crate::error::HalError;
        use crate::v4l2_backend::v4l2::V4l2UvcBackend;

        let backend = V4l2UvcBackend::new();
        let id = DeviceId("/dev/video-does-not-exist".into());

        assert!(
            matches!(backend.read_frame(&id).await, Err(HalError::DeviceNotOpen)),
            "read_frame on an unopened device must be DeviceNotOpen"
        );
        assert!(
            matches!(backend.close_device(&id).await, Err(HalError::DeviceNotOpen)),
            "close_device on an unopened device must be DeviceNotOpen"
        );
        assert!(
            matches!(
                backend.get_control(&id, 0x0098_0900).await,
                Err(HalError::DeviceNotOpen)
            ),
            "get_control on an unopened device must be DeviceNotOpen"
        );
        assert!(
            matches!(
                backend.set_control(&id, 0x0098_0900, 1).await,
                Err(HalError::DeviceNotOpen)
            ),
            "set_control on an unopened device must be DeviceNotOpen"
        );
        assert!(
            matches!(backend.current_format(&id).await, Ok(None)),
            "current_format on an unopened device must be Ok(None)"
        );
    }

    /// Opening a path that is not a V4L2 node must fail cleanly rather than
    /// panicking or leaving state behind.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn v4l2_open_bad_path_fails_cleanly_and_leaves_backend_closed() {
        use crate::backend::UvcBackend as _;
        use crate::device::DeviceId;
        use crate::error::HalError;
        use crate::v4l2_backend::v4l2::V4l2UvcBackend;

        let backend = V4l2UvcBackend::new();
        let id = DeviceId("/dev/null".into());
        assert!(backend.open_device(&id).await.is_err(), "/dev/null is not a capture device");
        // the failed open must not have left a half-open device behind
        assert!(
            matches!(backend.read_frame(&id).await, Err(HalError::DeviceNotOpen)),
            "a failed open must leave the backend closed"
        );
    }

    /// Hardware-gated V4L2 CAPTURE test — IRIS_USE_HW=1 plus a real webcam.
    /// This is the Linux mirror of `wmf_capture_real_frames`.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn v4l2_capture_real_frames() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping v4l2_capture_real_frames (set IRIS_USE_HW=1)");
            return;
        }
        // IRIS_USE_HW=1 says "exercise the hardware", not "a camera is
        // guaranteed". Tell the two apart: with no /dev/video* node at all
        // nothing is plugged in, which is an environment fact and a skip. With
        // a node present but nothing enumerated, that IS the regression this
        // test exists to catch, and the assert below fires.
        if !crate::v4l2_backend::v4l2::V4l2UvcBackend::video_nodes_present() {
            eprintln!(
                "SKIP {}: IRIS_USE_HW=1 but no /dev/video* node exists — no camera attached",
                "v4l2_capture_real_frames"
            );
            return;
        }
        use crate::backend::UvcBackend as _;
        use crate::device::PixelFormat;
        use crate::error::HalError;
        use crate::v4l2_backend::v4l2::V4l2UvcBackend;

        let backend = V4l2UvcBackend::new();
        let devs = backend.enumerate_devices().await.expect("enumerate failed");
        assert!(!devs.is_empty(), "no capture device present");
        let id = devs[0].id.clone();
        eprintln!("opening {} — {}", devs[0].id, devs[0].name);

        backend.open_device(&id).await.expect("open_device failed");

        let fmt = backend
            .current_format(&id)
            .await
            .expect("current_format failed")
            .expect("an open device must report the format it granted");
        eprintln!(
            "negotiated: {}x{} @{}fps {:?}",
            fmt.width, fmt.height, fmt.fps, fmt.pixel_format
        );
        assert!(
            fmt.width > 0 && fmt.height > 0,
            "driver reported a zero-sized format"
        );

        assert!(
            matches!(
                backend.open_device(&id).await,
                Err(HalError::DeviceAlreadyOpen)
            ),
            "opening an already-open device must be refused"
        );

        let controls = backend.list_controls(&id).await.expect("list_controls failed");
        eprintln!("controls exposed: {}", controls.len());

        let mut frames: Vec<Vec<u8>> = Vec::new();
        for i in 0..5 {
            let frame = backend
                .read_frame(&id)
                .await
                .unwrap_or_else(|e| panic!("read_frame {i} failed: {e}"));
            assert!(!frame.is_empty(), "frame {i} came back empty");
            frames.push(frame);
        }
        eprintln!(
            "frame sizes: {:?}",
            frames.iter().map(|f| f.len()).collect::<Vec<_>>()
        );

        // Prove the bytes are a real frame rather than an uninitialised or
        // stale mapping. MJPEG frames are self-describing: SOI at the start,
        // EOI at the end. For raw formats the size is fully determined, so
        // check that instead.
        match fmt.pixel_format {
            PixelFormat::Mjpeg => {
                for (i, f) in frames.iter().enumerate() {
                    assert_eq!(
                        &f[..2],
                        &[0xFF, 0xD8],
                        "frame {i} has no JPEG SOI marker — not a decodable frame"
                    );
                    let eoi = f
                        .windows(2)
                        .rposition(|w| w == [0xFF, 0xD9])
                        .unwrap_or_else(|| panic!("frame {i} contains no JPEG EOI marker"));
                    eprintln!(
                        "  frame {i}: len={} eoi_at={} trailing={}",
                        f.len(),
                        eoi,
                        f.len() - (eoi + 2)
                    );
                    // EOI must be at the end, allowing a padding byte: this
                    // camera pads each frame to an even length, so a JPEG whose
                    // natural length is odd carries one trailing stuffing byte.
                    // That is padding, not truncation — a torn frame stops
                    // thousands of bytes early or has no EOI at all, which this
                    // still catches.
                    let trailing = f.len() - (eoi + 2);
                    assert!(
                        trailing <= 2,
                        "frame {i} is truncated: EOI at {eoi} of {} bytes ({trailing} trailing)",
                        f.len()
                    );
                }
            }
            PixelFormat::Yuyv => {
                let expected = (fmt.width * fmt.height * 2) as usize;
                for (i, f) in frames.iter().enumerate() {
                    assert_eq!(f.len(), expected, "YUYV frame {i} is the wrong size");
                }
            }
            other => eprintln!("no size/marker rule for {other:?}; length checked only"),
        }

        backend.close_device(&id).await.expect("close_device failed");
        assert!(
            matches!(backend.read_frame(&id).await, Err(HalError::DeviceNotOpen)),
            "reading after close must fail"
        );
    }
}

/// `capture.pixel_format` in `iris.toml` was a dead string until 2026-08-31:
/// `bootstrap.rs` hardcoded `PixelFormat::Bgr24` and never read it. These pin
/// the parser that now routes it, and — the part that matters — pin the two
/// lists to each other, since they live in different crates and previously had
/// no relationship at all.
#[cfg(test)]
mod pixel_format_config_names {
    use crate::device::PixelFormat;

    #[test]
    fn every_name_validate_accepts_can_actually_be_parsed() {
        for name in iris_core::config::ALLOWED_PIXEL_FORMATS {
            assert!(
                PixelFormat::from_config_name(name).is_some(),
                "IrisConfig::validate accepts '{name}' but nothing can parse it \
                 — a config that passes validation would then be discarded"
            );
        }
    }

    #[test]
    fn every_canonical_name_round_trips() {
        for f in [
            PixelFormat::Rgb24,
            PixelFormat::Bgr24,
            PixelFormat::Nv12,
            PixelFormat::Yuyv,
            PixelFormat::Mjpeg,
        ] {
            let name = f.config_name();
            assert_eq!(PixelFormat::from_config_name(name), Some(f.clone()));
            assert!(
                iris_core::config::ALLOWED_PIXEL_FORMATS.contains(&name),
                "'{name}' parses but IrisConfig::validate would reject it"
            );
        }
    }

    #[test]
    fn yuy2_is_accepted_as_the_windows_spelling_of_yuyv() {
        assert_eq!(
            PixelFormat::from_config_name("yuy2"),
            Some(PixelFormat::Yuyv)
        );
    }

    #[test]
    fn parsing_is_case_insensitive() {
        assert_eq!(PixelFormat::from_config_name("NV12"), Some(PixelFormat::Nv12));
        assert_eq!(PixelFormat::from_config_name("MJPEG"), Some(PixelFormat::Mjpeg));
    }

    /// `bgra8` was in the old allowed list. No Iris backend has ever produced
    /// it, and it is 4 bytes per pixel where the nearest variant is 3, so
    /// mapping it to `Bgr24` would mis-size every frame. It must be refused.
    #[test]
    fn unknown_and_unsupported_names_are_refused() {
        assert_eq!(PixelFormat::from_config_name("bgra8"), None);
        assert_eq!(PixelFormat::from_config_name(""), None);
        assert_eq!(PixelFormat::from_config_name("rgb32"), None);
    }
}
