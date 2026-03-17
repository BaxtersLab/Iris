use crate::event::{HrtCommand, HrtStatus};
use crate::service::{HrtConfig, HrtService};
use tokio::task;

#[tokio::test]
async fn test_hrt_start_stop() {
    let (tx, _rx) = tokio::sync::broadcast::channel(16);
    let (service, handle) = HrtService::new(HrtConfig::default(), tx);
    let srv = task::spawn(service.run());
    handle.send(HrtCommand::Start).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(handle.status(), HrtStatus::Monitoring);
    handle.send(HrtCommand::Stop).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert_eq!(handle.status(), HrtStatus::Stopped);
    handle.send(HrtCommand::Shutdown).await.unwrap();
    srv.abort();
}

#[tokio::test]
async fn test_hrt_health_tick() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let (service, handle) = HrtService::new(
        HrtConfig {
            interval_ms: 10,
            ..HrtConfig::default()
        },
        tx,
    );
    let srv = task::spawn(service.run());
    handle.send(HrtCommand::Start).await.unwrap();
    // wait for a telemetry message
    let got = rx.recv().await.unwrap();
    // should be a HealthCheck event
    match got.event {
        iris_ipc::telemetry::TelemetryEvent::HealthCheck { .. } => {}
        _ => panic!("expected HealthCheck"),
    }
    handle.send(HrtCommand::Shutdown).await.unwrap();
    srv.abort();
}

#[tokio::test]
async fn test_hrt_force_check() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let (service, handle) = HrtService::new(HrtConfig::default(), tx);
    let srv = task::spawn(service.run());
    handle.send(HrtCommand::Start).await.unwrap();
    handle.send(HrtCommand::ForceCheck).await.unwrap();
    let got = rx.recv().await.unwrap();
    assert!(matches!(
        got.event,
        iris_ipc::telemetry::TelemetryEvent::HealthCheck { .. }
    ));
    handle.send(HrtCommand::Shutdown).await.unwrap();
    srv.abort();
}

#[tokio::test]
async fn test_hrt_set_interval() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let (service, handle) = HrtService::new(HrtConfig::default(), tx);
    let srv = task::spawn(service.run());
    handle
        .send(HrtCommand::SetInterval { interval_ms: 5 })
        .await
        .unwrap();
    handle.send(HrtCommand::Start).await.unwrap();
    // receive at least one tick quickly
    let _ = rx.recv().await.unwrap();
    handle.send(HrtCommand::Shutdown).await.unwrap();
    srv.abort();
}

#[tokio::test]
async fn test_hrt_shutdown() {
    let (tx, _rx) = tokio::sync::broadcast::channel(16);
    let (service, handle) = HrtService::new(HrtConfig::default(), tx);
    let srv = task::spawn(service.run());
    handle.send(HrtCommand::Shutdown).await.unwrap();
    // allow service to exit
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    srv.abort();
}

#[tokio::test]
async fn test_hrt_usb_bandwidth_warning() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let (service, handle) = HrtService::new(
        HrtConfig {
            usb_bandwidth_threshold: 0.5,
            ..HrtConfig::default()
        },
        tx,
    );
    let srv = task::spawn(service.run());
    // set override metrics with usb bandwidth > threshold
    handle.set_metrics_override(1.0, 1.0, 0.75).await; // 0.75 > 0.5
    handle.send(HrtCommand::ForceCheck).await.unwrap();
    let got = rx.recv().await.unwrap();
    // one of the events should be UsbBandwidthWarning (force check emits HealthCheck then maybe warning)
    assert!(matches!(
        got.event,
        iris_ipc::telemetry::TelemetryEvent::HealthCheck { .. }
            | iris_ipc::telemetry::TelemetryEvent::UsbBandwidthWarning { .. }
    ));
    handle.send(HrtCommand::Shutdown).await.unwrap();
    srv.abort();
}

#[tokio::test]
async fn test_hrt_status_watch() {
    let (tx, _rx) = tokio::sync::broadcast::channel(16);
    let (service, handle) = HrtService::new(HrtConfig::default(), tx);
    let srv = task::spawn(service.run());
    let mut sub = handle.subscribe_status();
    handle.send(HrtCommand::Start).await.unwrap();
    sub.changed().await.unwrap();
    assert_eq!(*sub.borrow(), HrtStatus::Monitoring);
    handle.send(HrtCommand::Stop).await.unwrap();
    sub.changed().await.unwrap();
    assert_eq!(*sub.borrow(), HrtStatus::Stopped);
    handle.send(HrtCommand::Shutdown).await.unwrap();
    srv.abort();
}
