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
    /// Frame fan-out. **Held for the lifetime of the runtime on purpose.**
    ///
    /// `StreamService::run` exits when its command channel closes, which
    /// happens as soon as the last handle drops. Letting this fall out of
    /// scope at the end of `bootstrap` therefore killed the service the moment
    /// startup finished: subscriber senders dropped, the window's receiver
    /// reported "capture_rx closed", and the preview went blank while the
    /// camera carried on capturing perfectly.
    pub stream_handle: iris_stream::StreamHandle,
    /// Camera controls, when this platform has a backend for them.
    ///
    /// `None` on a platform with no UVC control backend — the UI then says so
    /// rather than showing an empty panel that looks like a camera with no
    /// controls. Those are different facts.
    pub control_handle: Option<iris_control::ControlHandle>,
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
    /// The newest frames, for `GetFrame`.
    ///
    /// The stream service maintains this in every mode, so an agent can ask
    /// what the camera sees at any moment without the pipeline having been
    /// configured in advance for a puller.
    frames: iris_stream::SharedRingBuffer,
}

impl IrisDispatcher {
    pub fn new(
        capture_cmd: MpscSender<CaptureCommand>,
        frames: iris_stream::SharedRingBuffer,
    ) -> Self {
        Self { capture_cmd, frames }
    }
}

