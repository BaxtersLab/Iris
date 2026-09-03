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
        backend.open_device(&id, None).await.unwrap();
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

    /// Windows camera controls must report real ranges and actually take
    /// effect — the gap opened on 2026-08-31 and closed here.
    ///
    /// This asserts **state**, not that calls returned `Ok`: the ranges have to
    /// be internally consistent, and a value written has to read back changed.
    /// A backend that accepted every `set_control` and ignored it would pass a
    /// mere `is_ok()` check and fail this one.
    ///
    /// Hardware-gated: needs a camera Windows can see.
    #[cfg(windows)]
    #[tokio::test]
    async fn wmf_controls_report_real_ranges_and_take_effect() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping wmf_controls_report_real_ranges_and_take_effect (set IRIS_USE_HW=1)");
            return;
        }
        // Serialise camera access: see `camera_guard`.
        let _camera = camera_guard();
        use crate::backend::UvcBackend as _;
        let backend = crate::backend::new_wmf_backend().expect("WmfBackend::new() failed");
        let devs = backend.enumerate_devices().await.expect("enumerate failed");
        assert!(!devs.is_empty(), "no camera found");
        let id = &devs[0].id;
        backend.open_device(id, None).await.expect("open failed");

        let controls = backend.list_controls(id).await.expect("list_controls failed");
        eprintln!("WMF exposed {} control(s):", controls.len());
        for c in &controls {
            eprintln!(
                "  id={:#06x} {:<24} min={:<6} max={:<6} step={:<4} default={}",
                c.id, c.name, c.min, c.max, c.step, c.default
            );
        }
        assert!(
            !controls.is_empty(),
            "list_controls returned nothing; Windows camera controls are still unimplemented"
        );

        // Ranges must be internally consistent. A backend inventing values
        // rather than reading the driver tends to fail this.
        for c in &controls {
            assert!(c.min < c.max, "{}: min {} not below max {}", c.name, c.min, c.max);
            assert!(c.step > 0, "{}: step {} must be positive", c.name, c.step);
            assert!(
                c.default >= c.min && c.default <= c.max,
                "{}: default {} outside [{}, {}]",
                c.name, c.default, c.min, c.max
            );
        }

        // Every control must be readable, and the value must sit in its range.
        for c in &controls {
            let v = backend
                .get_control(id, c.id)
                .await
                .unwrap_or_else(|e| panic!("get_control({}) failed: {e:?}", c.name));
            assert!(
                v >= c.min && v <= c.max,
                "{}: current {} outside its own reported range [{}, {}]",
                c.name, v, c.min, c.max
            );
        }

        // Now prove a write takes effect. Pick the first control with room to
        // move, write a value that is definitely not the current one, and read
        // it back.
        let target = controls
            .iter()
            .find(|c| c.max - c.min >= c.step * 2)
            .expect("no control with a usable range");
        let before = backend.get_control(id, target.id).await.expect("get before");
        let want = if before + target.step <= target.max {
            before + target.step
        } else {
            before - target.step
        };
        backend
            .set_control(id, target.id, want)
            .await
            .unwrap_or_else(|e| panic!("set_control({} -> {want}) failed: {e:?}", target.name));
        let after = backend.get_control(id, target.id).await.expect("get after");
        eprintln!(
            "  set {} {} -> {}, read back {}",
            target.name, before, want, after
        );
        assert_ne!(
            after, before,
            "{} did not change: set_control was accepted but had no effect",
            target.name
        );

        // Put it back so the camera is left as found.
        let _ = backend.set_control(id, target.id, before).await;
        backend.close_device(id).await.expect("close failed");
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
        // Serialise camera access: see `camera_guard`.
        let _camera = camera_guard();
        let backend = crate::backend::new_wmf_backend().expect("WmfBackend::new() failed");
        let devs = backend.enumerate_devices().await.expect("enumerate failed");
        assert!(!devs.is_empty(), "no camera found");
        let id = &devs[0].id;
        eprintln!("capturing from: {} ({})", devs[0].name, id);

        let caps = backend.probe_capabilities(id).await.expect("probe failed");
        eprintln!("formats reported: {}", caps.formats.len());
        assert!(!caps.formats.is_empty(), "camera reported no formats");

        backend.open_device(id, None).await.expect("open failed");
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

    /// Hardware: a rate the camera does not advertise must be reported and must
    /// not break capture.
    ///
    /// This is the frame-rate test that **can** run on the attached hardware.
    /// Every mode on this camera offers exactly 30fps, so the transition test
    /// below has nothing to choose between and skips.
    ///
    /// **Be clear about what this does and does not prove.** It proves the
    /// *never worse off* property: an impossible request does not fail the
    /// open, the device settles on a rate it does advertise, capture still
    /// delivers, and the difference is reportable. It does **not** prove the
    /// request reached the driver — with only one advertised rate, a
    /// `negotiate_fps` that did nothing at all would produce this same result.
    /// Proving the request lands needs a camera offering two rates, which this
    /// one does not; that is what the transition test below is for, and why it
    /// skips rather than being deleted.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn v4l2_an_unadvertised_frame_rate_leaves_a_working_camera() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping v4l2_an_unadvertised_frame_rate_leaves_a_working_camera (set IRIS_USE_HW=1)");
            return;
        }
        if !crate::v4l2_backend::v4l2::V4l2UvcBackend::video_nodes_present() {
            eprintln!("SKIP: IRIS_USE_HW=1 but no /dev/video* node exists — no camera attached");
            return;
        }
        let _camera = camera_guard();
        use crate::backend::UvcBackend as _;
        use crate::device::FormatRequest;
        use crate::v4l2_backend::v4l2::V4l2UvcBackend;

        let backend = V4l2UvcBackend::new();
        let devs = backend.enumerate_devices().await.expect("enumerate failed");
        assert!(!devs.is_empty(), "no capture devices");
        let id = &devs[0].id;
        let caps = backend.probe_capabilities(id).await.expect("probe failed");
        let mode = caps.formats.first().cloned().expect("no formats advertised");
        let rates = backend
            .advertised_frame_rates(id, &mode)
            .await
            .expect("interval enumeration failed");

        // Pick something the mode definitely does not offer.
        let impossible = (1..=240u32).find(|r| !rates.contains(r)).expect("no free rate");
        eprintln!(
            "{}x{} {} offers {rates:?}; asking for {impossible}fps",
            mode.width, mode.height, mode.pixel_format
        );

        let req = FormatRequest {
            width: mode.width,
            height: mode.height,
            pixel_format: mode.pixel_format.clone(),
            fps: impossible,
        };
        backend
            .open_device(id, Some(&req))
            .await
            .expect("an impossible rate must not fail the open");
        let got = backend
            .current_format(id)
            .await
            .expect("current_format failed")
            .expect("no current format");
        // The camera must still deliver — this is the "never worse off" property.
        let frame = backend.read_frame(id).await.expect("capture must still work");
        backend.close_device(id).await.expect("close failed");

        eprintln!("granted {}fps, frame of {} bytes", got.fps, frame.len());
        assert!(!frame.is_empty(), "a frame must still arrive");
        assert_ne!(
            got.fps, impossible,
            "the driver cannot have granted a rate it does not advertise"
        );
        assert!(
            rates.is_empty() || rates.contains(&got.fps),
            "the device settled on {}fps, which is not among the advertised {rates:?}",
            got.fps
        );
        // And the mismatch must be describable rather than silent.
        assert!(
            req.describe_mismatch(got.width, got.height, &got.pixel_format, Some(got.fps))
                .is_some(),
            "an ungranted rate must be reportable"
        );
    }

    /// Hardware: request two *different* advertised frame rates and confirm the
    /// device follows — the rate twin of
    /// `v4l2_negotiates_between_two_advertised_modes`, and a transition for the
    /// same reason: a V4L2 device keeps the last interval set on it, so a
    /// single match can be a coincidence left over from a previous run.
    ///
    /// Skips honestly when the attached camera offers only one rate for a mode.
    /// A camera with nothing to choose between cannot demonstrate a choice, and
    /// pretending otherwise would be the same false green this suite has now
    /// caught twice.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn v4l2_negotiates_between_two_advertised_frame_rates() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping v4l2_negotiates_between_two_advertised_frame_rates (set IRIS_USE_HW=1)");
            return;
        }
        if !crate::v4l2_backend::v4l2::V4l2UvcBackend::video_nodes_present() {
            eprintln!("SKIP: IRIS_USE_HW=1 but no /dev/video* node exists — no camera attached");
            return;
        }
        let _camera = camera_guard();
        use crate::backend::UvcBackend as _;
        use crate::device::FormatRequest;
        use crate::v4l2_backend::v4l2::V4l2UvcBackend;

        let backend = V4l2UvcBackend::new();
        let devs = backend.enumerate_devices().await.expect("enumerate failed");
        assert!(!devs.is_empty(), "no capture devices");
        let id = &devs[0].id;
        let caps = backend.probe_capabilities(id).await.expect("probe failed");
        // Search EVERY advertised mode for one offering a choice of rate, not
        // just the first. A camera commonly offers several rates only at lower
        // resolutions, and testing the first mode alone would skip on hardware
        // that could in fact demonstrate the transition.
        let mut found = None;
        for m in &caps.formats {
            let rates = backend
                .advertised_frame_rates(id, m)
                .await
                .expect("interval enumeration failed");
            eprintln!(
                "  {}x{} {} offers rates {rates:?}",
                m.width, m.height, m.pixel_format
            );
            if rates.len() >= 2 && found.is_none() {
                found = Some((m.clone(), rates));
            }
        }
        let (mode, rates) = match found {
            Some(v) => v,
            None => {
                eprintln!(
                    "SKIP: no mode on this camera advertises more than one frame rate — \
                     a transition cannot be demonstrated on this hardware. The negotiation \
                     path itself is covered by the pure tests and by the live run."
                );
                return;
            }
        };
        eprintln!(
            "using {}x{} {} with rates {rates:?}",
            mode.width, mode.height, mode.pixel_format
        );

        async fn ask(
            backend: &V4l2UvcBackend,
            id: &crate::device::DeviceId,
            mode: &crate::device::FormatDescriptor,
            fps: u32,
        ) -> u32 {
            let req = FormatRequest {
                width: mode.width,
                height: mode.height,
                pixel_format: mode.pixel_format.clone(),
                fps,
            };
            backend.open_device(id, Some(&req)).await.expect("open failed");
            let got = backend
                .current_format(id)
                .await
                .expect("current_format failed")
                .expect("no current format");
            backend.close_device(id).await.expect("close failed");
            eprintln!("requested {fps}fps -> granted {}fps", got.fps);
            got.fps
        }
        let first = ask(&backend, id, &mode, rates[0]).await;
        let second = ask(&backend, id, &mode, rates[1]).await;

        assert_ne!(
            first, second,
            "both requests returned the same rate — the interval is not reaching the driver"
        );
        assert_eq!(first, rates[0], "an advertised rate was requested but not granted");
        assert_eq!(second, rates[1], "an advertised rate was requested but not granted");
    }

    /// One camera, one test at a time.
    ///
    /// libtest runs tests on parallel threads, and the V4L2 hardware tests all
    /// open `/dev/video0` and call `VIDIOC_S_FMT`. A second opener gets
    /// `Device or resource busy (os error 16)`, so without this they fail each
    /// other intermittently depending on scheduling — which is exactly the
    /// defect the Windows box found in round 5, where two single-instance tests
    /// shared one process-global mutex name and failed 12 runs in 20.
    ///
    /// The unix lock test there solved it by giving each test its own resource.
    /// That is not available here: there is only one camera, so the resource
    /// has to be taken in turn instead.
    ///
    /// **Windows needs it too, and for the same reason** — round 8 measured the
    /// WMF hardware tests failing **5 runs in 5** in parallel with
    /// `ReadSample: The video recording device is preempted by another immersive
    /// application (0xC00D3EA3)`, and 0 in 5 serially. Different error code,
    /// identical defect. So this is `any(linux, windows)` rather than
    /// linux-only.
    #[cfg(any(target_os = "linux", windows))]
    static CAMERA: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Take the camera, tolerating a previous test having panicked while holding
    /// it — a poisoned lock means another test failed, not that this one cannot
    /// run, and reporting both failures is more useful than masking one.
    #[cfg(any(target_os = "linux", windows))]
    fn camera_guard() -> std::sync::MutexGuard<'static, ()> {
        CAMERA.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- frame rate ---------------------------------------------------------
    //
    // `capture.target_fps` was the THIRD config knob in the same struct to be
    // validated, passed down, and honoured by the mock backend alone. It
    // outlived two rounds of format negotiation because on Linux the interval
    // is a different ioctl (`VIDIOC_S_PARM`) from the pixel mode (`S_FMT`), so
    // it was never in the struct being fixed.

    /// `timeperframe` must round, not truncate.
    ///
    /// NTSC-derived cameras report `1001/30000` — 29.97fps — which must read as
    /// **30**. Truncating gives 29, which would make a correctly granted
    /// request look like a mismatch and emit a warning on every single launch.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_frame_interval_becomes_whole_frames_per_second() {
        use crate::v4l2_backend::v4l2::{V4l2Fract, V4l2UvcBackend};
        let fps = |n: u32, d: u32| {
            V4l2UvcBackend::fps_from_interval(&V4l2Fract {
                numerator: n,
                denominator: d,
            })
        };
        assert_eq!(fps(1, 30), Some(30), "the ordinary case");
        assert_eq!(fps(1, 15), Some(15));
        assert_eq!(fps(1001, 30000), Some(30), "29.97 must round to 30, not 29");
        assert_eq!(fps(1001, 60000), Some(60), "59.94 must round to 60");
        // A zero either side is a driver that has told us nothing, which is a
        // different thing from "zero frames per second".
        assert_eq!(fps(0, 30), None, "a zero numerator is not 0fps");
        assert_eq!(fps(1, 0), None, "a zero denominator is not 0fps");
    }

    /// `v4l2_streamparm` is 204 bytes, and the ioctl number encodes that size.
    ///
    /// Get it wrong and `S_PARM` fails with EINVAL in a way that reads as "the
    /// driver refused the frame rate" rather than "the struct is the wrong
    /// shape". The existing `v4l2_ioctl_numbers_match_abi` test exists for the
    /// same reason.
    #[cfg(target_os = "linux")]
    #[test]
    fn v4l2_streamparm_abi_matches_the_ioctl_number() {
        use crate::v4l2_backend::v4l2::{V4l2StreamParm, VIDIOC_G_PARM, VIDIOC_S_PARM};
        assert_eq!(std::mem::size_of::<V4l2StreamParm>(), 204);
        const DIR_READ_WRITE: libc::c_ulong = 3 << 30;
        let enc = |nr: libc::c_ulong, size: usize| -> libc::c_ulong {
            DIR_READ_WRITE | ((size as libc::c_ulong) << 16) | (0x56 << 8) | nr
        };
        assert_eq!(VIDIOC_G_PARM, enc(21, std::mem::size_of::<V4l2StreamParm>()));
        assert_eq!(VIDIOC_S_PARM, enc(22, std::mem::size_of::<V4l2StreamParm>()));
    }

    /// A frame-rate difference is a mismatch, and an unknown rate is not.
    ///
    /// `None` means the platform did not negotiate a rate — which is what the
    /// Windows backend passes until it matches `MF_MT_FRAME_RATE`. Comparing
    /// against a rate nobody read would invent mismatches out of nothing.
    #[test]
    fn an_unknown_frame_rate_is_not_a_mismatch_but_a_wrong_one_is() {
        use crate::device::{FormatRequest, PixelFormat};
        let req = FormatRequest {
            width: 640,
            height: 480,
            pixel_format: PixelFormat::Yuyv,
            fps: 30,
        };
        assert!(
            req.describe_mismatch(640, 480, &PixelFormat::Yuyv, Some(30)).is_none(),
            "the mode and the rate both granted: nothing to report"
        );
        assert!(
            req.describe_mismatch(640, 480, &PixelFormat::Yuyv, None).is_none(),
            "an unknown rate must not invent a mismatch"
        );
        let msg = req
            .describe_mismatch(640, 480, &PixelFormat::Yuyv, Some(15))
            .expect("a wrong rate is a mismatch even when the mode is right");
        assert!(msg.contains("30fps"), "must name the rate asked for: {msg}");
        assert!(msg.contains("15fps"), "must name the rate granted: {msg}");

        // fps 0 means "do not ask", so no rate can be wrong.
        let dont_care = FormatRequest { fps: 0, ..req.clone() };
        assert!(
            dont_care.describe_mismatch(640, 480, &PixelFormat::Yuyv, Some(5)).is_none(),
            "fps 0 means the rate was never requested"
        );
    }

    /// `is_satisfied_by` must keep ignoring `fps`, and that is a decision.
    ///
    /// It selects a native *mode*. Folding the rate in would make the Windows
    /// media-type search demand an exact rate match and find nothing on a
    /// camera whose default rate differs — turning a working negotiation into a
    /// silent fallback. Pinned so nobody "fixes" it by symmetry.
    #[test]
    fn mode_selection_deliberately_ignores_the_frame_rate() {
        use crate::device::{FormatRequest, PixelFormat};
        let req = FormatRequest {
            width: 640,
            height: 480,
            pixel_format: PixelFormat::Yuyv,
            fps: 30,
        };
        assert!(
            req.is_satisfied_by(640, 480, &PixelFormat::Yuyv),
            "a mode matching on geometry and format satisfies the request \
             regardless of rate — the rate is negotiated separately"
        );
    }

    // ---- format negotiation, the platform-independent half -----------------
    //
    // Contributed by the Windows box in round 7 and hoisted onto `FormatRequest`
    // during integration, so both backends share one implementation and these
    // two run everywhere.

    /// The decision `mode_satisfies` encodes is "exact match or nothing", and
    /// that is a choice rather than an accident — so it is pinned here.
    ///
    /// Media Foundation does not clamp. Where V4L2 hands back the nearest
    /// supported mode with a success code, MF takes a media type we picked, so
    /// a near-miss is ours to accept or refuse. Refusing is what keeps a
    /// caller able to notice; accepting silently is how this knob died.
    // No `cfg` gate: as of the round-7 integration these live on
    // `FormatRequest` in `device.rs`, so the decision is platform-independent
    // and is now checked on **both** platforms rather than only the one that
    // happened to implement it second.
    #[test]
    fn a_mode_satisfies_only_an_exact_request() {
        use crate::device::{FormatRequest, PixelFormat};
        let req = FormatRequest {
            width: 640,
            height: 480,
            pixel_format: PixelFormat::Yuyv,
            fps: 30,
        };
        assert!(req.is_satisfied_by(640, 480, &PixelFormat::Yuyv));
        // Each of the three fields alone must be enough to refuse.
        assert!(!req.is_satisfied_by(640, 480, &PixelFormat::Nv12), "format ignored");
        assert!(!req.is_satisfied_by(1920, 480, &PixelFormat::Yuyv), "width ignored");
        assert!(!req.is_satisfied_by(640, 1080, &PixelFormat::Yuyv), "height ignored");
        // And "close" is still no. 4:3 at a different scale is the shape of
        // near-miss a "nearest mode" search would happily return.
        assert!(!req.is_satisfied_by(320, 240, &PixelFormat::Yuyv), "nearest accepted");
    }

    /// A granted mode must produce no message and a refused one must name both
    /// sides. "Not available" without saying what *is* running leaves a caller
    /// unable to tell whether anything is.
    #[test]
    fn a_mismatch_names_both_what_was_asked_and_what_is_running() {
        use crate::device::{FormatRequest, PixelFormat};
        let req = FormatRequest {
            width: 640,
            height: 480,
            pixel_format: PixelFormat::Yuyv,
            fps: 30,
        };
        assert!(
            req.describe_mismatch(640, 480, &PixelFormat::Yuyv, None).is_none(),
            "a granted request must be silent"
        );
        let msg = req.describe_mismatch(1920, 1080, &PixelFormat::Nv12, None)
            .expect("a refused request must say so");
        for needle in ["640x480", "YUYV", "1920x1080", "NV12"] {
            assert!(msg.contains(needle), "message must contain {needle}: {msg}");
        }

        // The real case measured on the Linux box before negotiation existed:
        // the shipped default config asked for 4K NV12 and the camera delivered
        // 1080p MJPEG, silently. Kept as a scenario because it is the concrete
        // failure this whole thread began with.
        let default_cfg = FormatRequest {
            width: 3840,
            height: 2160,
            pixel_format: PixelFormat::Nv12,
            fps: 30,
        };
        assert!(
            default_cfg.describe_mismatch(3840, 2160, &PixelFormat::Nv12, None).is_none(),
            "an exact grant must be silent"
        );
        let msg = default_cfg
            .describe_mismatch(1920, 1080, &PixelFormat::Mjpeg, None)
            .expect("a clamped mode must be reported");
        for needle in ["3840x2160", "1920x1080", "MJPEG"] {
            assert!(msg.contains(needle), "message must contain {needle}: {msg}");
        }
    }

    /// Hardware: request two *different* advertised modes in turn and confirm
    /// the device follows. The Windows mirror of
    /// `v4l2_negotiates_between_two_advertised_modes`.
    ///
    /// **It asserts a transition, not a single match.** Round 7's brief warned
    /// that a device retains the last mode set on it, and that a
    /// single-request test passed on Linux with negotiation *deliberately
    /// disabled* because an earlier run had already left the camera in the
    /// requested mode. Broken negotiation cannot produce two different formats
    /// from two different requests, so the comparison between them is the real
    /// check and the individual assertions are only the diagnosis.
    #[cfg(windows)]
    #[tokio::test]
    async fn wmf_negotiates_between_two_advertised_modes() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping wmf_negotiates_between_two_advertised_modes (set IRIS_USE_HW=1)");
            return;
        }
        // Serialise camera access: see `camera_guard`.
        let _camera = camera_guard();
        use crate::backend::UvcBackend as _;
        use crate::device::FormatRequest;

        let backend = crate::backend::new_wmf_backend().expect("WmfBackend::new() failed");
        let devs = backend.enumerate_devices().await.expect("enumerate failed");
        assert!(!devs.is_empty(), "no camera found");
        let id = &devs[0].id;
        let caps = backend.probe_capabilities(id).await.expect("probe failed");
        assert!(
            caps.formats.len() >= 2,
            "need two advertised modes to prove a transition, got {}",
            caps.formats.len()
        );

        let small = caps.formats.iter().min_by_key(|f| f.width * f.height).cloned().unwrap();
        let large = caps.formats.iter().max_by_key(|f| f.width * f.height).cloned().unwrap();
        assert_ne!(
            (small.width, small.height),
            (large.width, large.height),
            "the two chosen modes must differ or the transition proves nothing"
        );

        async fn ask(
            backend: &crate::backend::WmfBackend,
            id: &crate::device::DeviceId,
            want: &crate::device::FormatDescriptor,
        ) -> (u32, u32, crate::device::PixelFormat) {
            let req = FormatRequest {
                width: want.width,
                height: want.height,
                pixel_format: want.pixel_format.clone(),
                fps: 30,
            };
            backend.open_device(id, Some(&req)).await.expect("open failed");
            let got = backend
                .current_format(id)
                .await
                .expect("current_format failed")
                .expect("no current format");
            backend.close_device(id).await.expect("close failed");
            eprintln!(
                "requested {}x{} {} -> granted {}x{} {}",
                want.width, want.height, want.pixel_format,
                got.width, got.height, got.pixel_format
            );
            (got.width, got.height, got.pixel_format)
        }

        // Largest first, then smallest: whatever mode a previous run left the
        // camera in, at least one of these two is a real change.
        let got_large = ask(&backend, id, &large).await;
        let got_small = ask(&backend, id, &small).await;

        assert_ne!(
            got_large, got_small,
            "both requests returned the same mode — the request is not reaching Media Foundation"
        );
        assert_eq!(
            got_large,
            (large.width, large.height, large.pixel_format.clone()),
            "the large mode was advertised and requested but not granted"
        );
        assert_eq!(
            got_small,
            (small.width, small.height, small.pixel_format.clone()),
            "the small mode was advertised and requested but not granted"
        );
    }

    /// An unsupported request must leave a *working* camera, not a broken one.
    ///
    /// The fallback is the reason negotiation cannot make things worse than
    /// before it existed: no native mode matches, so MF's default is kept and
    /// the caller is warned. A camera that refused to open on a bad config
    /// would be a regression introduced by the feature meant to help.
    #[cfg(windows)]
    #[tokio::test]
    async fn wmf_an_impossible_request_falls_back_to_a_working_camera() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping wmf_an_impossible_request_falls_back (set IRIS_USE_HW=1)");
            return;
        }
        // Serialise camera access: see `camera_guard`.
        let _camera = camera_guard();
        use crate::backend::UvcBackend as _;
        use crate::device::{FormatRequest, PixelFormat};

        let backend = crate::backend::new_wmf_backend().expect("WmfBackend::new() failed");
        let devs = backend.enumerate_devices().await.expect("enumerate failed");
        assert!(!devs.is_empty(), "no camera found");
        let id = &devs[0].id;

        // A resolution no webcam advertises.
        let req = FormatRequest {
            width: 12345,
            height: 6789,
            pixel_format: PixelFormat::Yuyv,
            fps: 30,
        };
        backend
            .open_device(id, Some(&req))
            .await
            .expect("an unsupported request must not fail the open");
        let got = backend
            .current_format(id)
            .await
            .expect("current_format failed")
            .expect("no current format");
        eprintln!("impossible request -> fell back to {}x{} {}", got.width, got.height, got.pixel_format);
        assert_ne!(
            (got.width, got.height),
            (req.width, req.height),
            "the device cannot actually be in a mode it does not advertise"
        );
        let frame = backend.read_frame(id).await.expect("camera must still deliver frames");
        assert!(!frame.is_empty(), "fallback left the camera not producing frames");
        backend.close_device(id).await.expect("close failed");
    }

    // ---- WMF frame-rate negotiation (round 8) -----------------------------

    /// `MF_MT_FRAME_RATE` must **round**, not truncate.
    ///
    /// 30000/1001 is 29.97fps — the rate every broadcast-derived device
    /// reports. Truncation yields 29, so a correctly granted 30fps request
    /// would warn on every single launch that it got 29. This read truncated
    /// until round 8.
    #[cfg(windows)]
    #[test]
    fn a_frame_rate_ratio_is_rounded_not_truncated() {
        use crate::backend::fps_from_ratio;
        assert_eq!(fps_from_ratio(30, 1), Some(30));
        assert_eq!(fps_from_ratio(60, 1), Some(60));
        // The cases truncation gets wrong:
        assert_eq!(fps_from_ratio(30000, 1001), Some(30), "29.97 must read as 30");
        assert_eq!(fps_from_ratio(60000, 1001), Some(60), "59.94 must read as 60");
        assert_eq!(fps_from_ratio(24000, 1001), Some(24), "23.976 must read as 24");
        // Genuinely fractional rates still round to nearest, not up.
        assert_eq!(fps_from_ratio(29, 2), Some(15), "14.5 rounds to 15");
        assert_eq!(fps_from_ratio(28, 2), Some(14));
        // A ratio with a zero on either side is not a rate.
        assert_eq!(fps_from_ratio(0, 1), None);
        assert_eq!(fps_from_ratio(30, 0), None);
    }

    /// The rate preference, which **cannot be tested on this box's camera**.
    ///
    /// Every native mode the reference camera advertises is 30/1, so a
    /// `choose_mode` ignoring the rate entirely would behave identically
    /// against real hardware. This is the only place the preference is
    /// exercised, which is why it covers the orderings rather than one case.
    #[cfg(windows)]
    #[test]
    fn the_search_prefers_a_matching_rate_but_settles_for_the_mode() {
        use crate::backend::choose_mode;
        use crate::device::{FormatDescriptor, FormatRequest, PixelFormat};

        let mode = |idx: u32, w: u32, h: u32, fps: u32| {
            (
                idx,
                FormatDescriptor { width: w, height: h, fps, pixel_format: PixelFormat::Nv12 },
            )
        };
        let req = |fps: u32| FormatRequest {
            width: 640,
            height: 480,
            pixel_format: PixelFormat::Nv12,
            fps,
        };

        // The rate match wins even though a mode-only match came FIRST — the
        // ordering that catches a search which stops at the first mode hit.
        let cands = vec![mode(0, 640, 480, 15), mode(1, 640, 480, 30)];
        assert_eq!(choose_mode(&req(30), &cands), Some((1, true)));

        // ...and when the rate match comes first, it still wins.
        let cands = vec![mode(0, 640, 480, 30), mode(1, 640, 480, 15)];
        assert_eq!(choose_mode(&req(30), &cands), Some((0, true)));

        // No mode offers the rate: settle for the mode, and say the rate did
        // not match. A wrong rate is much less bad than the wrong resolution.
        let cands = vec![mode(0, 640, 480, 15), mode(1, 640, 480, 20)];
        assert_eq!(choose_mode(&req(30), &cands), Some((0, false)));

        // fps == 0 is "do not ask": reduces to round 7's mode-only search, so
        // the first mode match wins regardless of rate.
        let cands = vec![mode(0, 640, 480, 15), mode(1, 640, 480, 30)];
        assert_eq!(choose_mode(&req(0), &cands), Some((0, false)));

        // The rate must never rescue a mode that does not match.
        let cands = vec![mode(0, 1920, 1080, 30)];
        assert_eq!(choose_mode(&req(30), &cands), None);

        // Nothing at all.
        assert_eq!(choose_mode(&req(30), &[]), None);
    }

    // ---- V4L2 backend (Linux mirror of the WMF tests) ----------------------

    /// Hardware: request two *different* advertised modes in turn and confirm
    /// the device follows.
    ///
    /// Everything else about negotiation is testable without a camera; **this
    /// is not.** The defect was that no request ever reached the driver, and no
    /// pure-function test can tell asking from not asking.
    ///
    /// **It asserts a transition, not a single match, and that is the whole
    /// design of it.** A V4L2 device *retains the last format set on it*, across
    /// opens and across processes. The first version of this test requested one
    /// mode and asserted it was granted — and it passed with negotiation
    /// deliberately disabled, because an earlier run had already left the camera
    /// in that mode and "adopt whatever it is in" returned the same answer. A
    /// broken negotiation cannot produce two *different* formats from two
    /// different requests, so the comparison between them is the real check.
    #[cfg(target_os = "linux")] // these exercise the V4L2 module, which does not exist on Windows
    #[tokio::test]
    async fn v4l2_negotiates_between_two_advertised_modes() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping v4l2_negotiates_between_two_advertised_modes (set IRIS_USE_HW=1)");
            return;
        }
        if !crate::v4l2_backend::v4l2::V4l2UvcBackend::video_nodes_present() {
            eprintln!("SKIP: IRIS_USE_HW=1 but no /dev/video* node exists — no camera attached");
            return;
        }
        let _camera = camera_guard();
        use crate::backend::UvcBackend as _;
        use crate::device::FormatRequest;
        use crate::v4l2_backend::v4l2::V4l2UvcBackend;

        let backend = V4l2UvcBackend::new();
        let devs = backend.enumerate_devices().await.expect("enumerate failed");
        assert!(!devs.is_empty(), "no capture devices");
        let id = &devs[0].id;
        let caps = backend.probe_capabilities(id).await.expect("probe failed");
        assert!(caps.formats.len() >= 2, "need two modes to prove a transition");

        let small = caps
            .formats
            .iter()
            .min_by_key(|f| f.width * f.height)
            .cloned()
            .unwrap();
        let large = caps
            .formats
            .iter()
            .max_by_key(|f| f.width * f.height)
            .cloned()
            .unwrap();
        assert_ne!(
            (small.width, small.height),
            (large.width, large.height),
            "the two chosen modes must differ or the transition proves nothing"
        );

        async fn ask(
            backend: &V4l2UvcBackend,
            id: &crate::device::DeviceId,
            want: &crate::device::FormatDescriptor,
        ) -> (u32, u32, crate::device::PixelFormat) {
            let req = FormatRequest {
                width: want.width,
                height: want.height,
                pixel_format: want.pixel_format.clone(),
                fps: 30,
            };
            backend.open_device(id, Some(&req)).await.expect("open failed");
            let got = backend
                .current_format(id)
                .await
                .expect("current_format failed")
                .expect("no current format");
            backend.close_device(id).await.expect("close failed");
            eprintln!(
                "requested {}x{} {} -> granted {}x{} {}",
                want.width, want.height, want.pixel_format,
                got.width, got.height, got.pixel_format
            );
            (got.width, got.height, got.pixel_format)
        }

        // Largest first, then smallest. Whatever the camera was left in by a
        // previous run, at least one of these two is a real change.
        let got_large = ask(&backend, id, &large).await;
        let got_small = ask(&backend, id, &small).await;

        assert_ne!(
            got_large, got_small,
            "both requests returned the same mode — the request is not reaching the driver"
        );
        assert_eq!(
            got_large,
            (large.width, large.height, large.pixel_format.clone()),
            "the large mode was advertised and requested but not granted"
        );
        assert_eq!(
            got_small,
            (small.width, small.height, small.pixel_format.clone()),
            "the small mode was advertised and requested but not granted"
        );
    }

    // ---- format negotiation (added 2026-09-02) ---------------------------
    //
    // Until this landed, `capture.pixel_format`, `capture.width` and
    // `capture.height` were honoured by the MOCK backend alone: V4L2 read the
    // driver's current mode and wrote the same struct straight back, and WMF
    // never called SetCurrentMediaType. The knob was validated, passed down,
    // and overwritten. These tests pin the asking, not the getting.

    /// Every `PixelFormat` must survive a round trip through the fourcc map.
    ///
    /// The forward map already existed for *recognising* what a driver hands
    /// back; the inverse is what lets Iris *ask*. If the two disagree, Iris
    /// requests one format and then fails to recognise the identical format
    /// coming back — which reads as a driver fault and is not one.
    #[cfg(target_os = "linux")] // these exercise the V4L2 module, which does not exist on Windows
    #[test]
    fn every_pixel_format_round_trips_through_fourcc() {
        use crate::device::PixelFormat;
        use crate::v4l2_backend::v4l2::{fourcc_to_pixel_format, pixel_format_to_fourcc};
        for f in [
            PixelFormat::Yuyv,
            PixelFormat::Nv12,
            PixelFormat::Rgb24,
            PixelFormat::Bgr24,
            PixelFormat::Mjpeg,
        ] {
            let cc = pixel_format_to_fourcc(&f);
            assert_eq!(
                fourcc_to_pixel_format(cc),
                Some(f.clone()),
                "{f} did not survive the round trip through {cc:#010x}"
            );
        }
    }

    /// The request must actually reach the struct handed to `VIDIOC_S_FMT`.
    /// This is the single line whose absence was the whole defect: the old code
    /// wrote back the mode it had just read.
    #[cfg(target_os = "linux")] // these exercise the V4L2 module, which does not exist on Windows
    #[test]
    fn a_request_is_written_into_the_format_struct() {
        use crate::device::{FormatRequest, PixelFormat};
        use crate::v4l2_backend::v4l2::{pixel_format_to_fourcc, V4l2Format, V4l2UvcBackend};
        let mut fmt: V4l2Format = unsafe { std::mem::zeroed() };
        // Pre-load it with the "adopted" mode, as G_FMT would.
        fmt.pix.width = 1920;
        fmt.pix.height = 1080;
        fmt.pix.pixelformat = pixel_format_to_fourcc(&PixelFormat::Mjpeg);

        let req = FormatRequest {
            width: 640,
            height: 480,
            pixel_format: PixelFormat::Yuyv,
            fps: 30,
        };
        V4l2UvcBackend::apply_request(&mut fmt, &req);

        assert_eq!(fmt.pix.width, 640, "the requested width must be written");
        assert_eq!(fmt.pix.height, 480, "the requested height must be written");
        assert_eq!(
            fmt.pix.pixelformat,
            pixel_format_to_fourcc(&PixelFormat::Yuyv),
            "the requested pixel format must be written, not the adopted one"
        );
    }

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
        let _camera = camera_guard();
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
        // Controls changed contract on 2026-09-01 and this assertion changed
        // with them, deliberately.
        //
        // They used to require `open_device` first, which forced a control
        // service to hold the camera — and on V4L2 opening performs
        // `VIDIOC_S_FMT`, which a second handle cannot do while the capture
        // path negotiates its own format. That produced
        // `VIDIOC_S_FMT: Device or resource busy` and a dead preview.
        // Control ioctls need no streaming setup, so they now open the node
        // transiently when the backend does not already hold it.
        //
        // For a path that does not exist there is nothing to open, so the
        // honest answer is DeviceNotFound — still a real answer, which is what
        // this test exists to prove, and still never NotImplemented.
        assert!(
            matches!(
                backend.get_control(&id, 0x0098_0900).await,
                Err(HalError::DeviceNotFound)
            ),
            "get_control on a nonexistent device must be DeviceNotFound"
        );
        assert!(
            matches!(
                backend.set_control(&id, 0x0098_0900, 1).await,
                Err(HalError::DeviceNotFound)
            ),
            "set_control on a nonexistent device must be DeviceNotFound"
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
        let _camera = camera_guard();
        use crate::backend::UvcBackend as _;
        use crate::device::DeviceId;
        use crate::error::HalError;
        use crate::v4l2_backend::v4l2::V4l2UvcBackend;

        let backend = V4l2UvcBackend::new();
        let id = DeviceId("/dev/null".into());
        assert!(backend.open_device(&id, None).await.is_err(), "/dev/null is not a capture device");
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
        let _camera = camera_guard();
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

        backend.open_device(&id, None).await.expect("open_device failed");

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
                backend.open_device(&id, None).await,
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
