use crate::client::IpcClient;
use crate::command::IpcCommand;
use crate::envelope::IpcEnvelope;
use crate::response::{IpcResponse, ResponseData};
use crate::server::IpcServer;
use crate::telemetry::{TelemetryEnvelope, TelemetryEvent};
use tokio::task;

#[tokio::test]
async fn test_command_json_roundtrip() {
    let cmd = IpcCommand::SetFps { fps: 60 };
    let s = serde_json::to_string(&cmd).unwrap();
    let parsed: IpcCommand = serde_json::from_str(&s).unwrap();
    assert_eq!(cmd, parsed);
}

#[tokio::test]
async fn test_response_json_roundtrip() {
    let r = IpcResponse::Ok(ResponseData::Pong { uptime_ms: 5 });
    let s = serde_json::to_string(&r).unwrap();
    let parsed: IpcResponse = serde_json::from_str(&s).unwrap();
    assert_eq!(r, parsed);
}

#[tokio::test]
async fn test_telemetry_json_roundtrip() {
    let env = TelemetryEnvelope {
        timestamp: chrono::Utc::now(),
        sequence: 1,
        event: TelemetryEvent::CaptureStarted {
            width: 1920,
            height: 1080,
            fps: 30,
            format: "nv12".to_string(),
        },
    };
    let s = serde_json::to_string(&env).unwrap();
    let parsed: TelemetryEnvelope = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed.sequence, 1);
}

#[tokio::test]
async fn test_envelope_json_roundtrip() {
    let env = IpcEnvelope {
        id: 7,
        payload: crate::envelope::IpcPayload::Command(IpcCommand::Ping),
    };
    let s = serde_json::to_string(&env).unwrap();
    let parsed: IpcEnvelope = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed.id, 7);
}

#[tokio::test]
async fn test_ipc_server_ping() {
    let (server, handle, _telemetry_tx) = IpcServer::new(8);
    let srv = task::spawn(server.run());
    let client = IpcClient::new(handle);
    let resp = client.ping().await.unwrap();
    match resp {
        IpcResponse::Ok(ResponseData::Pong { uptime_ms: _ }) => {}
        _ => panic!("expected pong"),
    }
    // drop client to close channel
    drop(client);
    srv.abort();
}

#[tokio::test]
async fn test_ipc_server_telemetry_broadcast() {
    let (server, handle, _telemetry_tx) = IpcServer::new(8);
    let mut sub = handle.subscribe_telemetry();
    server.emit_telemetry(TelemetryEvent::DeviceEnumerated { count: 2 });
    let got = sub.recv().await.unwrap();
    match got.event {
        TelemetryEvent::DeviceEnumerated { count } => assert_eq!(count, 2),
        _ => panic!("unexpected event"),
    }
}

#[tokio::test]
async fn test_ipc_client_send_command() {
    let (server, handle, _telemetry_tx) = IpcServer::new(8);
    let srv = task::spawn(server.run());
    let client = IpcClient::new(handle);
    let resp = client.get_status().await.unwrap();
    match resp {
        IpcResponse::Ok(ResponseData::Status { .. }) => {}
        _ => panic!("expected status"),
    }
    drop(client);
    srv.abort();
}

#[tokio::test]
async fn test_telemetry_sequence_ordering() {
    let (server, handle, _telemetry_tx) = IpcServer::new(8);
    // emit multiple telemetry events; subscribe via handle
    let mut sub = handle.subscribe_telemetry();
    server.emit_telemetry(TelemetryEvent::SystemStarted {
        version: "0".to_string(),
    });
    server.emit_telemetry(TelemetryEvent::SystemShutdown {
        reason: "none".to_string(),
    });
    let a: crate::telemetry::TelemetryEnvelope = sub.recv().await.unwrap();
    let b: crate::telemetry::TelemetryEnvelope = sub.recv().await.unwrap();
    // we only assert that we received two events
    assert!(matches!(a.event, TelemetryEvent::SystemStarted { .. }));
    assert!(matches!(b.event, TelemetryEvent::SystemShutdown { .. }));
}
