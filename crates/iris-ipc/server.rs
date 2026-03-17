use crate::command::IpcCommand;
use crate::response::{IpcResponse, ResponseData};
use crate::telemetry::{TelemetryEnvelope, TelemetryEvent};
use chrono::Utc;
use iris_core::error::IrisResult;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc, oneshot};

pub struct IpcServer {
    cmd_rx: mpsc::Receiver<(IpcCommand, oneshot::Sender<IpcResponse>)>,
    telemetry_tx: broadcast::Sender<TelemetryEnvelope>,
}

pub struct IpcHandle {
    cmd_tx: mpsc::Sender<(IpcCommand, oneshot::Sender<IpcResponse>)>,
    #[allow(dead_code)]
    telemetry_rx: broadcast::Receiver<TelemetryEnvelope>,
    telemetry_tx: broadcast::Sender<TelemetryEnvelope>,
}

/// A small wrapper around `broadcast::Receiver` that logs `Lagged` and other recv errors.
pub struct LoggedTelemetryReceiver {
    inner: broadcast::Receiver<TelemetryEnvelope>,
    last_sequence: Option<u64>,
    id: usize,
}

impl LoggedTelemetryReceiver {
    pub fn new(inner: broadcast::Receiver<TelemetryEnvelope>) -> Self {
        static RECEIVER_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let id = RECEIVER_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        println!("LoggedTelemetryReceiver::new created id={}", id);
        Self {
            inner,
            last_sequence: None,
            id,
        }
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn try_recv(&mut self) -> Result<TelemetryEnvelope, broadcast::error::TryRecvError> {
        match self.inner.try_recv() {
            Ok(env) => Ok(env),
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                println!("LoggedTelemetryReceiver(id={}): try_recv Lagged ts={} skipped {} messages last_seq={:?}", self.id, Utc::now(), n, self.last_sequence);
                Err(broadcast::error::TryRecvError::Lagged(n))
            }
            Err(e) => {
                println!(
                    "LoggedTelemetryReceiver(id={}): try_recv error ts={} err={:?}",
                    self.id,
                    Utc::now(),
                    e
                );
                Err(e)
            }
        }
    }

    pub async fn recv(&mut self) -> Result<TelemetryEnvelope, broadcast::error::RecvError> {
        println!(
            "LoggedTelemetryReceiver(id={}): recv await ts={}",
            self.id,
            Utc::now()
        );
        match self.inner.recv().await {
            Ok(env) => {
                let now = Utc::now();
                println!("LoggedTelemetryReceiver(id={}): recv ok ts={} sequence={} event={:?} last_seq={:?}", self.id, now, env.sequence, env.event, self.last_sequence);
                // update last seen sequence
                self.last_sequence = Some(env.sequence);
                Ok(env)
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                println!("LoggedTelemetryReceiver(id={}): recv Lagged ts={} skipped {} messages last_seq={:?}", self.id, Utc::now(), n, self.last_sequence);
                Err(broadcast::error::RecvError::Lagged(n))
            }
            Err(e) => {
                println!(
                    "LoggedTelemetryReceiver(id={}): recv error ts={} err={:?}",
                    self.id,
                    Utc::now(),
                    e
                );
                Err(e)
            }
        }
    }
}

impl IpcServer {
    pub fn new(
        buffer_size: usize,
    ) -> (
        Self,
        IpcHandle,
        tokio::sync::broadcast::Sender<TelemetryEnvelope>,
    ) {
        let (cmd_tx, cmd_rx) = mpsc::channel(buffer_size);
        // Increase telemetry broadcast channel capacity to reduce likelihood of
        // receiver lag/drops under bursty load in tests and headless runs.
        let (telemetry_tx, telemetry_rx) = broadcast::channel(4096);

        (
            Self {
                cmd_rx,
                telemetry_tx: telemetry_tx.clone(),
            },
            IpcHandle {
                cmd_tx,
                telemetry_rx,
                telemetry_tx: telemetry_tx.clone(),
            },
            telemetry_tx.clone(),
        )
    }

    // Note: keep function returning quickly; additional debug logging occurs
    // elsewhere when telemetry is emitted or subscribers join.

    pub async fn run(mut self) {
        while let Some((cmd, resp_tx)) = self.cmd_rx.recv().await {
            match cmd {
                IpcCommand::Ping => {
                    let uptime = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let _ = resp_tx.send(IpcResponse::Ok(ResponseData::Pong { uptime_ms: uptime }));
                }
                IpcCommand::GetStatus => {
                    let status = ResponseData::Status {
                        capture_state: "Disconnected".to_string(),
                        device_name: "".to_string(),
                        fps: 0.0,
                        frame_count: 0,
                        subscriber_count: 0,
                    };
                    let _ = resp_tx.send(IpcResponse::Ok(status));
                }
                _ => {
                    let _ = resp_tx.send(IpcResponse::Ok(ResponseData::Empty));
                }
            }
        }
    }

    /// Run the IPC server using a provided dispatcher implementation.
    pub async fn run_with_dispatcher<D: crate::Dispatcher>(mut self, mut dispatcher: D) {
        while let Some((cmd, resp_tx)) = self.cmd_rx.recv().await {
            let fut = dispatcher.dispatch(cmd);
            let resp = fut.await;
            let _ = resp_tx.send(resp);
        }
    }

    pub fn emit_telemetry(&self, event: TelemetryEvent) {
        let envelope = TelemetryEnvelope {
            timestamp: chrono::Utc::now(),
            sequence: 0,
            event: event.clone(),
        };
        println!(
            "IpcServer: emit_telemetry event={:?} seq=0 ts={}",
            envelope.event, envelope.timestamp
        );
        if let Err(e) = self.telemetry_tx.send(envelope) {
            println!("IpcServer: telemetry send error: {:?}", e);
        } else {
            println!("IpcServer: telemetry envelope sent");
        }
    }
}

impl IpcHandle {
    pub async fn send_command(&self, cmd: IpcCommand) -> IrisResult<IpcResponse> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.cmd_tx
            .send((cmd, resp_tx))
            .await
            .map_err(|e| iris_core::error::IrisError::Ipc(format!("send failed: {}", e)))?;
        let resp = resp_rx
            .await
            .map_err(|e| iris_core::error::IrisError::Ipc(format!("recv failed: {}", e)))?;
        Ok(resp)
    }

    pub fn subscribe_telemetry(&self) -> LoggedTelemetryReceiver {
        println!("IpcHandle: subscribe_telemetry called");
        // Create a fresh subscription from the sender to avoid inheriting the
        // stored receiver's cursor position which may lag behind.
        let r = self.telemetry_tx.subscribe();
        println!("IpcHandle: subscribe_telemetry returned receiver (fresh)");
        LoggedTelemetryReceiver::new(r)
    }
}
