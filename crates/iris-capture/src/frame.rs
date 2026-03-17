use serde::{Deserialize, Serialize};
use iris_hal::device::PixelFormat;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
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
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Roi {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Roi {
    pub fn validate(&self, frame_width: u32, frame_height: u32) -> bool {
        self.width > 0
            && self.height > 0
            && self.x + self.width <= frame_width
            && self.y + self.height <= frame_height
    }
}
