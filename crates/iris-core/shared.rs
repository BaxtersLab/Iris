use serde::{Deserialize, Serialize};

/// An encoded video packet (H.264, etc.) shared across crates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncodedPacket {
    /// Raw encoded bytes (NAL units / container fragments).
    pub data: Vec<u8>,
    /// Presentation timestamp (in stream timescale units, typically 90kHz or configured by encoder).
    pub pts: i64,
    /// Decoding timestamp.
    pub dts: i64,
    /// Whether this packet contains a keyframe (IDR).
    pub keyframe: bool,
    /// Codec identifier (e.g. "h264").
    pub codec: String,
}

/// Simple pixel format enum used for shared frame metadata.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb24,
    Bgr24,
    Nv12,
    Yuyv,
}

/// A single captured screen frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureFrame {
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
    pub timestamp_us: u64,
    pub is_cropped: bool,
}

impl CaptureFrame {
    pub fn size_bytes(&self) -> usize {
        self.data.len()
    }

    pub fn expected_size(width: u32, height: u32, format: PixelFormat) -> usize {
        let pixels = (width * height) as usize;
        match format {
            PixelFormat::Rgb24 | PixelFormat::Bgr24 => pixels * 3,
            PixelFormat::Nv12 => pixels * 3 / 2,
            PixelFormat::Yuyv => pixels * 2,
        }
    }

    pub fn now_us() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64
    }
}
