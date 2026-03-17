use serde::{Deserialize, Serialize};

/// Events the HRT service can detect / emit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HrtEvent {
    HealthTick {
        cpu_percent: f32,
        memory_mb: f32,
        usb_bandwidth_percent: f32,
    },
    UsbBandwidthWarning {
        current_percent: f32,
        threshold: f32,
    },
    UsbDisconnected {
        device_id: String,
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

/// Commands that can be sent to the HRT service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HrtCommand {
    Start,
    Stop,
    ForceCheck,
    SetInterval { interval_ms: u64 },
    SetUsbThreshold { threshold: f32 },
    Shutdown,
}

/// Current status of the HRT service.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HrtStatus {
    Idle,
    Monitoring,
    Stopped,
}
