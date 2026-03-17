================================================================================
PHASE 2 — PIPELINE WIRING: CAPTURE → ENCODE → MUX
Baxter's Screen Record — Agent 2 Execution Block
================================================================================

PHASE:          2 of 4
MODULES:        B (Capture), C (Encode), D (Mux), E (IPC), F (UI)
CRATES:         bsr-capture, bsr-encode, bsr-mux, bsr-ipc, bsr-core, bsr-ui
DEPENDS ON:     Phase 1 complete (real DXGI capture working)
PRIOR STATE:    All services exist with typed channel interfaces but NOTHING
                connects them. CaptureService has "TODO: Send frame to encoder".
                EncoderService takes Receiver<CaptureFrame> + Sender<EncodedPacket>.
                MuxerService takes Receiver<EncodedPacket>.
                EncodedPacket type exists in BOTH bsr-encode and bsr-mux with
                different fields (type mismatch). UI uses dummy channels.

================================================================================
PURPOSE
================================================================================

Wire the three service stages into a live recording pipeline:

    CaptureService ──CaptureFrame──► EncoderService ──EncodedPacket──► MuxerService
                                                                          │
    UI ◄──TelemetryEvent──► IPC ◄──MuxerCommand──────────────────────────┘

When this phase is complete, pressing "Record" in the UI will:
  1. Start screen capture
  2. Feed BGRA8 frames to the H.264 encoder
  3. Feed encoded packets to the MP4 muxer
  4. Write a playable .mp4 file
  5. Report telemetry (FPS, frame count, duration) to the UI

================================================================================
CRITICAL PREREQUISITE: RESOLVE EncodedPacket TYPE MISMATCH
================================================================================

CURRENT PROBLEM:

  bsr-encode defines:
    pub struct EncodedPacket {
        pub data: Vec<u8>,
        pub timestamp: u64,
        pub pts: i64,
        pub dts: i64,
        pub keyframe: bool,
        pub codec: String,
    }

  bsr-mux defines its OWN:
    pub struct EncodedPacket {
        pub data: Vec<u8>,
        pub pts: i64,
        pub dts: i64,
        pub keyframe: bool,
    }
    (no codec, no timestamp)

These are two separate types. The channel from encoder → muxer cannot
carry both.

SOLUTION: Unify on a single canonical EncodedPacket.

Option A (RECOMMENDED): Move EncodedPacket into bsr-core as the single
source of truth. Both bsr-encode and bsr-mux import from bsr-core.

    // bsr-core/src/lib.rs — add:
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    pub struct EncodedPacket {
        pub data: Vec<u8>,
        pub pts: i64,
        pub dts: i64,
        pub keyframe: bool,
        pub codec: String,
    }

Then:
  - bsr-encode: remove local EncodedPacket, use bsr_core::EncodedPacket
  - bsr-mux: remove local EncodedPacket, use bsr_core::EncodedPacket
  - Update all `use` statements, constructor callsites, field accesses
  - cargo check --workspace to verify

Option B: Keep EncodedPacket in bsr-encode, add bsr-encode as a dependency
of bsr-mux, and import from there. Less clean but fewer moves.

→ Go with Option A. bsr-core already has no upstream crate deps and is
  the natural home for shared types.

  Also move CaptureFrame into bsr-core for consistency (it's currently
  in bsr-capture but bsr-encode already uses it via bsr-capture dep).
  Moving to bsr-core removes the need for bsr-encode → bsr-capture dep.

================================================================================
STEP-BY-STEP IMPLEMENTATION
================================================================================

STEP 1: Centralize shared types in bsr-core
---------------------------------------------

