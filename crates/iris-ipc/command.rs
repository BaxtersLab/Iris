use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "cmd", content = "args")]
pub enum IpcCommand {
    Init {
        config_path: Option<String>,
    },
    Shutdown,
    GetStatus,
    Ping,

    ListDevices,
    SelectDevice {
        device_id: String,
    },
    GetDeviceCapabilities,
    DisconnectDevice,

    StartCapture,
    StopCapture,
    PauseCapture,
    ResumeCapture,
    SetResolution {
        width: u32,
        height: u32,
    },
    SetFps {
        fps: u32,
    },
    SetPixelFormat {
        format: String,
    },
    SetRoi {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    ClearRoi,

    GetControl {
        control: String,
    },
    SetControl {
        control: String,
        value: i64,
    },
    ResetControl {
        control: String,
    },
    ListControls,
    LoadProfile {
        name: String,
    },
    SaveProfile {
        name: String,
    },

    Subscribe,
    Unsubscribe {
        subscriber_id: u64,
    },
    GetStreamStats,
    /// "What do you see right now?" — the newest captured frame, prepared for
    /// a vision model.
    ///
    /// Pull rather than push on purpose: the consumer this exists for is a
    /// local llama.cpp model with an mmproj, which looks when it decides to,
    /// not thirty times a second.
    GetFrame {
        /// Downscale so the longest edge is at most this wide. `None` uses a
        /// default suited to a vision projector; `Some(0)` disables scaling.
        #[serde(default)]
        max_width: Option<u32>,
        /// JPEG quality 1-100. `None` uses the default.
        #[serde(default)]
        quality: Option<u8>,
    },

    ReloadConfig,
    GetConfig,
    UpdateConfig {
        section: String,
        json: String,
    },

    /// Diagnostics: trigger an in-process rebase increment (tests/metrics).
    ForceRebase,
    /// Bring the Iris window to the foreground (no-op where unsupported).
    ShowUi,
}

#[cfg(test)]
mod tests {
    use super::IpcCommand;
    use serde_json;

    #[test]
    fn command_json_roundtrip() {
        let cmds = vec![
            IpcCommand::Ping,
            IpcCommand::GetStatus,
            IpcCommand::Init { config_path: None },
            IpcCommand::SetResolution {
                width: 1920,
                height: 1080,
            },
            IpcCommand::SetPixelFormat {
                format: "nv12".to_string(),
            },
            IpcCommand::Subscribe,
        ];

        for cmd in cmds {
            let s = serde_json::to_string(&cmd).unwrap();
            let parsed: IpcCommand = serde_json::from_str(&s).unwrap();
            assert_eq!(cmd, parsed);
        }
    }
}
