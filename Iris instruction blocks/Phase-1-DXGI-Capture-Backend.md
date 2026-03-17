================================================================================
PHASE 1 — REAL DXGI SCREEN CAPTURE BACKEND
Baxter's Screen Record — Agent 2 Execution Block
================================================================================

PHASE:          1 of 4
MODULE:         B — Screen Capture (bsr-capture)
CRATE:          bsr-capture
DEPENDS ON:     Existing CaptureBackend trait, CaptureFrame, CaptureConfig
PRIOR STATE:    CaptureBackend trait defined, MockCaptureBackend on non-Windows,
                DxgiCaptureBackend stub (returns error), CaptureService with
                timer loop and "TODO: Send frame to encoder"
STATUS:         34/34 workspace tests passing, all 7 crates compile

================================================================================
PURPOSE
================================================================================

Replace the stub DxgiCaptureBackend with a real Windows Desktop Duplication
(DXGI) screen capture implementation. When this phase is complete, calling
`capture_frame()` will return actual pixels from the user's display.

This is the most critical missing piece — without real capture, the entire
record pipeline is inert.

================================================================================
WHAT EXISTS TODAY  (do NOT delete or break)
================================================================================

File: crates/bsr-capture/src/lib.rs

  - CaptureError enum (InitializationFailed, FrameAcquisitionFailed,
    UnsupportedFormat, Shutdown)
  - CaptureFrame struct { data: Vec<u8>, timestamp: u64, width: u32,
    height: u32, format: FrameFormat }
  - FrameFormat enum { Bgra8, Rgba8 }
  - CaptureBackend async trait (initialize, capture_frame, shutdown)
  - CaptureConfig { width, height, fps, drop_policy }
  - CaptureTelemetry { frames_captured, frames_dropped, avg_latency_ms }
  - CaptureService<B: CaptureBackend> with run() loop
  - DxgiCaptureBackend stub (empty struct, returns
    CaptureError::FrameAcquisitionFailed("DXGI not implemented"))
  - MockCaptureBackend (non-Windows, generates synthetic frames)
  - 1 passing test (non-Windows mock test — may not run on Windows)

Cargo.toml for bsr-capture:
    bsr-ipc = { path = "../bsr-ipc" }
    async-trait, serde, serde_json, tokio, tracing

================================================================================
APPROACH — WINDOWS DESKTOP DUPLICATION API
================================================================================

Use the `windows` crate (same one already in use by bsr-encode via FFmpeg)
to access DXGI Desktop Duplication directly. This is preferred over the
`windows-capture` crate because:
  1. We already have the `windows` crate ecosystem in the workspace
  2. Full control over frame delivery, timing, and resource management
  3. No additional dependencies

API chain:
  CreateDXGIFactory1 → EnumAdapters → EnumOutputs → DuplicateOutput
  → AcquireNextFrame → copy staging texture → Map → read pixels → Unmap

================================================================================
STEP-BY-STEP IMPLEMENTATION
================================================================================

STEP 1: Add windows crate dependency to bsr-capture
----------------------------------------------------

Edit: crates/bsr-capture/Cargo.toml

Add under [target.'cfg(windows)'.dependencies]:

    windows = { version = "0.54", features = [
        "Win32_Graphics_Dxgi",
        "Win32_Graphics_Dxgi_Common",
        "Win32_Graphics_Direct3D",
        "Win32_Graphics_Direct3D11",
        "Win32_Foundation",
        "Win32_Security",
    ] }

Verify: cargo check -p bsr-capture

STEP 2: Implement DxgiCaptureBackend
-------------------------------------

Replace the stub `mod dxgi_backend` in crates/bsr-capture/src/lib.rs with
a real implementation. The module should remain #[cfg(windows)].

Key types inside the module:

    pub struct DxgiCaptureBackend {
        device: Option<ID3D11Device>,
        device_context: Option<ID3D11DeviceContext>,
        duplication: Option<IDXGIOutputDuplication>,
        staging_texture: Option<ID3D11Texture2D>,
        width: u32,
        height: u32,
        initialized: bool,
    }

