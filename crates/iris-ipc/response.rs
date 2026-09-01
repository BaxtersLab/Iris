use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", content = "data")]
pub enum IpcResponse {
    Ok(ResponseData),
    Error { code: u32, message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "payload")]
pub enum ResponseData {
    /// A captured frame, ready for an OpenAI-format vision request.
    ///
    /// `data_url` is the complete `data:image/jpeg;base64,...` string and drops
    /// straight into `image_url.url`. It is assembled here rather than by each
    /// caller because a caller building it by hand can get the MIME type or the
    /// base64 padding wrong silently, and be told only that the model saw
    /// nothing.
    Frame {
        /// Capture sequence number, so a caller can tell a fresh frame from a
        /// repeat of one it already has.
        sequence: u64,
        /// Geometry AFTER downscaling.
        width: u32,
        height: u32,
        /// Capture time in microseconds since the epoch, for staleness checks.
        captured_us: u64,
        mime: String,
        data_url: String,
    },
    Empty,
    Pong {
        uptime_ms: u64,
    },
    Status {
        capture_state: String,
        device_name: String,
        fps: f64,
        frame_count: u64,
        subscriber_count: usize,
    },
    DeviceList {
        devices: Vec<DeviceEntry>,
    },
    DeviceCapabilities {
        capabilities: String,
    },
    ControlValue {
        control: String,
        value: i64,
    },
    ControlList {
        controls: Vec<ControlEntry>,
    },
    StreamStats {
        frames_delivered: u64,
        frames_dropped: u64,
        subscriber_count: usize,
        ring_buffer_usage: f32,
    },
    SubscriberId {
        id: u64,
    },
    Config {
        json: String,
    },
    ProfileSaved {
        name: String,
    },
    ProfileLoaded {
        name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceEntry {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub resolutions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ControlEntry {
    pub name: String,
    pub current: i64,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub default: i64,
    pub auto_supported: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn response_json_roundtrip() {
        let r = IpcResponse::Ok(ResponseData::Pong { uptime_ms: 123 });
        let s = serde_json::to_string(&r).unwrap();
        let parsed: IpcResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(r, parsed);
    }
}
