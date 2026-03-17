use chrono::Utc;
use iris_capture::backend::{CaptureConfig, DropPolicy};
use iris_capture::service::{CaptureCommand, CaptureHandle, CaptureService};
use iris_capture::telemetry::CaptureTelemetry as CaptureTelemetryEvent;
use iris_core::app::AppState;
use iris_core::config::IrisConfig;
use iris_core::error::IrisResult;
use iris_hal::backend::{MockUvcBackend, UvcBackend};
use iris_hrt::service::{HrtConfig, HrtService};
use iris_ipc::telemetry::{TelemetryEnvelope, TelemetryEvent};
use iris_ipc::{response::IpcResponse, response::ResponseData, IpcHandle, IpcServer};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use std::convert::Infallible;
use std::net::SocketAddr;
use iris_core::pipeline::prometheus_text;
use tracing::info;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::mpsc::{self, Sender as MpscSender};
use tokio::task::JoinHandle;

/// Minimal runtime handles returned after bootstrap
pub struct IrisRuntime {
    pub app_state: Arc<AppState>,
    pub ipc_handle: IpcHandle,
    pub capture_handle: CaptureHandle,
    // Join handles for spawned services so the caller may await or drop
    _tasks: Vec<JoinHandle<()>>,
    // Keep a clone of the capture telemetry sender alive for the lifetime of the runtime
    _capture_telemetry_tx: broadcast::Sender<CaptureTelemetryEvent>,
    // persistent keepalive receiver to avoid transient zero-receiver windows
    _capture_telemetry_keepalive: tokio::sync::broadcast::Receiver<CaptureTelemetryEvent>,
}

/// Minimal dispatcher implemented inside `iris-ui` that implements the `iris-ipc::Dispatcher` trait.
pub struct IrisDispatcher {
    capture_cmd: MpscSender<CaptureCommand>,
}

impl IrisDispatcher {
    pub fn new(capture_cmd: MpscSender<CaptureCommand>) -> Self {
        Self { capture_cmd }
    }
}