SAFETY: The D3D11/DXGI COM pointers are !Send. Follow the same pattern
as Iris's WMF backend:

    unsafe impl Send for DxgiCaptureBackend {}
    unsafe impl Sync for DxgiCaptureBackend {}

These are safe because:
  - All D3D11 operations are serialized through the single DeviceContext
  - The CaptureBackend trait methods take &mut self (exclusive access)
  - No concurrent access is possible through CaptureService

STEP 3: initialize() implementation
-------------------------------------

    async fn initialize(&mut self) -> Result<(), CaptureError> {
        unsafe {
            // 1. Create DXGI factory
            let factory: IDXGIFactory1 = CreateDXGIFactory1()
                .map_err(|e| CaptureError::InitializationFailed(
                    format!("CreateDXGIFactory1: {e}")))?;

            // 2. Get primary adapter (index 0)
            let adapter: IDXGIAdapter1 = factory.EnumAdapters1(0)
                .map_err(|e| CaptureError::InitializationFailed(
                    format!("EnumAdapters1: {e}")))?;

            // 3. Get primary output (index 0)
            let output: IDXGIOutput = adapter.EnumOutputs(0)
                .map_err(|e| CaptureError::InitializationFailed(
                    format!("EnumOutputs: {e}")))?;

            // 4. Get output description for dimensions
            let desc = output.GetDesc()
                .map_err(|e| CaptureError::InitializationFailed(
                    format!("GetDesc: {e}")))?;
            self.width = (desc.DesktopCoordinates.right -
                          desc.DesktopCoordinates.left) as u32;
            self.height = (desc.DesktopCoordinates.bottom -
                           desc.DesktopCoordinates.top) as u32;

            // 5. Create D3D11 device
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter,               // pAdapter
                D3D_DRIVER_TYPE_UNKNOWN, // use the adapter we picked
                None,                    // no software rasterizer
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,                    // default feature levels
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            ).map_err(|e| CaptureError::InitializationFailed(
                format!("D3D11CreateDevice: {e}")))?;

            let device = device.ok_or(CaptureError::InitializationFailed(
                "D3D11 device is None".into()))?;
            let context = context.ok_or(CaptureError::InitializationFailed(
                "D3D11 context is None".into()))?;

            // 6. Cast to IDXGIOutput1 and duplicate
            let output1: IDXGIOutput1 = output.cast()
                .map_err(|e| CaptureError::InitializationFailed(
                    format!("Cast to IDXGIOutput1: {e}")))?;
            let duplication = output1.DuplicateOutput(&device)
                .map_err(|e| CaptureError::InitializationFailed(
                    format!("DuplicateOutput: {e}")))?;

            // 7. Create staging texture (CPU-readable copy target)
            let tex_desc = D3D11_TEXTURE2D_DESC {
                Width: self.width,
                Height: self.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: D3D11_BIND_FLAG(0),
                CPUAccessFlags: D3D11_CPU_ACCESS_READ,
                MiscFlags: D3D11_RESOURCE_MISC_FLAG(0),
            };

            let staging = device.CreateTexture2D(&tex_desc, None)
                .map_err(|e| CaptureError::InitializationFailed(
                    format!("CreateTexture2D staging: {e}")))?;

            self.device = Some(device);
            self.device_context = Some(context);
            self.duplication = Some(duplication);
            self.staging_texture = Some(staging);
            self.initialized = true;

            tracing::info!(
                "DXGI capture initialized: {}x{}", self.width, self.height
            );
            Ok(())
        }
    }