impl iris_ipc::Dispatcher for IrisDispatcher {
    fn dispatch(
        &mut self,
        cmd: iris_ipc::command::IpcCommand,
    ) -> Pin<Box<dyn Future<Output = IpcResponse> + Send>> {
        let cmd_sender = self.capture_cmd.clone();
        let frames = self.frames.clone();
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
                    // On Windows, try WMF enumeration first for real USB cameras;
                    // fall back to the mock backend if WMF returns an error or nothing.
                    #[cfg(windows)]
                    let raw_list = {
                        let wmf_result = iris_hal::wmf_backend::wmf::WmfUvcBackend::enumerate_sync();
                        match wmf_result {
                            Ok(list) if !list.is_empty() => {
                                println!("ListDevices: WMF found {} device(s)", list.len());
                                list
                            }
                            Ok(_) => {
                                println!("ListDevices: WMF returned 0 devices, falling back to mock");
                                MockUvcBackend::new().enumerate_devices().await.unwrap_or_default()
                            }
                            Err(e) => {
                                println!("ListDevices: WMF error ({:?}), falling back to mock", e);
                                MockUvcBackend::new().enumerate_devices().await.unwrap_or_default()
                            }
                        }
                    };
                    // On Linux, try V4L2 enumeration first for real USB cameras;
                    // fall back to the mock backend if V4L2 errors or finds nothing.
                    #[cfg(target_os = "linux")]
                    let raw_list = {
                        let v4l2_result =
                            iris_hal::v4l2_backend::v4l2::V4l2UvcBackend::enumerate_sync();
                        match v4l2_result {
                            Ok(list) if !list.is_empty() => {
                                println!("ListDevices: V4L2 found {} device(s)", list.len());
                                list
                            }
                            Ok(_) => {
                                println!("ListDevices: V4L2 returned 0 devices, falling back to mock");
                                MockUvcBackend::new().enumerate_devices().await.unwrap_or_default()
                            }
                            Err(e) => {
                                println!("ListDevices: V4L2 error ({:?}), falling back to mock", e);
                                MockUvcBackend::new().enumerate_devices().await.unwrap_or_default()
                            }
                        }
                    };
                    #[cfg(not(any(windows, target_os = "linux")))]
                    let raw_list = MockUvcBackend::new().enumerate_devices().await.unwrap_or_default();

                    let devices = raw_list
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
                IpcCommand::GetFrame { max_width, quality } => {
                    // Defaults chosen for a vision projector rather than for a
                    // display: these models tile a few hundred pixels square,
                    // so 1080p costs encode time, transfer and tokens to reach
                    // the same tiles. 768 is a common upper bound across
                    // llama.cpp mmproj builds.
                    const DEFAULT_MAX_WIDTH: u32 = 768;
                    const DEFAULT_QUALITY: u8 = 80;

                    let latest = match frames.lock() {
                        Ok(rb) => rb.read_latest().cloned(),
                        Err(_) => None,
                    };
                    match latest {
                        None => IpcResponse::Error {
                            code: 404,
                            message: "no frame captured yet — start capture first".into(),
                        },
                        Some(slot) => {
                            let frame = iris_capture::frame::CaptureFrame {
                                sequence: slot.sequence,
                                width: slot.width,
                                height: slot.height,
                                format: slot.format.clone(),
                                data: slot.data.clone(),
                                timestamp_us: slot.timestamp_us,
                                is_cropped: false,
                            };
                            match iris_capture::snapshot::snapshot(
                                &frame,
                                max_width.unwrap_or(DEFAULT_MAX_WIDTH),
                                quality.unwrap_or(DEFAULT_QUALITY),
                            ) {
                                Ok(snap) => IpcResponse::Ok(ResponseData::Frame {
                                    sequence: slot.sequence,
                                    width: snap.width,
                                    height: snap.height,
                                    captured_us: slot.timestamp_us,
                                    mime: snap.mime.to_string(),
                                    data_url: snap.data_url(),
                                }),
                                Err(e) => IpcResponse::Error {
                                    code: 500,
                                    message: format!("snapshot failed: {e}"),
                                },
                            }
                        }
                    }
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
    /// Start a control service over this platform's UVC backend, if it has one.
    ///
    /// Returns `None` where no control backend exists, so the UI can say
    /// "controls are unavailable on this build" rather than showing an empty
    /// list, which would read as "this camera has no controls".
    fn spawn_control_service(
        config: &iris_core::config::IrisConfig,
        telemetry_tx: broadcast::Sender<iris_ipc::telemetry::TelemetryEnvelope>,
    ) -> Option<iris_control::ControlHandle> {
        let profiles_dir = iris_core::config::IrisConfig::config_search_paths()
            .first()
            .and_then(|p| p.parent().map(|d| d.join("profiles")))
            .unwrap_or_else(|| std::path::PathBuf::from("profiles"));

        // Which device? The configured preference if there is one, else the
        // first the platform enumerates. Enumeration does not require the
        // device to be open.
        #[cfg(target_os = "linux")]
        {
            use iris_hal::v4l2_backend::v4l2::V4l2UvcBackend;
            let devices = V4l2UvcBackend::enumerate_sync().ok()?;
            let chosen = if config.device.preferred_device.is_empty() {
                devices.first()?.id.clone()
            } else {
                devices
                    .iter()
                    .find(|d| d.id.0 == config.device.preferred_device)
                    .or_else(|| devices.first())?
                    .id
                    .clone()
            };
            let backend = std::sync::Arc::new(V4l2UvcBackend::new());
            let (svc, handle) = iris_control::ControlService::new(
                backend,
                chosen.clone(),
                telemetry_tx,
                profiles_dir,
            );
            println!("ControlService: managing controls for {chosen}");
            tokio::spawn(svc.run());
            return Some(handle);
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (config, telemetry_tx, profiles_dir);
            None
        }
    }

    pub async fn bootstrap(config: IrisConfig) -> IrisResult<Self> {
        // 1. App state
        let app_state = AppState::new();

        // 2. IPC server + handle
        // Note: IpcServer::new returns telemetry sender so we can share it with services.
        let (ipc_server, ipc_handle, telemetry_tx) = IpcServer::new(256);

        let hrt_config = HrtConfig::default();
        let (hrt_service, _hrt_handle) = HrtService::new(hrt_config, telemetry_tx.clone());

        // 4. HAL backend and capture service
        //
        // `format` and `drop_policy` are read from `iris.toml`. They used to be
        // hardcoded here — `PixelFormat::Bgr24` and `DropPolicy::Oldest` — while
        // `capture.pixel_format` and `capture.drop_policy` sat in the config
        // struct, were serialised, were range-checked by `IrisConfig::validate`,
        // and were then thrown away. Same shape as the 2026-08-01 finding that
        // `IrisConfig::load()` itself was never called: a configurable mechanism
        // that exists and is not routed through (Article XI §3).
        //
        // Both parse failures fall back to the previous hardcoded value rather
        // than refusing to start, because `main` validates the config first and
        // will already have reported an unusable value.
        let capture_format = iris_hal::device::PixelFormat::from_config_name(
            &config.capture.pixel_format,
        )
        .unwrap_or_else(|| {
            eprintln!(
                "capture.pixel_format '{}' not recognised; using bgr24",
                config.capture.pixel_format
            );
            iris_hal::device::PixelFormat::Bgr24
        });
        let drop_policy: DropPolicy = config.capture.drop_policy.parse().unwrap_or_else(|_| {
            eprintln!(
                "capture.drop_policy '{}' not recognised; using oldest",
                config.capture.drop_policy
            );
            DropPolicy::Oldest
        });
        println!(
            "Capture config: {}x{} @{} fps format={} drop_policy={:?} queue_depth={}",
            config.capture.width,
            config.capture.height,
            config.capture.target_fps,
            capture_format.config_name(),
            drop_policy,
            config.capture.max_queue_depth,
        );
        let capture_cfg = CaptureConfig {
            width: config.capture.width,
            height: config.capture.height,
            target_fps: config.capture.target_fps,
            format: capture_format,
            max_queue_depth: config.capture.max_queue_depth,
            drop_policy,
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
        // IRIS_BACKEND: "mock" (default) | "dxgi" (Windows screen) | "wmf"
        // (Windows camera) | "v4l2" (Linux camera).
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
            } else if backend_name == "wmf" {
                #[cfg(windows)]
                {
                    // Initialize COM/MF on a dedicated keeper thread (parked for
                    // the process lifetime) so the MAIN thread stays free for
                    // winit's STA OleInitialize — MTA on main panics the window
                    // with RPC_E_CHANGED_MODE.
                    let (wmf_tx, wmf_rx) = std::sync::mpsc::channel();
                    let _ = std::thread::Builder::new()
                        .name("wmf-com-keeper".into())
                        .spawn(move || {
                            let created = iris_hal::backend::new_wmf_backend();
                            let keep_alive = created.is_ok();
                            let _ = wmf_tx.send(created);
                            if keep_alive {
                                loop {
                                    std::thread::park();
                                }
                            }
                        });
                    match wmf_rx.recv() {
                        Ok(Ok(wmf)) => Box::new(iris_capture::backend::UvcCaptureBackend::new(
                            wmf,
                            capture_cfg.clone(),
                        )),
                        other => {
                            let why = match other {
                                Ok(Err(e)) => format!("{e:?}"),
                                Err(e) => format!("channel: {e:?}"),
                                Ok(Ok(_)) => unreachable!(),
                            };
                            println!("IRIS_BACKEND=wmf init failed ({why}); falling back to mock");
                            Box::new(iris_capture::backend::MockCaptureBackend::new(
                                capture_cfg.clone(),
                            ))
                        }
                    }
                }
                #[cfg(not(windows))]
                {
                    println!("IRIS_BACKEND=wmf is Windows-only; falling back to mock");
                    Box::new(iris_capture::backend::MockCaptureBackend::new(
                        capture_cfg.clone(),
                    ))
                }
            } else if backend_name == "v4l2" {
                #[cfg(target_os = "linux")]
                {
                    Box::new(iris_capture::backend::UvcCaptureBackend::new(
                        iris_hal::v4l2_backend::v4l2::V4l2UvcBackend::new(),
                        capture_cfg.clone(),
                    ))
                }
                #[cfg(not(target_os = "linux"))]
                {
                    println!("IRIS_BACKEND=v4l2 is Linux-only; falling back to mock");
                    Box::new(iris_capture::backend::MockCaptureBackend::new(
                        capture_cfg.clone(),
                    ))
                }
            } else if backend_name == "mock" {
                println!("IRIS_BACKEND=mock: using the synthetic backend");
                Box::new(iris_capture::backend::MockCaptureBackend::new(
                    capture_cfg.clone(),
                ))
            } else {
                // NO IRIS_BACKEND SET — use the camera if there is one.
                //
                // This defaulted to the mock backend, which fills every byte
                // with 128. So Iris launched normally — from the desktop, with
                // no environment set — showed a **uniform grey rectangle**
                // instead of the camera, and looked like a broken preview
                // rather than a synthetic one. The camera only appeared if you
                // knew to export IRIS_BACKEND=v4l2, which nothing tells you.
                //
                // A camera application defaults to the camera. The mock stays
                // available, explicitly, as IRIS_BACKEND=mock.
                #[cfg(target_os = "linux")]
                {
                    use iris_hal::v4l2_backend::v4l2::V4l2UvcBackend;
                    let have_camera = matches!(
                        V4l2UvcBackend::enumerate_sync(),
                        Ok(ref devices) if !devices.is_empty()
                    );
                    if have_camera {
                        println!("Capture backend: v4l2 (a camera is present)");
                        Box::new(iris_capture::backend::UvcCaptureBackend::new(
                            V4l2UvcBackend::new(),
                            capture_cfg.clone(),
                        ))
                    } else {
                        println!(
                            "Capture backend: mock — no /dev/video* camera found. \
                             The preview will be a flat grey test image, not a fault."
                        );
                        Box::new(iris_capture::backend::MockCaptureBackend::new(
                            capture_cfg.clone(),
                        ))
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    println!("Capture backend: mock (no default camera backend on this platform)");
                    Box::new(iris_capture::backend::MockCaptureBackend::new(
                        capture_cfg.clone(),
                    ))
                }
            };

        let (capture_service, capture_handle) = CaptureService::new(
            boxed_backend,
            capture_cfg.clone(),
            capture_telemetry_tx.clone(),
        );

        // The stream service sits BETWEEN capture and everything that wants
        // frames, which is what lets there be more than one such thing.
        //
        // Capture's channel has exactly one consumer. Before this, that
        // consumer was the window, so a frame could be displayed or handed to
        // an agent but not both. Now the window is an ordinary subscriber and
        // the ring is the pull surface an agent reads through `GetFrame`.
        //
        // Push mode, because the window wants every frame as it arrives; the
        // ring is maintained regardless of mode, so the agent's pull path does
        // not depend on that choice.
        let stream_mode: iris_stream::StreamMode = config
            .stream
            .default_mode
            .parse()
            .unwrap_or(iris_stream::StreamMode::Push);
        let stream_mode = if stream_mode.is_implemented() {
            stream_mode
        } else {
            eprintln!(
                "stream.default_mode = {:?} is not implemented; using push",
                config.stream.default_mode
            );
            iris_stream::StreamMode::Push
        };
        let mut capture_handle = capture_handle;
        // Take the raw receiver out and leave a closed one behind; the window's
        // real source is installed below once it has subscribed.
        let (_placeholder_tx, placeholder_rx) = mpsc::channel(1);
        let raw_frames = capture_handle.swap_frame_rx(placeholder_rx);
        let (stream_service, stream_handle) = iris_stream::StreamService::new(
            raw_frames,
            telemetry_tx.clone(),
            stream_mode,
            config.stream.ring_buffer_capacity.max(2),
            config.stream.max_subscribers.max(1),
        );
        if stream_mode == iris_stream::StreamMode::Pull {
            // Not overridden — the config is honoured — but said plainly,
            // because the symptom is a permanently empty preview with a
            // perfectly healthy camera behind it.
            eprintln!(
                "stream.default_mode = \"pull\": frames go to the ring only, so the \
                 window will show nothing. Use \"push\" for a windowed run; \
                 \"pull\" suits a headless agent-only one."
            );
        }
        let frames_ring = stream_handle.ring_buffer.clone();
        tokio::spawn(stream_service.run());
        println!(
            "StreamService: {} mode, ring {} frames, up to {} subscribers",
            stream_mode, config.stream.ring_buffer_capacity, config.stream.max_subscribers
        );

        // Hand the window a subscription in place of the raw capture channel.
        let ui_frames = stream_handle
            .subscribe()
            .await
            .map(|sub| sub.into_receiver());
        match ui_frames {
            Ok(rx) => {
                capture_handle.swap_frame_rx(rx);
            }
            Err(e) => {
                eprintln!("could not subscribe the UI to the stream service: {e}");
                return Err(iris_core::error::IrisError::Stream(format!("{e}")));
            }
        }

        // create the capture command channel that will be used by the dispatcher
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        // 5. Dispatcher
        let dispatcher = IrisDispatcher::new(cmd_tx.clone(), frames_ring.clone());

        // 6. Spawn services
        let mut tasks = Vec::new();

        // Spawn a minimal HTTP server to expose /metrics and a debug endpoint
        // `/debug/force_rebase` which triggers an in-process rebase increment.
        // Binding address can be configured via METRICS_BIND (default 127.0.0.1:9180).
        let metrics_bind = std::env::var("METRICS_BIND").unwrap_or_else(|_| "127.0.0.1:9180".to_string());
        if let Ok(addr) = metrics_bind.parse::<SocketAddr>() {
            // `/frame` is served from the same listener as `/metrics`.
            //
            // The consumer this exists for is a local llama.cpp model driven
            // through the OpenAI chat-completions API — which is HTTP and JSON
            // already. Giving it a second transport to learn (a unix socket, a
            // named pipe, a bespoke framing) would be a worse interface for the
            // one caller it has, and this listener is already here.
            let http_frames = frames_ring.clone();
            let svc = make_service_fn(move |_conn| {
                let http_frames = http_frames.clone();
                async move {
                    let http_frames = http_frames.clone();
                    Ok::<_, Infallible>(service_fn(move |req: Request<Body>| {
                        let http_frames = http_frames.clone();
                        async move {
                    match req.uri().path() {
                        "/frame" => {
                            let q = req.uri().query().unwrap_or("");
                            let param = |k: &str| -> Option<u32> {
                                q.split('&')
                                    .filter_map(|kv| kv.split_once('='))
                                    .find(|(name, _)| *name == k)
                                    .and_then(|(_, v)| v.parse().ok())
                            };
                            let max_width = param("max_width").unwrap_or(768);
                            let quality = param("quality").unwrap_or(80).clamp(1, 100) as u8;

                            let latest = match http_frames.lock() {
                                Ok(rb) => rb.read_latest().cloned(),
                                Err(_) => None,
                            };
                            let (status, body) = match latest {
                                None => (
                                    503,
                                    "{\"error\":\"no frame captured yet — is capture running?\"}"
                                        .to_string(),
                                ),
                                Some(slot) => {
                                    let frame = iris_capture::frame::CaptureFrame {
                                        sequence: slot.sequence,
                                        width: slot.width,
                                        height: slot.height,
                                        format: slot.format.clone(),
                                        data: slot.data.clone(),
                                        timestamp_us: slot.timestamp_us,
                                        is_cropped: false,
                                    };
                                    match iris_capture::snapshot::snapshot(
                                        &frame, max_width, quality,
                                    ) {
                                        Ok(snap) => (
                                            200,
                                            format!(
                                                concat!(
                                                    "{{\"sequence\":{},\"width\":{},",
                                                    "\"height\":{},\"captured_us\":{},",
                                                    "\"mime\":\"{}\",\"data_url\":\"{}\"}}"
                                                ),
                                                slot.sequence,
                                                snap.width,
                                                snap.height,
                                                slot.timestamp_us,
                                                snap.mime,
                                                snap.data_url()
                                            ),
                                        ),
                                        Err(e) => (
                                            500,
                                            format!("{{\"error\":\"snapshot failed: {e}\"}}"),
                                        ),
                                    }
                                }
                            };
                            Ok::<_, Infallible>(
                                Response::builder()
                                    .status(status)
                                    .header("content-type", "application/json")
                                    .body(Body::from(body))
                                    .unwrap(),
                            )
                        }
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
                        }
                    }))
                }
            });
            tokio::spawn(async move {
                // `Server::bind` PANICS when the address is taken, and the
                // release profile is `panic = "abort"`, so a busy port killed
                // the whole application — camera, UI and all — over an
                // auxiliary metrics endpoint. Observed 2026-08-31 while
                // verifying the .deb:
                //
                //   thread 'tokio-rt-worker' panicked at hyper server.rs:81:
                //   error binding to 127.0.0.1:9180: Address already in use
                //   Aborted (core dumped)
                //
                // It also meant two instances of Iris could never run at once,
                // and that any unrelated program holding 9180 stopped Iris from
                // starting at all — with the failure surfacing as an abort
                // rather than as a message about a port.
                //
                // `try_bind` returns the error instead. Metrics are a
                // diagnostic, so losing them degrades the app; it must not end
                // it. Anything scraping /metrics still finds out, by the
                // endpoint being absent.
                match Server::try_bind(&addr) {
                    Ok(builder) => {
                        if let Err(e) = builder.serve(svc).await {
                            eprintln!("metrics endpoint on {addr} stopped: {e}");
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "metrics endpoint disabled — cannot bind {addr}: {e}"
                        );
                        eprintln!(
                            "  (set METRICS_BIND to another address, or stop whatever holds it; \
                             Iris continues without /metrics)"
                        );
                    }
                }
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
                        // Closed = capture service is gone; exit instead of
                        // busy-spinning on a dead channel.
                        println!("Forwarder: recv error: {:?}; exiting", e);
                        break;
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

        // Camera controls. A SEPARATE backend instance from the capture path on
        // purpose: capture holds the device streaming, and V4L2 permits a
        // second open for control ioctls on its own fd. Sharing one instance
        // would mean the control service and the capture loop contending for
        // the same handle.
        //
        // The device is opened by the service itself when it starts, and the
        // profiles directory sits beside the config so a profile is findable
        // next to the settings it belongs with.
        let control_handle = Self::spawn_control_service(&config, telemetry_tx.clone());

        Ok(Self {
            app_state,
            ipc_handle,
            capture_handle,
            stream_handle,
            control_handle,
            _tasks: tasks,
            _capture_telemetry_tx: capture_telemetry_tx.clone(),
            _capture_telemetry_keepalive: capture_keepalive,
        })
    }
}
