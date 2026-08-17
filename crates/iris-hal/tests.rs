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
}