STEP 4: capture_frame() implementation
----------------------------------------

    async fn capture_frame(&mut self) -> Result<CaptureFrame, CaptureError> {
        if !self.initialized {
            return Err(CaptureError::InitializationFailed(
                "Not initialized".into()));
        }

        unsafe {
            let duplication = self.duplication.as_ref().unwrap();
            let context = self.device_context.as_ref().unwrap();
            let staging = self.staging_texture.as_ref().unwrap();

            // AcquireNextFrame with 500ms timeout
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut desktop_resource: Option<IDXGIResource> = None;
            duplication.AcquireNextFrame(
                500, // timeout_ms
                &mut frame_info,
                &mut desktop_resource,
            ).map_err(|e| CaptureError::FrameAcquisitionFailed(
                format!("AcquireNextFrame: {e}")))?;

            let desktop_resource = desktop_resource.ok_or(
                CaptureError::FrameAcquisitionFailed(
                    "Desktop resource is None".into()))?;

            // Get the texture from the resource
            let desktop_texture: ID3D11Texture2D = desktop_resource.cast()
                .map_err(|e| CaptureError::FrameAcquisitionFailed(
                    format!("Cast to Texture2D: {e}")))?;

            // Copy GPU texture → staging texture (CPU-readable)
            context.CopyResource(staging, &desktop_texture);

            // Release the frame ASAP so DXGI can continue
            duplication.ReleaseFrame()
                .map_err(|e| CaptureError::FrameAcquisitionFailed(
                    format!("ReleaseFrame: {e}")))?;

            // Map the staging texture to read pixels
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(
                staging,
                0,
                D3D11_MAP_READ,
                0,
                Some(&mut mapped),
            ).map_err(|e| CaptureError::FrameAcquisitionFailed(
                format!("Map: {e}")))?;

            // Copy pixel rows (src stride may differ from width*4)
            let src_stride = mapped.RowPitch as usize;
            let dst_stride = (self.width * 4) as usize;
            let mut data = Vec::with_capacity(dst_stride * self.height as usize);

            let src_ptr = mapped.pData as *const u8;
            for row in 0..self.height {
                let src_row = std::slice::from_raw_parts(
                    src_ptr.add(row as usize * src_stride),
                    dst_stride,
                );
                data.extend_from_slice(src_row);
            }

            context.Unmap(staging, 0);

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64;

            Ok(CaptureFrame {
                data,
                timestamp,
                width: self.width,
                height: self.height,
                format: FrameFormat::Bgra8,
            })
        }
    }

STEP 5: shutdown() implementation
-----------------------------------

    async fn shutdown(&mut self) -> Result<(), CaptureError> {
        // Drop in reverse order of creation
        self.duplication = None;
        self.staging_texture = None;
        self.device_context = None;
        self.device = None;
        self.initialized = false;
        tracing::info!("DXGI capture shutdown");
        Ok(())
    }

STEP 6: DxgiCaptureBackend::new() and Drop
--------------------------------------------

    impl DxgiCaptureBackend {
        pub fn new() -> Self {
            Self {
                device: None,
                device_context: None,
                duplication: None,
                staging_texture: None,
                width: 0,
                height: 0,
                initialized: false,
            }
        }
    }

    impl Drop for DxgiCaptureBackend {
        fn drop(&mut self) {
            if self.initialized {
                tracing::warn!("DxgiCaptureBackend dropped while still active");
            }
            // COM pointers are released by Drop on the Option<T> fields
        }
    }

STEP 7: Make DxgiCaptureBackend public
---------------------------------------

Add to the top of lib.rs (outside the module, but still #[cfg(windows)]):

    #[cfg(windows)]
    pub use dxgi_backend::DxgiCaptureBackend;

STEP 8: Add hardware-gated test
---------------------------------

Add to #[cfg(test)] mod tests in lib.rs:

    /// Hardware test — only runs when BSR_USE_HW=1.
    /// Requires a Windows machine with a display.
    #[cfg(windows)]
    #[tokio::test]
    async fn test_dxgi_real_capture() {
        if std::env::var("BSR_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping test_dxgi_real_capture (set BSR_USE_HW=1)");
            return;
        }
        let mut backend = dxgi_backend::DxgiCaptureBackend::new();
        backend.initialize().await.unwrap();

        let frame = backend.capture_frame().await.unwrap();
        eprintln!("Captured: {}x{} format={:?} data_len={}",
            frame.width, frame.height, frame.format, frame.data.len());
        assert!(frame.width > 0);
        assert!(frame.height > 0);
        assert_eq!(frame.format, FrameFormat::Bgra8);
        assert_eq!(frame.data.len(), (frame.width * frame.height * 4) as usize);

        backend.shutdown().await.unwrap();
    }

