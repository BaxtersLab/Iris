use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// NOTE: `size_bytes` on `TelemetryEvent::FrameCaptured` is authoritative and
// represents the exact frame size in bytes as produced by the capture backend
// after any cropping/format conversions. Consumers may rely on this field and
// should treat it as an unsigned byte count.

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TelemetryEnvelope {
    pub timestamp: DateTime<Utc>,
    pub sequence: u64,
    pub event: TelemetryEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", content = "data")]
pub enum TelemetryEvent {
    SystemStarted {
        version: String,
    },
    SystemShutdown {
        reason: String,
    },
    ConfigLoaded {
        path: String,
    },
    ConfigError {
        message: String,
    },

    DeviceEnumerated {
        count: usize,
    },
    DeviceSelected {
        device_id: String,
        name: String,
    },
    DeviceConnected {
        device_id: String,
    },
    DeviceDisconnected {
        device_id: String,
        reason: String,
    },
    DeviceReconnecting {
        attempt: u32,
        max_attempts: u32,
    },
    DeviceCapabilitiesProbed {
        device_id: String,
        resolutions: Vec<String>,
    },

    CaptureStarted {
        width: u32,
        height: u32,
        fps: u32,
        format: String,
    },
    CaptureStopped {
        total_frames: u64,
    },
    CapturePaused,
    CaptureResumed,
    FrameCaptured {
        sequence: u64,
        width: u32,
        height: u32,
        size_bytes: usize,
    },
    /// GestureDetected: emitted by the capture/vision pipeline when a calibrated
    /// gesture is recognized. `gesture` is a stable name (e.g., "thumbs_up_right").
    /// `score` is a confidence 0.0..1.0 and `user_id` is optional to support
    /// per-user calibration/profiles.
    GestureDetected {
        gesture: String,
        score: f32,
        user_id: Option<String>,
    },
    FrameDropped {
        sequence: u64,
        reason: String,
    },
    /// OverlayFieldMoved: emitted by the UI when a user moves or resizes an overlay field.
    /// Contains the `id` and new geometry in pixels relative to the UI canvas.
    OverlayFieldMoved {
        id: u32,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    CaptureError {
        message: String,
    },

    ControlChanged {
        control: String,
        old_value: i64,
        new_value: i64,
    },
    ControlAutoToggled {
        control: String,
        auto_enabled: bool,
    },
    ProfileLoaded {
        name: String,
        controls_applied: usize,
    },
    ProfileSaved {
        name: String,
    },
    /// Emitted when overlay->control mappings are saved from the UI.
    /// `path` is the full path to the mappings TOML file.
    MappingsUpdated {
        path: String,
        controls_applied: usize,
    },

    SubscriberAdded {
        id: u64,
        total: usize,
    },
    SubscriberRemoved {
        id: u64,
        total: usize,
    },
    StreamDelivery {
        subscriber_id: u64,
        frame_sequence: u64,
        latency_us: u64,
    },
    /// The pull ring recycled slots — it overwrote frames nothing had read.
    ///
    /// **This is normal operation, not an incident.** The ring is deliberately
    /// lossy: a pull consumer asks "what do you see right now" and always wants
    /// the newest frame, so overwriting older ones is the design working. It is
    /// reported because the *rate* is diagnostic — a ring recycling far faster
    /// than a consumer reads means the ring is too small or the consumer too
    /// slow — not because any frame was owed to anybody.
    ///
    /// It was called `RingBufferOverflow { dropped_frames }` until 2026-09-01,
    /// which was wrong three separate ways: nothing overflowed, nothing was
    /// dropped, and the number was the lifetime total rather than this event's
    /// count. See `iris-stream`'s `report_recycled`.
    RingBufferRecycled {
        /// Frames overwritten **since the previous report**.
        overwritten: u64,
        /// Frames overwritten since the service started. Matches
        /// `StreamStats::ring_buffer_overwrites`.
        total_overwritten: u64,
    },

    /// Emitted once per second when a rate ceiling discarded events.
    ///
    /// A limiter that silently drops turns a busy system into one that looks
    /// idle. This is sent **outside** the budget it reports on, or the message
    /// about dropping would itself be dropped.
    TelemetryRateLimited {
        dropped: u64,
    },

    HealthCheck {
        cpu_percent: f32,
        memory_mb: f32,
        usb_bandwidth_percent: f32,
    },
    UsbBandwidthWarning {
        current_percent: f32,
        threshold: f32,
    },
    ThermalWarning {
        temperature_c: f32,
    },
    ErrorRecovered {
        subsystem: String,
        message: String,
    },
    FatalError {
        subsystem: String,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json;

    #[test]
    fn telemetry_json_roundtrip() {
        let ev = TelemetryEvent::SystemStarted {
            version: "0.1".to_string(),
        };
        let env = TelemetryEnvelope {
            timestamp: Utc::now(),
            sequence: 1,
            event: ev,
        };
        let s = serde_json::to_string(&env).unwrap();
        let parsed: TelemetryEnvelope = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.sequence, 1);
    }
}
