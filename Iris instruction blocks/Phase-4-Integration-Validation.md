================================================================================
PHASE 4 — INTEGRATION TESTING & REAL-WORLD VALIDATION
Baxter's Screen Record — Agent 2 Execution Block
================================================================================

PHASE:          4 of 4
MODULES:        All (A through F)
CRATES:         All 7 crates, plus workspace-level integration tests
DEPENDS ON:     Phases 1-3 complete (real DXGI capture, wired pipeline,
                real named pipe IPC)
PRIOR STATE:    All individual components working. Pipeline wired. IPC real.
                Need to verify end-to-end that the whole system actually
                produces valid recordings, handles errors, and performs
                well enough for real use.

================================================================================
PURPOSE
================================================================================

Validate that Baxter's Screen Record works as a real product:

  1. End-to-end recording: capture real screen → H.264 encode → MP4 file
  2. The output MP4 is playable
  3. Performance is acceptable (stable FPS, bounded memory)
  4. Error conditions are handled gracefully
  5. UI → pipeline → IPC integration works
  6. Duration recording works (record for N seconds, verify)
  7. Named pipe IPC works cross-process

This phase produces NO new features — only tests and validation. If any
test fails, fix the relevant Phase 1-3 code before proceeding.

================================================================================
TEST CATEGORIES
================================================================================

All tests below should be in a new file:
    tests/integration_tests.rs   (workspace root tests/ directory)

Or split across:
    tests/e2e_recording.rs
    tests/ipc_integration.rs
    tests/performance.rs
    tests/error_recovery.rs

All hardware tests gated behind BSR_USE_HW=1 environment variable.

================================================================================
TEST 1: SMOKE TEST — Record 3 Seconds
================================================================================

    /// End-to-end: capture real screen, encode H.264, mux MP4, verify file.
    #[cfg(windows)]
    #[tokio::test]
    async fn smoke_test_record_3_seconds() {
        if std::env::var("BSR_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping (set BSR_USE_HW=1)");
            return;
        }

        let output_dir = tempfile::tempdir().unwrap();
        let config = BsrConfig {
            capture: CaptureSubConfig { width: 1920, height: 1080, fps: 30 },
            encode: EncodeSubConfig {
                preset: "medium".into(),
                bitrate_kbps: 8000,
            },
            output: OutputSubConfig {
                directory: output_dir.path().to_string_lossy().into(),
                naming_strategy: FileNamingStrategy::Timestamp,
                max_duration_secs: None,
            },
        };

        // Start pipeline
        let mut pipeline = RecordingPipeline::start(&config).await.unwrap();

        // Record for 3 seconds
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        // Stop
        pipeline.stop().await.unwrap();

        // Find the output MP4
        let files: Vec<_> = std::fs::read_dir(output_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "mp4").unwrap_or(false))
            .collect();

        assert_eq!(files.len(), 1, "Expected exactly 1 MP4 file");

        let mp4_path = files[0].path();
        let metadata = std::fs::metadata(&mp4_path).unwrap();
        eprintln!("Output: {} ({} bytes)", mp4_path.display(), metadata.len());

        // A 3-second 1080p H.264 video should be at least 100KB
        assert!(metadata.len() > 100_000,
            "MP4 too small: {} bytes", metadata.len());

        // Verify MP4 header (starts with ftyp box)
        let mut header = [0u8; 8];
        let mut f = std::fs::File::open(&mp4_path).unwrap();
        std::io::Read::read_exact(&mut f, &mut header).unwrap();
        // ftyp box: bytes 4-7 should be "ftyp"
        assert_eq!(&header[4..8], b"ftyp",
            "Not a valid MP4 file (missing ftyp)");
    }

================================================================================
TEST 2: DURATION CAP — Max Recording Duration
================================================================================

    /// Verify that max_duration_secs causes automatic stop.
    #[cfg(windows)]
    #[tokio::test]
    async fn test_duration_cap_5_seconds() {
        if std::env::var("BSR_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping (set BSR_USE_HW=1)");
            return;
        }

        let output_dir = tempfile::tempdir().unwrap();
        let config = BsrConfig {
            // ... standard config ...
            output: OutputSubConfig {
                directory: output_dir.path().to_string_lossy().into(),
                naming_strategy: FileNamingStrategy::Timestamp,
                max_duration_secs: Some(5), // auto-stop after 5 sec
            },
            // ... rest ...
        };

        let mut pipeline = RecordingPipeline::start(&config).await.unwrap();

        // Wait up to 10 seconds — pipeline should auto-stop at ~5
        let start = std::time::Instant::now();
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;

        // Pipeline should have auto-stopped, but call stop to be safe
        pipeline.stop().await.ok();

        // Verify file exists and is reasonable for 5s recording
        let files: Vec<_> = std::fs::read_dir(output_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "mp4").unwrap_or(false))
            .collect();

        assert_eq!(files.len(), 1);
        let size = std::fs::metadata(files[0].path()).unwrap().len();
        eprintln!("5s recording: {} bytes", size);

        // Should be roughly 5 seconds of video (~3-5MB at 8Mbps)
        assert!(size > 200_000, "Too small for 5s recording");
    }

