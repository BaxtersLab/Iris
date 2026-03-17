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
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PixelFormat::Rgb24 => write!(f, "RGB24"),
            PixelFormat::Bgr24 => write!(f, "BGR24"),
            PixelFormat::Nv12 => write!(f, "NV12"),
            PixelFormat::Yuyv => write!(f, "YUYV"),
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
