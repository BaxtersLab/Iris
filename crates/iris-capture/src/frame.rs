// Re-export the canonical `CaptureFrame` and `PixelFormat` from `iris-core`.
pub use iris_core::{CaptureFrame, PixelFormat};

use serde::{Deserialize, Serialize};

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