================================================================================
TEST 3: START/STOP/START — Pipeline Reuse
================================================================================

    /// Verify we can record, stop, and record again without crashing.
    #[cfg(windows)]
    #[tokio::test]
    async fn test_start_stop_start() {
        if std::env::var("BSR_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping (set BSR_USE_HW=1)");
            return;
        }

        let output_dir = tempfile::tempdir().unwrap();

        // First recording
        let config1 = make_test_config(output_dir.path());
        let mut pipeline1 = RecordingPipeline::start(&config1).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        pipeline1.stop().await.unwrap();

        // Brief pause between recordings
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Second recording
        let config2 = make_test_config(output_dir.path());
        let mut pipeline2 = RecordingPipeline::start(&config2).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        pipeline2.stop().await.unwrap();

        // Should have 2 MP4 files
        let mp4_count = std::fs::read_dir(output_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "mp4").unwrap_or(false))
            .count();

        assert_eq!(mp4_count, 2, "Expected 2 MP4 files from 2 recordings");
    }

================================================================================
TEST 4: NAMED PIPE IPC — Cross-Process Control
================================================================================

    /// Verify IPC command pipe can start/stop recording.
    #[cfg(windows)]
    #[tokio::test]
    async fn test_ipc_start_stop_recording() {
        if std::env::var("BSR_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping (set BSR_USE_HW=1)");
            return;
        }

        // Start the command-handling service with a real pipeline behind it
        let (cmd_tx, mut cmd_rx) = mpsc::channel(4);
        let (telemetry_tx, _) = broadcast::channel(64);

        let mut server = bsr_ipc::named_pipe::PipeServer::start(
            cmd_tx, telemetry_tx.clone(),
        ).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let client = bsr_ipc::named_pipe::PipeClient::new();

        // Send StartRecording
        let resp = client.send_command(&IpcCommand::StartRecording).await.unwrap();
        assert!(matches!(resp, IpcResponse::Ok));

        // Verify server received it
        let cmd = cmd_rx.recv().await.unwrap();
        assert!(matches!(cmd, IpcCommand::StartRecording));

        // Send StopRecording
        let resp = client.send_command(&IpcCommand::StopRecording).await.unwrap();
        assert!(matches!(resp, IpcResponse::Ok));

        let cmd = cmd_rx.recv().await.unwrap();
        assert!(matches!(cmd, IpcCommand::StopRecording));

        server.stop().await;
    }

================================================================================
TEST 5: TELEMETRY FLOW — End-to-End
================================================================================

    /// Verify telemetry events flow from pipeline through IPC to client.
    #[cfg(windows)]
    #[tokio::test]
    async fn test_telemetry_through_pipe() {
        if std::env::var("BSR_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping (set BSR_USE_HW=1)");
            return;
        }

        let (cmd_tx, _) = mpsc::channel(4);
        let (telemetry_tx, _) = broadcast::channel(64);

        let mut server = bsr_ipc::named_pipe::PipeServer::start(
            cmd_tx, telemetry_tx.clone(),
        ).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let client = bsr_ipc::named_pipe::PipeClient::new();
        let mut tele_rx = client.subscribe_telemetry().await.unwrap();

        // Wait for subscription
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Broadcast 3 telemetry events from the "pipeline"
        for i in 0..3 {
            telemetry_tx.send(TelemetryEvent::FramesCaptured(i * 30)).ok();
        }

        // Client should receive all 3
        for i in 0..3 {
            let event = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                tele_rx.recv(),
            ).await.unwrap().unwrap();

            assert!(matches!(event,
                TelemetryEvent::FramesCaptured(n) if n == i * 30));
        }

        server.stop().await;
    }