Move to bsr-core/src/lib.rs (add alongside existing BsrConfig, AppState):

    /// A single captured screen frame.
    #[derive(Debug, Clone)]
    pub struct CaptureFrame {
        pub data: Vec<u8>,
        pub timestamp: u64,
        pub width: u32,
        pub height: u32,
        pub format: FrameFormat,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum FrameFormat {
        Bgra8,
        Rgba8,
    }

    /// An encoded video packet (H.264 NAL unit(s)).
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    pub struct EncodedPacket {
        pub data: Vec<u8>,
        pub pts: i64,
        pub dts: i64,
        pub keyframe: bool,
        pub codec: String,
    }

Then in bsr-capture/src/lib.rs:
  - Remove CaptureFrame and FrameFormat definitions
  - Add: pub use bsr_core::{CaptureFrame, FrameFormat};  (re-export so
    existing downstream code doesn't break)

In bsr-encode/src/lib.rs:
  - Remove local EncodedPacket definition
  - Replace with: use bsr_core::EncodedPacket;
  - Ensure bsr-encode/Cargo.toml has: bsr-core = { path = "../bsr-core" }
    (it may already have this)
  - Update H264EncoderBackend::encode_frame() to include codec: "h264"

In bsr-mux/src/lib.rs:
  - Remove local EncodedPacket definition
  - Replace with: use bsr_core::EncodedPacket;
  - Update Cargo.toml if needed

Verify: cargo check --workspace

STEP 2: Add frame sender to CaptureService
--------------------------------------------

Currently CaptureService::run() captures frames but has no output channel.

MODIFY CaptureService to accept an mpsc::Sender<CaptureFrame>:

    pub struct CaptureService<B: CaptureBackend> {
        backend: B,
        config: CaptureConfig,
        telemetry_tx: broadcast::Sender<TelemetryEvent>,
        shutdown_rx: mpsc::Receiver<()>,
        frame_tx: mpsc::Sender<CaptureFrame>,   // ← ADD THIS
    }

    impl<B: CaptureBackend> CaptureService<B> {
        pub fn new(
            backend: B,
            config: CaptureConfig,
            telemetry_tx: broadcast::Sender<TelemetryEvent>,
            shutdown_rx: mpsc::Receiver<()>,
            frame_tx: mpsc::Sender<CaptureFrame>,   // ← ADD THIS
        ) -> Self { ... }
    }

In the run() loop, replace the "TODO: Send frame to encoder" with:

    match self.backend.capture_frame().await {
        Ok(frame) => {
            if self.frame_tx.send(frame).await.is_err() {
                tracing::warn!("Frame receiver dropped, stopping capture");
                break;
            }
            telemetry.frames_captured += 1;
        }
        Err(e) => {
            tracing::warn!("Capture failed: {e:?}");
            telemetry.frames_dropped += 1;
        }
    }

Update the existing tests to supply a frame_tx channel (mpsc::channel(4)).

STEP 3: Create RecordingPipeline orchestrator
-----------------------------------------------

Create a new module in bsr-core or a new file crates/bsr-core/src/pipeline.rs

    pub struct RecordingPipeline {
        /// Join handle for capture task
        capture_handle: Option<tokio::task::JoinHandle<()>>,
        /// Join handle for encoder task
        encode_handle: Option<tokio::task::JoinHandle<()>>,
        /// Join handle for muxer task
        mux_handle: Option<tokio::task::JoinHandle<()>>,
        /// Send () to trigger capture shutdown
        capture_shutdown_tx: mpsc::Sender<()>,
        /// Send MuxerCommand::Stop to stop muxer
        muxer_cmd_tx: mpsc::Sender<MuxerCommand>,
        /// Receive telemetry from all stages
        telemetry_rx: broadcast::Receiver<TelemetryEvent>,
        /// Pipeline state
        state: PipelineState,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PipelineState {
        Idle,
        Starting,
        Recording,
        Stopping,
    }

    impl RecordingPipeline {
        /// Create and launch the full capture → encode → mux pipeline.
        pub async fn start(config: &BsrConfig) -> Result<Self, PipelineError> {
            // 1. Create channels
            let (frame_tx, frame_rx) = mpsc::channel::<CaptureFrame>(8);
            let (packet_tx, packet_rx) = mpsc::channel::<EncodedPacket>(16);
            let (telemetry_tx, telemetry_rx) = broadcast::channel::<TelemetryEvent>(64);
            let (capture_shutdown_tx, capture_shutdown_rx) = mpsc::channel::<()>(1);
            let (muxer_cmd_tx, muxer_cmd_rx) = mpsc::channel::<MuxerCommand>(4);
            let (muxer_telemetry_tx, _muxer_telemetry_rx) =
                mpsc::channel::<MuxerTelemetry>(16);

            // 2. Instantiate backends
            #[cfg(windows)]
            let capture_backend = bsr_capture::DxgiCaptureBackend::new();
            #[cfg(not(windows))]
            let capture_backend = bsr_capture::MockCaptureBackend::new();

            let capture_config = CaptureConfig {
                width: config.capture.width,
                height: config.capture.height,
                fps: config.capture.fps,
                drop_policy: /* from config or default */
            };

            let encoder_config = bsr_encode::EncoderConfig {
                codec: "h264".into(),
                preset: config.encode.preset.clone(),
                bitrate_kbps: config.encode.bitrate_kbps,
                width: config.capture.width,
                height: config.capture.height,
                fps: config.capture.fps,
            };

            // Create a mock IPC client for muxer (or wire to real one)
            let (ipc_server, ipc_client) = bsr_ipc::IpcServer::new_pair();

            let muxer_config = bsr_ipc::MuxerConfig {
                output_dir: config.output.directory.clone(),
                naming_strategy: config.output.naming_strategy.clone(),
                max_duration_secs: config.output.max_duration_secs,
            };

            // 3. Build services
            let capture_svc = bsr_capture::CaptureService::new(
                capture_backend, capture_config,
                telemetry_tx.clone(), capture_shutdown_rx, frame_tx,
            );

            let encoder_backend = bsr_encode::H264EncoderBackend::new();
            let encoder_svc = bsr_encode::EncoderService::new(
                encoder_backend, encoder_config,
                frame_rx, packet_tx,
            );

            let muxer_backend = bsr_mux::Mp4Muxer::new();
            let muxer_svc = bsr_mux::MuxerService::new(
                muxer_backend, muxer_config,
                packet_rx, muxer_telemetry_tx, muxer_cmd_rx, ipc_client,
            );

            // 4. Spawn service tasks
            let capture_handle = tokio::spawn(async move {
                if let Err(e) = capture_svc.run().await {
                    tracing::error!("Capture service error: {e:?}");
                }
            });

            let encode_handle = tokio::spawn(async move {
                if let Err(e) = encoder_svc.run().await {
                    tracing::error!("Encoder service error: {e:?}");
                }
            });

            let mux_handle = tokio::spawn(async move {
                if let Err(e) = muxer_svc.run().await {
                    tracing::error!("Muxer service error: {e:?}");
                }
            });

            Ok(Self {
                capture_handle: Some(capture_handle),
                encode_handle: Some(encode_handle),
                mux_handle: Some(mux_handle),
                capture_shutdown_tx,
                muxer_cmd_tx,
                telemetry_rx,
                state: PipelineState::Recording,
            })
        }

        /// Stop the pipeline gracefully.
        pub async fn stop(&mut self) -> Result<(), PipelineError> {
            self.state = PipelineState::Stopping;

            // 1. Stop capture → frame_tx drops → encoder sees closed channel
            let _ = self.capture_shutdown_tx.send(()).await;

            // 2. Wait for capture to finish
            if let Some(h) = self.capture_handle.take() {
                let _ = h.await;
            }

            // frame_tx is now dropped (capture ended), so frame_rx in encoder
            // will return None, causing it to flush and exit.

            // 3. Wait for encoder to finish
            if let Some(h) = self.encode_handle.take() {
                let _ = h.await;
            }

            // packet_tx is now dropped (encoder ended), so packet_rx in muxer
            // will return None, causing it to finalize and exit.

            // 4. Send explicit stop to muxer just in case
            let _ = self.muxer_cmd_tx.send(MuxerCommand::Stop).await;

            // 5. Wait for muxer to finish
            if let Some(h) = self.mux_handle.take() {
                let _ = h.await;
            }

            self.state = PipelineState::Idle;
            tracing::info!("Recording pipeline stopped");
            Ok(())
        }
    }

NOTE: Adjust service constructors and run() signatures as needed to match
what actually exists. The above is the TARGET structure — adapt field names
and constructors to the actual code. The key contract is:

  CaptureService.frame_tx ──→ EncoderService.frame_rx
  EncoderService.packet_tx ──→ MuxerService.packet_rx

STEP 4: Wire UI to real pipeline
----------------------------------

In bsr-ui, the AppWindow currently uses dummy channels. Replace with:

    pub struct AppWindow {
        pipeline: Option<RecordingPipeline>,
        telemetry_rx: Option<broadcast::Receiver<TelemetryEvent>>,
        // ... existing fields ...
    }

On "Record" button click:
    let pipeline = RecordingPipeline::start(&self.config).await?;
    self.telemetry_rx = Some(pipeline.telemetry_rx.resubscribe());
    self.pipeline = Some(pipeline);

On "Stop" button click:
    if let Some(ref mut pipeline) = self.pipeline {
        pipeline.stop().await?;
    }
    self.pipeline = None;

NOTE: eframe's update() is synchronous. Use a tokio runtime handle or
spawn_blocking bridge. The existing bsr-ui likely already has a tokio
handle or a channel-based command pattern — follow the existing pattern.

STEP 5: Ensure encoder handles channel closure gracefully
-----------------------------------------------------------

In bsr-encode's EncoderService::run(), the loop should look like:

    while let Some(frame) = self.frame_rx.recv().await {
        match self.backend.encode_frame(&frame).await {
            Ok(Some(packet)) => {
                if self.packet_tx.send(packet).await.is_err() {
                    tracing::warn!("Packet receiver dropped");
                    break;
                }
            }
            Ok(None) => { /* MFT buffering, no output yet */ }
            Err(e) => tracing::error!("Encode error: {e:?}"),
        }
    }
    // Channel closed → flush remaining packets
    self.backend.shutdown().await.ok();

STEP 6: Ensure muxer handles channel closure gracefully
---------------------------------------------------------

In bsr-mux's MuxerService::run(), use tokio::select! between:
  - packet_rx.recv() → write_packet
  - muxer_cmd_rx.recv() → handle Stop/etc
  - break when packet_rx returns None (channel closed)

On exit, call self.backend.finalize() then self.backend.shutdown().

STEP 7: Add BsrConfig sub-configs if missing
----------------------------------------------

bsr-core's BsrConfig may not yet have capture/encode/output sub-sections.
Add as needed:

    pub struct CaptureSubConfig {
        pub width: u32,
        pub height: u32,
        pub fps: u32,
    }

    pub struct EncodeSubConfig {
        pub preset: String,
        pub bitrate_kbps: u32,
    }

    pub struct OutputSubConfig {
        pub directory: String,
        pub naming_strategy: FileNamingStrategy,
        pub max_duration_secs: Option<u64>,
    }

These should be Serialize + Deserialize + Default. Provide sensible
defaults (1920x1080, 30fps, medium preset, 8000 kbps, user's Videos
folder, timestamp naming).

================================================================================
FILE CHANGES SUMMARY
================================================================================

    MODIFY  crates/bsr-core/src/lib.rs        Add CaptureFrame, FrameFormat,
                                               EncodedPacket, sub-configs
    CREATE  crates/bsr-core/src/pipeline.rs    RecordingPipeline orchestrator
    MODIFY  crates/bsr-capture/src/lib.rs      Re-export from core, add frame_tx
    MODIFY  crates/bsr-encode/src/lib.rs       Use core::EncodedPacket
    MODIFY  crates/bsr-mux/src/lib.rs          Use core::EncodedPacket
    MODIFY  crates/bsr-ui/src/lib.rs           Wire AppWindow to real pipeline
    MODIFY  crates/bsr-*/Cargo.toml            Update deps as needed

================================================================================
ACCEPTANCE CRITERIA
================================================================================

1.  cargo check --workspace compiles clean
2.  cargo test --workspace — all existing tests still pass (≥34)
3.  With BSR_USE_HW=1:
      a. RecordingPipeline::start() succeeds
      b. Record for 3 seconds, then stop()
      c. An .mp4 file exists in the output directory
      d. The file is >0 bytes and has valid MP4 headers
4.  EncodedPacket is defined in exactly ONE place (bsr-core)
5.  CaptureService sends real frames to the encoder channel
6.  Pipeline shutdown is graceful (capture → encode → mux, in order)
7.  UI "Record" button triggers pipeline start
8.  UI "Stop" button triggers pipeline stop

================================================================================
GRACEFUL SHUTDOWN SEQUENCE (critical)
================================================================================

    1. User clicks Stop  (or IPC StopRecording arrives)
    2. capture_shutdown_tx sends ()
    3. CaptureService loop exits → frame_tx is dropped
    4. EncoderService sees frame_rx return None → flushes MFT → packet_tx drops
    5. MuxerService sees packet_rx return None → calls finalize() → writes MP4
                                                                    trailer
    6. All three JoinHandles resolve

This cascading shutdown ensures no data is lost — the MP4 trailer is
properly written because the muxer always finalizes before exiting.

================================================================================
CHANNEL BUFFER SIZES
================================================================================

    frame_tx/frame_rx:    8 frames  (at 30fps, ~250ms buffer; prevents
                          blocking capture if encoder is temporarily slow)
    packet_tx/packet_rx: 16 packets (encoder outputs fewer packets than
                          input frames due to B-frames and buffering)
    telemetry broadcast: 64 events  (UI polls periodically, events can be
                          dropped without harm)

These can be tuned later. Start with these values.

================================================================================
NOTES FOR BUILDER AGENT
================================================================================

- Read the actual field names in each service's constructor before writing
  code — the examples above use expected names but the real code may differ
  slightly.
- The EncoderService and MuxerService may already handle channel closure;
  verify before adding duplicate logic.
- The MuxerService likely already uses tokio::select! — check its run()
  method before modifying.
- If bsr-core doesn't have a pipeline.rs yet, create it and add
  `pub mod pipeline;` to lib.rs.
- For the UI → pipeline bridge, if eframe update() is sync, use a
  oneshot channel or Arc<Mutex<Option<RecordingPipeline>>> pattern. Don't
  block the UI thread.
- Run cargo test --workspace after EVERY file change to catch regressions
  immediately.

================================================================================
VERIFICATION COMMANDS
================================================================================

    cd 'C:\Users\Baxter\Desktop\Baxters Screen Record\Baxters Screen Record'
    $env:VCPKG_ROOT = "C:\tools\vcpkg"
    $env:LIBCLANG_PATH = "C:\tools\LLVM\bin"

    cargo check --workspace
    cargo test --workspace

    # End-to-end hardware test
    $env:BSR_USE_HW = "1"
    cargo test -p bsr-core pipeline_smoke_test -- --nocapture

================================================================================
COMMIT MESSAGE
================================================================================

    Phase-2: Wire capture→encode→mux pipeline, unify EncodedPacket, add orchestrator

================================================================================
END OF PHASE 2
================================================================================
