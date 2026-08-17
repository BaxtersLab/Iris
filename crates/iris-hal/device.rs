use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub String);

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormatDescriptor {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub pixel_format: PixelFormat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PixelFormat {
    Rgb24,
    Bgr24,
    Nv12,
    Yuyv,
    /// Motion-JPEG — a **compressed** stream, one JPEG image per frame.
    ///
    /// Unlike every other variant this is not raw pixel data, so it cannot be
    /// cropped, strided or indexed without being decoded first. Callers must
    /// treat it as an opaque byte blob.
    ///
    /// It is enumerated because on USB 2.0 UVC cameras nearly every mode above
    /// ~640x480 is MJPEG-only (uncompressed 1080p exceeds USB 2.0 bandwidth);
    /// skipping it makes such cameras appear far more limited than they are.
    /// Windows Media Foundation decodes MJPEG transparently and reports NV12,
    /// so this variant is only ever produced by the Linux V4L2 path.
    Mjpeg,
}

impl PixelFormat {
    /// True when the variant is raw pixel data that can be cropped/indexed
    /// directly. False for compressed streams, which must be decoded first.
    pub fn is_raw(&self) -> bool {
        !matches!(self, PixelFormat::Mjpeg)
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PixelFormat::Rgb24 => write!(f, "RGB24"),
            PixelFormat::Bgr24 => write!(f, "BGR24"),
            PixelFormat::Nv12 => write!(f, "NV12"),
            PixelFormat::Yuyv => write!(f, "YUYV"),
            PixelFormat::Mjpeg => write!(f, "MJPEG"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceCapabilities {
    pub formats: Vec<FormatDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlCapabilityInfo {
    pub id: u32,
    pub name: String,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub default: i64,
}