impl iris_ipc::Dispatcher for IrisDispatcher {
    fn dispatch(
        &mut self,
        cmd: iris_ipc::command::IpcCommand,
    ) -> Pin<Box<dyn Future<Output = IpcResponse> + Send>> {
        let cmd_sender = self.capture_cmd.clone();
        Box::pin(async move {
            use iris_ipc::command::IpcCommand;
            match cmd {
                IpcCommand::Ping => IpcResponse::Ok(ResponseData::Pong { uptime_ms: 0 }),
                IpcCommand::GetStatus => {
                    let status = ResponseData::Status {
                        capture_state: format!("{:?}", ""),
                        device_name: "Mock Camera".to_string(),
                        fps: 0.0,
                        frame_count: 0,
                        subscriber_count: 0,
                    };
                    IpcResponse::Ok(status)
                }
                IpcCommand::ListDevices => {
                    // Query mock backend directly for devices
                    let backend = MockUvcBackend::new();
                    match backend.enumerate_devices().await {
                        Ok(list) => {
                            let devices = list
                                .into_iter()
                                .map(|d| iris_ipc::response::DeviceEntry {
                                    id: d.id.0.clone(),
                                    name: d.name.clone(),
                                    vendor: "Iris".to_string(),
                                    resolutions: vec![],
                                })
                                .collect();
                            IpcResponse::Ok(ResponseData::DeviceList { devices })
                        }
                        Err(_) => IpcResponse::Ok(ResponseData::DeviceList { devices: vec![] }),
                    }
                }
                IpcCommand::ResumeCapture => {
                    let _ = cmd_sender.send(CaptureCommand::Resume).await;
                    IpcResponse::Ok(ResponseData::Empty)
                }
                IpcCommand::PauseCapture => {
                    let _ = cmd_sender.send(CaptureCommand::Pause).await;
                    IpcResponse::Ok(ResponseData::Empty)
                }
                IpcCommand::SetFps { fps } => {
                    let _ = cmd_sender.send(CaptureCommand::SetFps(fps)).await;
                    IpcResponse::Ok(ResponseData::Empty)
                }
                IpcCommand::SetRoi {
                    x,
                    y,
                    width,
                    height,
                } => {
                    let roi = iris_capture::frame::Roi {
                        x,
                        y,
                        width,
                        height,
                    };
                    let _ = cmd_sender.send(CaptureCommand::SetRoi(Some(roi))).await;
                    IpcResponse::Ok(ResponseData::Empty)
                }
                IpcCommand::ClearRoi => {
                    let _ = cmd_sender.send(CaptureCommand::SetRoi(None)).await;
                    IpcResponse::Ok(ResponseData::Empty)
                }
                IpcCommand::SetResolution {
                    width: _,
                    height: _,
                } => IpcResponse::Ok(ResponseData::Empty),
                IpcCommand::SetPixelFormat { format: _ } => IpcResponse::Ok(ResponseData::Empty),
                IpcCommand::SelectDevice { device_id: _ } => IpcResponse::Ok(ResponseData::Empty),
                IpcCommand::GetDeviceCapabilities => {
                    IpcResponse::Ok(ResponseData::DeviceCapabilities {
                        capabilities: "unknown".to_string(),
                    })
                }
                IpcCommand::Subscribe => IpcResponse::Ok(ResponseData::SubscriberId { id: 1 }),
                IpcCommand::Unsubscribe { subscriber_id: _ } => {
                    IpcResponse::Ok(ResponseData::Empty)
                }
                IpcCommand::GetStreamStats => IpcResponse::Ok(ResponseData::StreamStats {
                    frames_delivered: 0,
                    frames_dropped: 0,
                    subscriber_count: 0,
                    ring_buffer_usage: 0.0,
                }),
                IpcCommand::GetConfig => IpcResponse::Ok(ResponseData::Config {
                    json: "{}".to_string(),
                }),
                IpcCommand::ReloadConfig => IpcResponse::Ok(ResponseData::Empty),
                IpcCommand::UpdateConfig {
                    section: _,
                    json: _,
                } => IpcResponse::Ok(ResponseData::Empty),
                IpcCommand::LoadProfile { name } => {
                    IpcResponse::Ok(ResponseData::ProfileLoaded { name })
                }
                IpcCommand::SaveProfile { name } => {
                    IpcResponse::Ok(ResponseData::ProfileSaved { name })
                }
                IpcCommand::StartCapture => {
                    let _ = cmd_sender.send(CaptureCommand::Resume).await;
                    IpcResponse::Ok(ResponseData::Empty)
                }
                IpcCommand::StopCapture => {
                    let _ = cmd_sender.send(CaptureCommand::Stop).await;
                    IpcResponse::Ok(ResponseData::Empty)
                }
                IpcCommand::ForceRebase => {
                    // Trigger an in-process rebase increment for diagnostics/tests
                    iris_core::pipeline::force_increment_rebase_for_test();
                    IpcResponse::Ok(ResponseData::Empty)
                }
                IpcCommand::ShowUi => {
                    // Attempt to bring the Iris window to the foreground on Windows.
                    #[cfg(windows)]
                    {
                        crate::win32::bring_iris_to_front();
                    }
                    IpcResponse::Ok(ResponseData::Empty)
                }
                _ => IpcResponse::Ok(ResponseData::Empty),
            }
        })
    }
}