================================================================================
TEST 6: PERFORMANCE — FPS STABILITY
================================================================================

    /// Verify capture maintains target FPS (within 10% tolerance).
    #[cfg(windows)]
    #[tokio::test]
    async fn test_fps_stability_30fps() {
        if std::env::var("BSR_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping (set BSR_USE_HW=1)");
            return;
        }

        let mut backend = bsr_capture::DxgiCaptureBackend::new();
        backend.initialize().await.unwrap();

        let start = std::time::Instant::now();
        let mut frame_count = 0u32;

        // Capture for 2 seconds
        while start.elapsed() < std::time::Duration::from_secs(2) {
            match backend.capture_frame().await {
                Ok(_) => frame_count += 1,
                Err(e) => eprintln!("Frame drop: {e:?}"),
            }
            // Pace at 30fps
            tokio::time::sleep(std::time::Duration::from_millis(33)).await;
        }

        backend.shutdown().await.unwrap();

        let elapsed = start.elapsed().as_secs_f64();
        let actual_fps = frame_count as f64 / elapsed;
        eprintln!("Captured {} frames in {:.2}s = {:.1} FPS",
                  frame_count, elapsed, actual_fps);

        // Should be within 10% of 30fps (27-33)
        assert!(actual_fps > 27.0, "FPS too low: {actual_fps:.1}");
        assert!(actual_fps < 33.0, "FPS too high: {actual_fps:.1}");
    }

================================================================================
TEST 7: MEMORY — No Unbounded Growth
================================================================================

    /// Verify memory doesn't grow unboundedly during recording.
    #[cfg(windows)]
    #[tokio::test]
    async fn test_memory_bounded() {
        if std::env::var("BSR_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping (set BSR_USE_HW=1)");
            return;
        }

        // Record for 5 seconds, check that RSS doesn't grow more than
        // 200MB above baseline.
        // Use winapi GetProcessMemoryInfo or sysinfo crate.

        let output_dir = tempfile::tempdir().unwrap();
        let config = make_test_config(output_dir.path());

        let baseline = get_rss_mb();
        eprintln!("Baseline RSS: {baseline} MB");

        let mut pipeline = RecordingPipeline::start(&config).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        let peak = get_rss_mb();
        eprintln!("Peak RSS: {peak} MB");

        pipeline.stop().await.unwrap();

        let growth = peak - baseline;
        eprintln!("Memory growth: {growth} MB");

        assert!(growth < 200, "Memory grew by {growth} MB (limit: 200 MB)");
    }

    fn get_rss_mb() -> u64 {
        // Use windows API: GetProcessMemoryInfo
        // Or simpler: use sysinfo crate
        // Implementation depends on what's available
        // Placeholder:
        0
    }

NOTE: If sysinfo crate is not in the workspace, use the Windows API
directly via the `windows` crate: GetProcessMemoryInfo → WorkingSetSize.
Or add sysinfo as a dev-dependency for test convenience.

================================================================================
TEST 8: ERROR RECOVERY — Capture Failure
================================================================================

    /// Verify pipeline handles capture failure without crashing.
    #[tokio::test]
    async fn test_capture_error_handled() {
        // Use a mock backend that fails after 10 frames
        let mut backend = FailingMockBackend { frames_until_fail: 10 };
        // Start pipeline with this backend
        // Verify encoder and muxer shut down gracefully
        // Verify no panic, no hang
    }

    struct FailingMockBackend {
        frames_until_fail: u32,
    }

    #[async_trait::async_trait]
    impl CaptureBackend for FailingMockBackend {
        async fn initialize(&mut self) -> Result<(), CaptureError> { Ok(()) }

        async fn capture_frame(&mut self) -> Result<CaptureFrame, CaptureError> {
            if self.frames_until_fail == 0 {
                return Err(CaptureError::FrameAcquisitionFailed(
                    "Simulated failure".into()));
            }
            self.frames_until_fail -= 1;
            Ok(CaptureFrame {
                data: vec![0u8; 1920 * 1080 * 4],
                timestamp: 0,
                width: 1920, height: 1080,
                format: FrameFormat::Bgra8,
            })
        }

        async fn shutdown(&mut self) -> Result<(), CaptureError> { Ok(()) }
    }

================================================================================
TEST 9: OUTPUT VERIFICATION — Valid MP4 Structure
================================================================================

    /// Check the MP4 structure has proper atoms/boxes.
    #[cfg(windows)]
    #[tokio::test]
    async fn test_mp4_has_moov_atom() {
        if std::env::var("BSR_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping (set BSR_USE_HW=1)");
            return;
        }

        let output_dir = tempfile::tempdir().unwrap();
        let config = make_test_config(output_dir.path());

        let mut pipeline = RecordingPipeline::start(&config).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        pipeline.stop().await.unwrap();

        let mp4_path = find_mp4_in(output_dir.path());
        let data = std::fs::read(&mp4_path).unwrap();

        // Search for 'moov' box (bytes: size + "moov")
        let has_moov = data.windows(4).any(|w| w == b"moov");
        assert!(has_moov, "MP4 missing moov atom — file may be corrupt");

        // Search for 'mdat' box (media data)
        let has_mdat = data.windows(4).any(|w| w == b"mdat");
        assert!(has_mdat, "MP4 missing mdat atom — no media data");
    }

