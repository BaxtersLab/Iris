use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpcEnvelope {
    pub id: u64,
    pub payload: IpcPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "body")]
pub enum IpcPayload {
    Command(super::command::IpcCommand),
    Response(super::response::IpcResponse),
    Telemetry(super::telemetry::TelemetryEnvelope),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn envelope_json_roundtrip() {
        let cmd = super::super::command::IpcCommand::Ping;
        let env = IpcEnvelope {
            id: 1,
            payload: IpcPayload::Command(cmd),
        };
        let s = serde_json::to_string(&env).unwrap();
        let parsed: IpcEnvelope = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.id, 1);
    }
}