impl IrisRuntime {
    pub async fn bootstrap(config: IrisConfig) -> IrisResult<Self> {
        // 1. App state
        let app_state = AppState::new();

        // 2. IPC server + handle
        // Note: IpcServer::new returns telemetry sender so we can share it with services.
        let (ipc_server, ipc_handle, telemetry_tx) = IpcServer::new(256);

        let hrt_config = HrtConfig::default();
        let (hrt_service, _hrt_handle) = HrtService::new(hrt_config, telemetry_tx.clone());

        // 4. Mock HAL backend and capture service
        // Pixel format: use available formats from `iris-hal::device::PixelFormat`
        let capture_cfg = CaptureConfig {
            width: config.capture.width,
            height: config.capture.height,
            target_fps: config.capture.target_fps,
            format: iris_hal::device::PixelFormat::Bgr24,
            max_queue_depth: config.capture.max_queue_depth,
            drop_policy: DropPolicy::Oldest,
            roi: None,
        };
        // Select capture backend at runtime. Use `IRIS_BACKEND=dxgi` to force DXGI on Windows,
        // otherwise fall back to the mock backend. This keeps selection configurable and safer.
        // Increase capture telemetry capacity to reduce lag/dropped messages
        // during high-frequency or bursty capture periods in tests.
        let (capture_telemetry_tx, _capture_telemetry_rx) = broadcast::channel(4096);
        let backend_name = std::env::var("IRIS_BACKEND")
            .unwrap_or_default()
            .to_lowercase();

        // Box up a dynamic backend so `CaptureService` can be instantiated at runtime.
        let boxed_backend: Box<dyn iris_capture::backend::CaptureBackend + Send + Sync> =
            if backend_name == "dxgi" {
                #[cfg(windows)]
                {
                    Box::new(iris_capture::DxgiCaptureBackend::new(capture_cfg.clone()))
                }
                #[cfg(not(windows))]
                {
                    Box::new(iris_capture::backend::MockCaptureBackend::new(
                        capture_cfg.clone(),
                    ))
                }
            } else {
                Box::new(iris_capture::backend::MockCaptureBackend::new(
                    capture_cfg.clone(),
                ))
            };

        // Create an external frame sender for possible consumers (encoder, tests).
        let (frame_tx, _frame_rx) = tokio::sync::mpsc::channel(capture_cfg.max_queue_depth);
        let (capture_service, capture_handle) = CaptureService::new(
            boxed_backend,
            capture_cfg.clone(),
            capture_telemetry_tx.clone(),
            frame_tx,
        );

        // create the capture command channel that will be used by the dispatcher
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        // 5. Dispatcher
        let dispatcher = IrisDispatcher::new(cmd_tx.clone());

        // 6. Spawn services
        let mut tasks = Vec::new();

        // Spawn a minimal HTTP server to expose /metrics and a debug endpoint
        // `/debug/force_rebase` which triggers an in-process rebase increment.
        // Binding address can be configured via METRICS_BIND (default 127.0.0.1:9180).
        let metrics_bind = std::env::var("METRICS_BIND").unwrap_or_else(|_| "127.0.0.1:9180".to_string());
        if let Ok(addr) = metrics_bind.parse::<SocketAddr>() {
            let svc = make_service_fn(|_conn| async move {
                Ok::<_, Infallible>(service_fn(|req: Request<Body>| async move {
                    match req.uri().path() {
                        "/metrics" => {
                            let body = prometheus_text();
                            Ok::<_, Infallible>(
                                Response::builder()
                                    .status(200)
                                    .header("content-type", "text/plain; version=0.0.4")
                                    .body(Body::from(body))
                                    .unwrap(),
                            )
                        }
                        "/debug/force_rebase" => {
                            iris_core::pipeline::force_increment_rebase_for_test();
                            Ok::<_, Infallible>(
                                Response::builder()
                                    .status(200)
                                    .body(Body::from("ok"))
                                    .unwrap(),
                            )
                        }
                        _ => Ok::<_, Infallible>(
                            Response::builder()
                                .status(404)
                                .body(Body::from("not found"))
                                .unwrap(),
                        ),
                    }
                }))
            });
            tokio::spawn(async move {
                let _ = Server::bind(&addr).serve(svc).await;
            });
        } else {
            println!("Invalid METRICS_BIND '{}', skipping metrics HTTP server", metrics_bind);
        }
        tasks.push(tokio::spawn(async move {
            ipc_server.run_with_dispatcher(dispatcher).await
        }));
        tasks.push(tokio::spawn(async move { hrt_service.run().await }));
        tasks.push(tokio::spawn(
            async move { capture_service.run(cmd_rx).await },
        ));

        // 7. Forward capture telemetry into the global telemetry envelope stream
        // Create a dedicated receiver for the forwarder directly from the
        // capture telemetry sender to avoid inheriting cursor state via
        // `resubscribe()` which previously caused subtle receiver lifecycle
        // effects in tests.
        let _cap_sub = capture_telemetry_tx.subscribe();
        let seq_counter = Arc::new(AtomicU64::new(0));
        let tx_clone = telemetry_tx.clone();

        // Envelope queues and dispatcher for ordered sending with priority support
        // Increase envelope queue capacities to tolerate bursts and reduce
        // message drops when the dispatcher is busy.
        let (envelope_tx, mut envelope_rx) = mpsc::channel::<TelemetryEnvelope>(4096);
        let (priority_tx, mut priority_rx) = mpsc::channel::<TelemetryEnvelope>(4096);
        let envelope_queue_len = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let priority_queue_len = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // Forwarder: produce ordered envelopes and enqueue them (priority vs normal)
        let envelope_tx_clone = envelope_tx.clone();
        let priority_tx_clone = priority_tx.clone();
        let seq_counter_clone = seq_counter.clone();
        let mut cap_sub_clone = capture_telemetry_tx.subscribe();
        let envelope_queue_len_forwarder = envelope_queue_len.clone();
        let priority_queue_len_forwarder = priority_queue_len.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                match cap_sub_clone.recv().await {
                    Ok(ct) => {
                        println!("Forwarder: received capture telemetry frames={} roi={} resolution={}", ct.frames_captured, ct.roi_active, ct.resolution);
                        let seq = seq_counter_clone.fetch_add(1, Ordering::Relaxed);
                        let mut width = 0u32;
                        let mut height = 0u32;
                        if let Some((w,h)) = ct.resolution.split_once('x') {
                            if let Ok(wv) = w.parse::<u32>() { width = wv; }
                            if let Ok(hv) = h.parse::<u32>() { height = hv; }
                        }
                        let event = TelemetryEvent::FrameCaptured { sequence: ct.frames_captured, width, height, size_bytes: ct.size_bytes };
                        let enqueue_ts = Utc::now();
                        let envelope = TelemetryEnvelope { timestamp: enqueue_ts, sequence: seq, event };

                        if ct.roi_active {
                            // enqueue into priority queue
                            priority_queue_len_forwarder.fetch_add(1, Ordering::Relaxed);
                            if let Err(e) = priority_tx_clone.send(envelope).await {
                                println!("Forwarder: priority queue send failed: {:?}", e);
                                priority_queue_len_forwarder.fetch_sub(1, Ordering::Relaxed);
                            } else {
                                let q = priority_queue_len_forwarder.load(Ordering::Relaxed);
                                println!("Forwarder: enqueued PRIORITY envelope ts={} seq={} priority_queue_len={}", Utc::now(), seq, q);
                            }
                        } else {
                            // enqueue into normal queue
                            envelope_queue_len_forwarder.fetch_add(1, Ordering::Relaxed);
                            if let Err(e) = envelope_tx_clone.send(envelope).await {
                                println!("Forwarder: envelope queue send failed: {:?}", e);
                                envelope_queue_len_forwarder.fetch_sub(1, Ordering::Relaxed);
                            } else {
                                let q = envelope_queue_len_forwarder.load(Ordering::Relaxed);
                                println!("Forwarder: enqueued envelope ts={} seq={} queue_len={}", Utc::now(), seq, q);
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        println!("Forwarder: lagged, skipped {} messages", n);
                        continue;
                    }
                    Err(e) => {
                        println!("Forwarder: recv error: {:?}", e);
                        continue;
                    }
                }
            }
        }));

        // Capture-level debug subscriber: independently observe raw capture telemetry
        // so we can detect whether capture telemetry subscriptions are being dropped.
        let mut cap_debug = capture_telemetry_tx.subscribe();
        tasks.push(tokio::spawn(async move {
            loop {
                match cap_debug.recv().await {
                    Ok(ct) => {
                        println!(
                            "CapDebug: recv frames={} roi={} resolution={}",
                            ct.frames_captured, ct.roi_active, ct.resolution
                        );
                    }
                    Err(e) => {
                        println!("CapDebug: recv error: {:?}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
            }
        }));

        // Keep a persistent receiver alive to reduce race windows where
        // subscriber_count briefly becomes zero between subscriber creation
        // and forwarder subscription. This is a conservative runtime fix
        // that preserves delivery semantics for tests and CI.
        let capture_keepalive = capture_telemetry_tx.subscribe();

        // Dispatcher: pop envelopes and send sequentially to IPC telemetry sender,
        // giving priority to the priority queue so ROI envelopes are delivered first.
        let tx_for_dispatch = tx_clone.clone();
        let envelope_queue_len_dispatcher = envelope_queue_len.clone();
        let priority_queue_len_dispatcher = priority_queue_len.clone();
        // Debug subscriber: independently subscribe to IPC telemetry and log all receipts.
        let mut debug_sub = tx_clone.subscribe();
        tasks.push(tokio::spawn(async move {
            loop {
                match debug_sub.recv().await {
                    Ok(env) => {
                        println!(
                            "DebugSub: recv ts={} sequence={} event={:?}",
                            Utc::now(),
                            env.sequence,
                            env.event
                        );
                    }
                    Err(e) => {
                        println!("DebugSub: recv error ts={} err={:?}", Utc::now(), e);
                        // On lag or closed channel, continue to observe behavior
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
            }
        }));

        tasks.push(tokio::spawn(async move {
            println!("Dispatcher: loop starting (select)");
            loop {
                tokio::select! {
                    biased;
                    // Priority envelopes are handled first whenever present
                    maybe_pri = priority_rx.recv() => {
                        match maybe_pri {
                            Some(env) => {
                                priority_queue_len_dispatcher.fetch_sub(1, Ordering::Relaxed);
                                let rc = tx_for_dispatch.receiver_count();
                                println!("Dispatcher(PRIORITY): send-start ts={} sequence={} receivers={} priority_after={}", Utc::now(), env.sequence, rc, priority_queue_len_dispatcher.load(Ordering::Relaxed));
                                let send_start = std::time::Instant::now();
                                if let Err(e) = tx_for_dispatch.send(env) {
                                    let dur = send_start.elapsed();
                                    println!("Dispatcher(PRIORITY): telemetry send error ts={} error={:?} duration_ms={}", Utc::now(), e, dur.as_millis());
                                } else {
                                    let dur = send_start.elapsed();
                                    println!("Dispatcher(PRIORITY): envelope sent ts={} duration_ms={}", Utc::now(), dur.as_millis());
                                }
                            }
                            None => {
                                println!("Dispatcher: priority_rx closed");
                                // continue to allow normal queue to drain or exit if closed
                            }
                        }
                    }
                    maybe_env = envelope_rx.recv() => {
                        match maybe_env {
                            Some(env) => {
                                let q_before = envelope_queue_len_dispatcher.load(Ordering::Relaxed);
                                envelope_queue_len_dispatcher.fetch_sub(1, Ordering::Relaxed);
                                let q_after = envelope_queue_len_dispatcher.load(Ordering::Relaxed);
                                let rc = tx_for_dispatch.receiver_count();
                                println!("Dispatcher: send-start ts={} sequence={} receivers={} queue_before={} queue_after={}", Utc::now(), env.sequence, rc, q_before, q_after);
                                let send_start = std::time::Instant::now();
                                if let Err(e) = tx_for_dispatch.send(env) {
                                    let dur = send_start.elapsed();
                                    println!("Dispatcher: telemetry send error ts={} error={:?} duration_ms={}", Utc::now(), e, dur.as_millis());
                                } else {
                                    let dur = send_start.elapsed();
                                    println!("Dispatcher: envelope sent ts={} duration_ms={}", Utc::now(), dur.as_millis());
                                }
                            }
                            None => {
                                println!("Dispatcher: envelope_rx closed");
                                break;
                            }
                        }
                    }
                }
            }
            println!("Dispatcher: exiting");
        }));

        Ok(Self {
            app_state,
            ipc_handle,
            capture_handle,
            _tasks: tasks,
            _capture_telemetry_tx: capture_telemetry_tx.clone(),
            _capture_telemetry_keepalive: capture_keepalive,
        })
    }
}
