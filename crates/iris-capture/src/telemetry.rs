use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureTelemetry {
    pub frames_captured: u64,
    pub frames_dropped: u64,
    pub current_fps: f64,
    pub target_fps: u32,
    pub resolution: String,
    pub format: String,
    pub size_bytes: usize,
    pub queue_depth: usize,
    pub roi_active: bool,
}