================================================================================
WINDOWS API IMPORTS NEEDED
================================================================================

    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Direct3D::*;
    use windows::Win32::Graphics::Direct3D11::*;

Key functions and types:
    CreateDXGIFactory1, IDXGIFactory1, IDXGIAdapter1, IDXGIOutput,
    IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11Texture2D, D3D11_TEXTURE2D_DESC, D3D11_MAPPED_SUBRESOURCE,
    D3D11_USAGE_STAGING, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D_DRIVER_TYPE_UNKNOWN,
    D3D11_SDK_VERSION, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_OUTDUPL_FRAME_INFO, DXGI_SAMPLE_DESC

windows crate version: 0.54 (match existing workspace usage)

Check exact API signatures against windows 0.54 — some functions use
out-pointer patterns. The code above shows the expected patterns but
verify with cargo check. Common gotchas in 0.54:
  - D3D11CreateDevice takes Some(&mut option) for out-params
  - CreateDXGIFactory1() returns Result<T> directly (generic)
  - Map/Unmap may take the resource as a reference, not trait object

================================================================================
ACCEPTANCE CRITERIA
================================================================================

1. cargo check -p bsr-capture compiles clean (0 errors, 0 warnings)
2. cargo test --workspace passes all 34+ existing tests (no regression)
3. With BSR_USE_HW=1: test_dxgi_real_capture passes
4. Captured frame dimensions match the primary monitor resolution
5. Frame data length == width * height * 4 (BGRA8)
6. Frame format == FrameFormat::Bgra8
7. DxgiCaptureBackend::new() succeeds without initialize()
8. shutdown() releases all D3D/DXGI resources
9. capture_frame() returns error if not initialized

================================================================================
KNOWN RISKS & MITIGATIONS
================================================================================

RISK: windows 0.54 API signature mismatch
  → Run cargo check after every change, fix signatures iteratively
  → Use the same pattern from Iris's WMF backend (out-pointer for create,
    .ok()? for HRESULT returns that don't wrap Result)

RISK: Desktop Duplication not available (RDP session, no display)
  → Return CaptureError::InitializationFailed with clear message
  → DuplicateOutput will fail with DXGI_ERROR_NOT_CURRENTLY_AVAILABLE

RISK: Frame timeout when screen is static (DXGI only delivers on change)
  → AcquireNextFrame with 500ms timeout. On timeout, re-deliver the
    previous frame (or retry). The encoder handles duplicate frames fine.

RISK: Stride mismatch (GPU texture row pitch != width * 4)
  → Already handled: row-by-row copy from mapped.RowPitch to dst_stride

================================================================================
VERIFICATION COMMANDS
================================================================================

    cd 'C:\Users\Baxter\Desktop\Baxters Screen Record\Baxters Screen Record'
    $env:VCPKG_ROOT = "C:\tools\vcpkg"
    $env:LIBCLANG_PATH = "C:\tools\LLVM\bin"

    # Compile check
    cargo check -p bsr-capture

    # Run all tests (no regression)
    cargo test --workspace

    # Hardware test (requires display)
    $env:BSR_USE_HW = "1"
    cargo test -p bsr-capture test_dxgi_real_capture -- --nocapture

================================================================================
COMMIT MESSAGE
================================================================================

    Phase-1: Real DXGI capture backend — Desktop Duplication, BGRA8 frames

================================================================================
END OF PHASE 1
================================================================================
