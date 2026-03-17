use crate::event::{HrtCommand, HrtEvent, HrtStatus};
use iris_ipc::telemetry::{TelemetryEnvelope, TelemetryEvent};
use tokio::sync::{mpsc, watch, broadcast};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use iris_core::error::IrisResult;

/// Configuration for the HRT service.
pub struct HrtConfig {
    pub interval_ms: u64,
    pub usb_bandwidth_threshold: f32,
    pub thermal_threshold_c: f32,
}

impl Default for HrtConfig {
    fn default() -> Self {
        Self { interval_ms: 2000, usb_bandwidth_threshold: 0.85, thermal_threshold_c: 75.0 }
    }
}

/// The HRT background service.
pub struct HrtService {
    config: HrtConfig,
    cmd_rx: mpsc::Receiver<HrtCommand>,
    status_tx: watch::Sender<HrtStatus>,
    telemetry_tx: broadcast::Sender<TelemetryEnvelope>,
    sequence: Arc<AtomicU64>,
    metrics_override: Arc<tokio::sync::Mutex<Option<(f32,f32,f32)>>>,
}

/// Handle for sending commands and reading status.
pub struct HrtHandle {
    cmd_tx: mpsc::Sender<HrtCommand>,
    status_rx: watch::Receiver<HrtStatus>,
    metrics_override: Arc<tokio::sync::Mutex<Option<(f32,f32,f32)>>>,
}

impl HrtService {
    pub fn new(config: HrtConfig, telemetry_tx: broadcast::Sender<TelemetryEnvelope>) -> (Self, HrtHandle) {
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (status_tx, status_rx) = watch::channel(HrtStatus::Idle);
        let sequence = Arc::new(AtomicU64::new(1));
        let metrics_override = Arc::new(tokio::sync::Mutex::new(None));

        (
            Self { config, cmd_rx, status_tx: status_tx.clone(), telemetry_tx: telemetry_tx.clone(), sequence: sequence.clone(), metrics_override: metrics_override.clone() },
            HrtHandle { cmd_tx, status_rx, metrics_override }
        )
    }

    pub async fn run(mut self) {
        let mut interval = tokio::time::interval(Duration::from_millis(self.config.interval_ms));
        let mut running = false;

        loop {
            tokio::select! {
                Some(cmd) = self.cmd_rx.recv() => {
                    match cmd {
                        HrtCommand::Start => {
                            let _ = self.status_tx.send(HrtStatus::Monitoring);
                            running = true;
                        }
                        HrtCommand::Stop => {
                            let _ = self.status_tx.send(HrtStatus::Stopped);
                            running = false;
                        }
                        HrtCommand::ForceCheck => {
                            let e = self.collect_metrics().await;
                            self.handle_event(e);
                        }
                        HrtCommand::SetInterval { interval_ms } => {
                            self.config.interval_ms = interval_ms;
                            interval = tokio::time::interval(Duration::from_millis(self.config.interval_ms));
                        }
                        HrtCommand::SetUsbThreshold { threshold } => {
                            self.config.usb_bandwidth_threshold = threshold;
                        }
                        HrtCommand::Shutdown => {
                            let _ = self.status_tx.send(HrtStatus::Stopped);
                            break;
                        }
                    }
                }
                _ = interval.tick() => {
                    if running {
                        let e = self.collect_metrics().await;
                        self.handle_event(e);
                    }
                }
            }
        }
    }

    async fn collect_metrics(&self) -> HrtEvent {
        // Check for test override first
        {
            let mut guard = self.metrics_override.lock().await;
            if let Some((cpu, mem, usb)) = *guard {
                *guard = None;
                return HrtEvent::HealthTick { cpu_percent: cpu, memory_mb: mem, usb_bandwidth_percent: usb };
            }
        }

        // placeholder zeroed metrics
        HrtEvent::HealthTick { cpu_percent: 0.0, memory_mb: 0.0, usb_bandwidth_percent: 0.0 }
    }

    fn handle_event(&self, ev: HrtEvent) {
        match ev {
            HrtEvent::HealthTick { cpu_percent, memory_mb, usb_bandwidth_percent } => {
                // emit HealthCheck telemetry
                let telemetry = TelemetryEvent::HealthCheck { cpu_percent, memory_mb, usb_bandwidth_percent };
                self.emit(telemetry.clone());
                // check usb threshold
                if usb_bandwidth_percent > self.config.usb_bandwidth_threshold {
                    let warn = TelemetryEvent::UsbBandwidthWarning { current_percent: usb_bandwidth_percent, threshold: self.config.usb_bandwidth_threshold };
                    self.emit(warn);
                }
            }
            HrtEvent::UsbBandwidthWarning { current_percent, threshold } => {
                let warn = TelemetryEvent::UsbBandwidthWarning { current_percent, threshold };
                self.emit(warn);
            }
            HrtEvent::UsbDisconnected { device_id } => {
                let ev = TelemetryEvent::DeviceDisconnected { device_id, reason: "disconnected".to_string() };
                self.emit(ev);
            }
            HrtEvent::ThermalWarning { temperature_c } => {
                let ev = TelemetryEvent::ThermalWarning { temperature_c };
                self.emit(ev);
            }
            HrtEvent::ErrorRecovered { subsystem, message } => {
                let ev = TelemetryEvent::ErrorRecovered { subsystem, message };
                self.emit(ev);
            }
            HrtEvent::FatalError { subsystem, message } => {
                let ev = TelemetryEvent::FatalError { subsystem, message };
                self.emit(ev);
            }
        }
    }

    fn emit(&self, event: TelemetryEvent) {
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        let envelope = TelemetryEnvelope { timestamp: chrono::Utc::now(), sequence: seq, event };
        let _ = self.telemetry_tx.send(envelope);
    }
}

impl HrtHandle {
    pub async fn send(&self, cmd: HrtCommand) -> IrisResult<()> {
        self.cmd_tx.send(cmd).await.map_err(|e| iris_core::error::IrisError::Ipc(format!("hrt send failed: {}", e)))?;
        Ok(())
    }

    pub fn status(&self) -> HrtStatus {
        self.status_rx.borrow().clone()
    }

    pub fn subscribe_status(&self) -> watch::Receiver<HrtStatus> {
        self.status_rx.clone()
    }

    /// Testing helper: inject metrics that will be used for the next ForceCheck or tick.
    pub async fn set_metrics_override(&self, cpu: f32, mem: f32, usb: f32) {
        let mut guard = self.metrics_override.lock().await;
        *guard = Some((cpu, mem, usb));
    }
}
