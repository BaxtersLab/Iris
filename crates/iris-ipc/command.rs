use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "cmd", content = "args")]
pub enum IpcCommand {
    Init { config_path: Option<String> },
    Shutdown,
    GetStatus,
    Ping,

    ListDevices,
    SelectDevice { device_id: String },
    GetDeviceCapabilities,
    DisconnectDevice,

    StartCapture,
    StopCapture,
    PauseCapture,
    ResumeCapture,
    SetResolution { width: u32, height: u32 },
    SetFps { fps: u32 },
    SetPixelFormat { format: String },
    SetRoi { x: u32, y: u32, width: u32, height: u32 },
    ClearRoi,

    GetControl { control: String },
    SetControl { control: String, value: i64 },
    ResetControl { control: String },
    ListControls,
    LoadProfile { name: String },
    SaveProfile { name: String },

    Subscribe,
    Unsubscribe { subscriber_id: u64 },
    GetStreamStats,

    ReloadConfig,
    GetConfig,
    UpdateConfig { section: String, json: String },
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
            IpcCommand::SetResolution { width: 1920, height: 1080 },
            IpcCommand::SetPixelFormat { format: "nv12".to_string() },
            IpcCommand::Subscribe,
        ];

        for cmd in cmds {
            let s = serde_json::to_string(&cmd).unwrap();
            let parsed: IpcCommand = serde_json::from_str(&s).unwrap();
            assert_eq!(cmd, parsed);
        }
    }
}
