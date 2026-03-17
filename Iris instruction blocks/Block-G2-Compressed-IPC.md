Block G-2 — iris-stream Compressed IPC
=======================================

Objective
---------
Add compressed frame streaming to iris-stream behind a `compressed-ipc` Cargo
feature flag. This enables MJPEG passthrough and optional software H.264 encoding
for bandwidth-constrained IPC delivery. This is entirely additive — no existing
code from G-1 is modified, only new modules and a feature gate.

Prerequisites
-------------
Block G-1 must be complete.

Feature Flag Setup
------------------
In crates/iris-stream/Cargo.toml, add:

```toml
[features]
default = []
compressed-ipc = ["dep:image"]

[dependencies]
image = { version = "0.25", optional = true }
```

The `image` crate provides JPEG encoding. For H.264, we use a stub trait (real
H.264 encoding will require an external encoder like x264 or OpenH264, which can
be added later without changing this interface).

File: crates/iris-stream/compressed.rs
---------------------------------------
Only compiled when `compressed-ipc` feature is enabled.

```rust
#![cfg(feature = "compressed-ipc")]

use serde::{Deserialize, Serialize};
use iris_capture::frame::CaptureFrame;
use iris_hal::device::PixelFormat;

/// Compressed frame codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressedCodec {
    /// MJPEG — either passthrough (if source is MJPEG) or JPEG encode.
    Mjpeg,
    /// H.264 — via software encoder (stub).
    H264,
}

/// A compressed frame ready for IPC delivery.
#[derive(Debug, Clone)]
pub struct CompressedFrame {
    /// Original frame sequence number.
    pub sequence: u64,
    /// Codec used.
    pub codec: CompressedCodec,
    /// Compressed data bytes.
    pub data: Vec<u8>,
    /// Original width.
    pub width: u32,
    /// Original height.
    pub height: u32,
    /// Compression ratio (original_size / compressed_size).
    pub compression_ratio: f32,
    /// Timestamp in microseconds.
    pub timestamp_us: u64,
}

/// Trait for frame compressors.
pub trait FrameCompressor: Send + Sync {
    /// Compress a raw capture frame.
    fn compress(&self, frame: &CaptureFrame) -> Result<CompressedFrame, String>;

    /// Get the codec this compressor produces.
    fn codec(&self) -> CompressedCodec;
}

/// MJPEG compressor.
/// - If source frame is already MJPEG: passthrough (zero-cost).
/// - If source frame is BGRA8: encode to JPEG using the `image` crate.
/// - Other formats: return error (conversion not supported yet).
pub struct MjpegCompressor {
    /// JPEG quality (1-100).
    pub quality: u8,
}

impl MjpegCompressor {
    pub fn new(quality: u8) -> Self {
        Self { quality: quality.clamp(1, 100) }
    }
}

impl FrameCompressor for MjpegCompressor {
    fn compress(&self, frame: &CaptureFrame) -> Result<CompressedFrame, String> {
        let data = match frame.format {
            PixelFormat::Mjpeg => {
                // Passthrough — already JPEG encoded
                frame.data.clone()
            }
            PixelFormat::Bgra8 => {
                // Encode BGRA8 to JPEG using the `image` crate
                use image::{ImageBuffer, Rgba};
                let img: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(
                    frame.width, frame.height, frame.data.clone(),
                ).ok_or("invalid BGRA8 frame dimensions")?;

                let mut buf = std::io::Cursor::new(Vec::new());
                img.write_to(&mut buf, image::ImageFormat::Jpeg)
                    .map_err(|e| format!("JPEG encode error: {e}"))?;
                buf.into_inner()
            }
            _ => return Err(format!("MJPEG compression not supported for {:?}", frame.format)),
        };

        let ratio = if !data.is_empty() {
            frame.data.len() as f32 / data.len() as f32
        } else {
            0.0
        };

        Ok(CompressedFrame {
            sequence: frame.sequence,
            codec: CompressedCodec::Mjpeg,
            data,
            width: frame.width,
            height: frame.height,
            compression_ratio: ratio,
            timestamp_us: frame.timestamp_us,
        })
    }

    fn codec(&self) -> CompressedCodec {
        CompressedCodec::Mjpeg
    }
}

/// H.264 compressor stub.
/// Always returns an error — real H.264 encoding requires an external library.
pub struct H264CompressorStub;

impl FrameCompressor for H264CompressorStub {
    fn compress(&self, _frame: &CaptureFrame) -> Result<CompressedFrame, String> {
        Err("H.264 encoding not yet implemented — requires external encoder".into())
    }

    fn codec(&self) -> CompressedCodec {
        CompressedCodec::H264
    }
}
```

### Integration with StreamService

Add to `crates/iris-stream/service.rs`, guarded by `#[cfg(feature = "compressed-ipc")]`:

```rust
#[cfg(feature = "compressed-ipc")]
impl StreamService {
    /// Deliver a compressed frame to IPC subscribers.
    /// Serializes the CompressedFrame metadata as JSON header + binary data.
    fn deliver_compressed(
        &mut self,
        compressed: &compressed::CompressedFrame,
    ) {
        // For each IPC-mode subscriber: serialize and send
        // For now: write to a local buffer (real named pipe delivery in I-1)
    }
}
```

### Update lib.rs

Add to crates/iris-stream/lib.rs:

```rust
#[cfg(feature = "compressed-ipc")]
pub mod compressed;
```

Unit Tests
----------
File: crates/iris-stream/compressed_tests.rs (only compiled with feature)

```rust
#![cfg(feature = "compressed-ipc")]
```

### Required Tests

1. `test_mjpeg_passthrough` — create a CaptureFrame with PixelFormat::Mjpeg and fake JPEG data, compress with MjpegCompressor, verify output data == input data
2. `test_mjpeg_encode_bgra8` — create a small 4x4 BGRA8 frame, compress, verify output is valid JPEG bytes (starts with 0xFF 0xD8)
3. `test_mjpeg_unsupported_format` — NV12 frame returns Err
4. `test_h264_stub_returns_error` — H264CompressorStub always returns Err
5. `test_compression_ratio` — BGRA8 frame compressed, ratio > 1.0 (compressed smaller than raw)
6. `test_compressed_frame_metadata` — sequence, width, height, timestamp preserved through compression

### How to run tests for this block
```
cargo test -p iris-stream --features compressed-ipc
```

Acceptance Criteria
-------------------
1. `cargo check -p iris-stream` passes (without feature)
2. `cargo check -p iris-stream --features compressed-ipc` passes
3. `cargo test -p iris-stream --features compressed-ipc` — all 6 compressed tests pass
4. Existing G-1 tests still pass without the feature enabled
5. MJPEG passthrough is zero-cost for MJPEG source frames
6. BGRA8 → JPEG encoding produces valid JPEG output
7. H.264 stub compiles and returns clear "not implemented" error
8. No code outside `#[cfg(feature = "compressed-ipc")]` is changed