================================================================================
HELPER FUNCTIONS
================================================================================

    fn make_test_config(output_dir: &std::path::Path) -> BsrConfig {
        BsrConfig {
            capture: CaptureSubConfig {
                width: 1920,
                height: 1080,
                fps: 30,
            },
            encode: EncodeSubConfig {
                preset: "medium".into(),
                bitrate_kbps: 8000,
            },
            output: OutputSubConfig {
                directory: output_dir.to_string_lossy().into(),
                naming_strategy: FileNamingStrategy::Timestamp,
                max_duration_secs: None,
            },
        }
    }

    fn find_mp4_in(dir: &std::path::Path) -> std::path::PathBuf {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.path().extension().map(|x| x == "mp4").unwrap_or(false))
            .expect("No MP4 file found in output directory")
            .path()
    }

================================================================================
DEPENDENCIES TO ADD (dev-dependencies in workspace Cargo.toml)
================================================================================

    [dev-dependencies]
    tempfile = "3"

tempfile is used for creating temporary output directories in tests.
It auto-cleans on drop, preventing test file accumulation.

================================================================================
TEST EXECUTION ORDER
================================================================================

Run tests in this order to isolate failures:

  1. Tests 1-3 (recording basics) — validates pipeline works at all
  2. Tests 4-5 (IPC) — validates named pipe control
  3. Test 6 (FPS) — validates performance
  4. Test 7 (memory) — validates resource management
  5. Tests 8-9 (error + structure) — validates robustness

If Test 1 fails, do NOT proceed to other tests — fix the pipeline first.

================================================================================
ACCEPTANCE CRITERIA
================================================================================

1.  cargo test --workspace compiles clean (all phases integrated)
2.  All existing tests pass (≥34 original + Phase 1-3 additions)
3.  With BSR_USE_HW=1:
      a. smoke_test_record_3_seconds PASSES
      b. Output MP4 has valid ftyp and moov atoms
      c. MP4 file size is reasonable for duration
4.  test_start_stop_start produces 2 separate MP4 files
5.  test_ipc_start_stop_recording PASSES (named pipe command round-trip)
6.  test_telemetry_through_pipe PASSES (streaming telemetry over pipe)
7.  test_fps_stability_30fps maintains 27-33 FPS
8.  test_memory_bounded stays under 200MB growth over 5 seconds
9.  test_capture_error_handled exits cleanly (no panic, no hang)
10. test_mp4_has_moov_atom confirms valid container structure

================================================================================
WHAT "DONE" LOOKS LIKE
================================================================================

When all 4 phases are complete and all tests pass:

  1. User launches bsr-ui (the egui app)
  2. Clicks "Record" → screen capture starts, H.264 encoding begins, MP4
     file is being written in real-time
  3. Telemetry (FPS, frame count, file size) updates live in the UI
  4. Clicks "Stop" → pipeline shuts down gracefully, MP4 finalized
  5. The .mp4 file plays in VLC/Windows Media Player
  6. User can record again without restarting the app
  7. A separate process can connect via named pipe to control recording

This is a working, real-world screen recorder.

================================================================================
NOTES FOR BUILDER AGENT
================================================================================

- All hardware tests must be gated behind BSR_USE_HW=1. Never make a
  test that requires a real display run unconditionally — CI won't have one.
- Use tempfile for output directories. Never write to the user's real
  Videos folder in tests.
- The test code above is ILLUSTRATIVE. Adjust types, field names, and
  constructors to match what actually exists after Phases 1-3. Read the
  actual source before writing tests.
- If a test fails, fix the Phase 1-3 code first, then re-run. Tests in
  this phase are meant to CATCH problems, not paper over them.
- For get_rss_mb(), either add `sysinfo` as a dev-dep or use the Windows
  API directly. If neither is practical, skip the memory test and note it
  as a manual verification step.
- Consider running the FPS test multiple times — a single run can be
  affected by system load. 3 consecutive passes is a good bar.
- Run cargo test --workspace after every change.

================================================================================
VERIFICATION COMMANDS
================================================================================

    cd '<screen-recorder-project-root>'
    $env:VCPKG_ROOT = "C:\tools\vcpkg"
    $env:LIBCLANG_PATH = "C:\tools\LLVM\bin"

    # Full workspace (all tests, no hardware)
    cargo test --workspace

    # Hardware integration tests
    $env:BSR_USE_HW = "1"
    cargo test --workspace -- --nocapture

    # Specific test
    cargo test smoke_test_record_3_seconds -- --nocapture
    cargo test test_ipc_start_stop_recording -- --nocapture
    cargo test test_fps_stability_30fps -- --nocapture

================================================================================
COMMIT MESSAGE
================================================================================

    Phase-4: Integration tests — e2e recording, IPC, FPS, memory, MP4 validation

================================================================================
END OF PHASE 4
================================================================================
