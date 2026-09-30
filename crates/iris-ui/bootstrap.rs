use chrono::Utc;
use iris_capture::backend::{CaptureConfig, DropPolicy, OpenedCamera};
use iris_capture::service::{CaptureCommand, CaptureHandle, CaptureService, CaptureServiceState};
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
    /// Mirror the image left-to-right.
    ///
    /// Shared, not per-consumer: a webcam's mirrored view puts text held up to
    /// it backwards, and the reason to flip is so the **model** can read it.
    /// A toggle that only flipped the window would look like it worked and
    /// change nothing about what the agent receives.
    pub mirror: std::sync::Arc<std::sync::atomic::AtomicBool>,
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

/// What `ListDevices` offers, given what enumeration found.
///
/// With no camera this used to offer the mock's "Mock Camera". The window then
/// selected it, started capture, and showed flat grey frames as though they
/// were a picture, and agents received the same frames as real ones (operator
/// ruling, 2026-09-29). Now: real cameras if there are any; the synthetic
/// device only when `IRIS_BACKEND=mock` asked for it; otherwise nothing.
#[derive(Debug)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) enum Offer<T> {
    Real(Vec<T>),
    Synthetic,
    Nothing,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn offer<T, E>(found: Result<Vec<T>, E>, synthetic_requested: bool) -> Offer<T> {
    match found {
        Ok(list) if !list.is_empty() => Offer::Real(list),
        _ if synthetic_requested => Offer::Synthetic,
        _ => Offer::Nothing,
    }
}

/// `IRIS_BACKEND=mock`: the one way to ask for the synthetic camera.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn synthetic_requested() -> bool {
    std::env::var("IRIS_BACKEND")
        .map(|v| v.eq_ignore_ascii_case("mock"))
        .unwrap_or(false)
}

/// The capture backend on Linux with no `IRIS_BACKEND` set: the camera if there
/// is one, otherwise `NoCameraBackend`, never the synthetic mock.
#[cfg(target_os = "linux")]
pub(crate) fn default_linux_backend(
    have_camera: bool,
    cfg: CaptureConfig,
) -> Box<dyn iris_capture::backend::CaptureBackend + Send + Sync> {
    if have_camera {
        println!("Capture backend: v4l2 (a camera is present)");
        Box::new(iris_capture::backend::UvcCaptureBackend::new(
            iris_hal::v4l2_backend::v4l2::V4l2UvcBackend::new(),
            cfg,
        ))
    } else {
        println!(
            "Capture backend: none, {} (no /dev/video*). The window says so, and \
             no test image is sent anywhere; IRIS_BACKEND=mock gives the synthetic one.",
            iris_capture::backend::NO_CAMERA
        );
        Box::new(iris_capture::backend::NoCameraBackend)
    }
}

// --- GetStatus ----------------------------------------------------------------
// Every field GetStatus returned was invented: device "Mock Camera" whatever
// was in use (a real camera included), an empty capture state, and 0 fps, 0
// frames and 0 subscribers however busy capture was. An agent asking "what is
// Iris looking at" was told a fiction. Each field now comes from the thing it
// describes.

/// The capture service's state, as GetStatus reports it.
pub(crate) fn capture_state_text(state: &CaptureServiceState) -> String {
    match state {
        CaptureServiceState::Idle => "Idle".into(),
        CaptureServiceState::Capturing => "Capturing".into(),
        CaptureServiceState::Paused => "Paused".into(),
        CaptureServiceState::Error(e) => format!("Error: {e}"),
    }
}

/// Measured frames per second over the frames the ring holds, and how many it
/// has taken in. The rate is 0 when fewer than two frames are held or the
/// newest is over a second old (nothing is arriving), never an estimate.
pub(crate) fn ring_rate(rb: &iris_stream::RingBuffer, now_us: u64) -> (f64, u64) {
    let total = rb.total_written();
    let held = rb.len();
    let (Some(oldest), Some(newest)) = (rb.read_by_age(0), rb.read_latest()) else {
        return (0.0, total);
    };
    if held < 2
        || newest.timestamp_us <= oldest.timestamp_us
        || now_us.saturating_sub(newest.timestamp_us) > 1_000_000
    {
        return (0.0, total);
    }
    let secs = (newest.timestamp_us - oldest.timestamp_us) as f64 / 1_000_000.0;
    ((held - 1) as f64 / secs, total)
}

/// What GetStatus names as the capture source, given `IRIS_BACKEND` and the
/// cameras enumeration found. Mirrors the backend selection in `bootstrap`,
/// including where it falls back to the synthetic source.
pub(crate) fn capture_source_label(backend_name: &str, cameras: &[String]) -> String {
    let synthetic = |why: &str| format!("synthetic test source ({why})");
    match backend_name {
        "mock" => synthetic("IRIS_BACKEND=mock"),
        "dxgi" if cfg!(windows) => "screen capture (IRIS_BACKEND=dxgi)".into(),
        "dxgi" => synthetic("IRIS_BACKEND=dxgi is Windows-only"),
        "wmf" if !cfg!(windows) => synthetic("IRIS_BACKEND=wmf is Windows-only"),
        "v4l2" if !cfg!(target_os = "linux") => synthetic("IRIS_BACKEND=v4l2 is Linux-only"),
        // The Windows default path still falls back to the mock with no camera
        // (Linux uses NoCameraBackend); say so rather than "none".
        _ if cameras.is_empty() && cfg!(windows) => synthetic("no camera found"),
        _ => match cameras.first() {
            Some(name) => name.clone(),
            None => format!("none: {}", iris_capture::backend::NO_CAMERA),
        },
    }
}

// --- answers that are true ----------------------------------------------------
// Beyond GetStatus (above), the dispatcher answered Ping with an uptime of 0,
// Subscribe with id 1 every time, GetDeviceCapabilities with "unknown",
// GetStreamStats with zeros, and SelectDevice, the profile, config and mode
// commands and every unlisted command with a bare "ok" while doing nothing. The
// window's Select button moved its "selected" mark on that "ok", whichever camera
// capture was actually reading. Each answer now says what happened.

/// What a command that is not implemented answers, instead of a bare "ok".
pub(crate) fn not_implemented(what: &str) -> IpcResponse {
    IpcResponse::Error { code: 501, message: format!("{what} is not implemented") }
}

/// Capture has ended (StopCapture ends it for good) or never started.
pub(crate) const CAPTURE_NOT_RUNNING: &str =
    "capture is not running (it was stopped, or never started: see GetStatus); restart Iris to capture again";

/// Pass a command to the capture service, and say so when nothing is listening.
async fn tell_capture(tx: &MpscSender<CaptureCommand>, cmd: CaptureCommand) -> IpcResponse {
    match tx.send(cmd).await {
        Ok(()) => IpcResponse::Ok(ResponseData::Empty),
        Err(_) => IpcResponse::Error { code: 503, message: CAPTURE_NOT_RUNNING.into() },
    }
}

/// SelectDevice: "ok" only for the camera capture has open. Switching cameras
/// needs capture to restart, which it cannot yet (ROADMAP.md).
pub(crate) fn select_device_answer(open: Option<&OpenedCamera>, wanted: &str) -> IpcResponse {
    match open {
        Some(cam) if cam.id == wanted => IpcResponse::Ok(ResponseData::Empty),
        Some(cam) => IpcResponse::Error {
            code: 501,
            message: format!(
                "switching cameras is not implemented: Iris is capturing from {} ({}). \
                 Start Iris with IRIS_DEVICE={wanted} to use that camera.",
                cam.name, cam.id
            ),
        },
        None => IpcResponse::Error { code: 503, message: "no camera is open, so none can be selected".into() },
    }
}

/// How long SelectDevice waits for capture to open its camera. The window asks
/// as soon as it has listed the cameras, which can be before capture has opened
/// one. A capture that has ended does not make it wait: its sender is gone.
const SELECT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Reads a camera's formats, as JSON. `None` where no probe exists.
pub(crate) type CapabilityProbe = fn(&str) -> Result<String, String>;

#[cfg(target_os = "linux")]
fn v4l2_capabilities(id: &str) -> Result<String, String> {
    let caps = iris_hal::v4l2_backend::v4l2::V4l2UvcBackend::probe_capabilities_sync(
        &iris_hal::device::DeviceId(id.into()),
    )
    .map_err(|e| e.to_string())?;
    serde_json::to_string(&caps).map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
const CAPABILITY_PROBE: Option<CapabilityProbe> = Some(v4l2_capabilities);
#[cfg(not(target_os = "linux"))]
const CAPABILITY_PROBE: Option<CapabilityProbe> = None;

/// Minimal dispatcher implemented inside `iris-ui` that implements the `iris-ipc::Dispatcher` trait.
pub struct IrisDispatcher {
    capture_cmd: MpscSender<CaptureCommand>,
    /// The capture service's state, for GetStatus.
    capture_state: tokio::sync::watch::Receiver<CaptureServiceState>,
    /// Stream statistics (subscriber count), for GetStatus.
    stream: iris_stream::StreamHandle,
    /// What is being captured from, for GetStatus (see `capture_source_label`),
    /// when no camera is open to name.
    source: String,
    /// The camera capture has open, for SelectDevice, GetStatus and
    /// GetDeviceCapabilities.
    device: tokio::sync::watch::Receiver<Option<OpenedCamera>>,
    /// When the dispatcher started, for Ping.
    started: std::time::Instant,
    /// Reads the open camera's formats, for GetDeviceCapabilities.
    probe: Option<CapabilityProbe>,
    /// Shared mirror flag, so `GetFrame` matches what the window shows.
    mirror: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
        mirror: std::sync::Arc<std::sync::atomic::AtomicBool>,
        capture_state: tokio::sync::watch::Receiver<CaptureServiceState>,
        stream: iris_stream::StreamHandle,
        source: String,
        device: tokio::sync::watch::Receiver<Option<OpenedCamera>>,
    ) -> Self {
        Self {
            capture_cmd,
            capture_state,
            stream,
            source,
            device,
            started: std::time::Instant::now(),
            probe: CAPABILITY_PROBE,
            frames,
            mirror,
        }
    }

    /// The same dispatcher with another capability probe (tests).
    #[cfg(test)]
    pub(crate) fn with_probe(mut self, probe: Option<CapabilityProbe>) -> Self {
        self.probe = probe;
        self
    }
}

impl iris_ipc::Dispatcher for IrisDispatcher {
    fn dispatch(
        &mut self,
        cmd: iris_ipc::command::IpcCommand,
    ) -> Pin<Box<dyn Future<Output = IpcResponse> + Send>> {
        let cmd_sender = self.capture_cmd.clone();
        let frames = self.frames.clone();
        let mirror = self.mirror.clone();
        let capture_state = capture_state_text(&self.capture_state.borrow());
        let stream = self.stream.clone();
        let source = self.source.clone();
        let mut device = self.device.clone();
        let open_name = device.borrow().as_ref().map(|c| c.name.clone());
        let started = self.started;
        let probe = self.probe;
        Box::pin(async move {
            use iris_ipc::command::IpcCommand;
            match cmd {
                IpcCommand::Ping => IpcResponse::Ok(ResponseData::Pong {
                    uptime_ms: started.elapsed().as_millis() as u64,
                }),
                IpcCommand::GetStatus => {
                    let (fps, frame_count) = match frames.lock() {
                        Ok(rb) => ring_rate(&rb, iris_capture::frame::CaptureFrame::now_us()),
                        Err(_) => (0.0, 0),
                    };
                    let subscriber_count = match stream.stats().await {
                        Ok(s) => s.subscriber_count,
                        Err(_) => 0,
                    };
                    let status = ResponseData::Status {
                        capture_state,
                        // The camera actually open, which IRIS_DEVICE may have
                        // chosen over the first one found.
                        device_name: open_name.unwrap_or(source),
                        fps,
                        frame_count,
                        subscriber_count,
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
                    // On Linux, list the V4L2 cameras. With none, the list is
                    // EMPTY (the window then says "No camera found"); the
                    // synthetic device is offered only when IRIS_BACKEND=mock
                    // asked for it. See `offer`.
                    #[cfg(target_os = "linux")]
                    let raw_list = {
                        let v4l2_result =
                            iris_hal::v4l2_backend::v4l2::V4l2UvcBackend::enumerate_sync();
                        if let Err(e) = &v4l2_result {
                            println!("ListDevices: V4L2 error ({:?})", e);
                        }
                        match offer(v4l2_result, synthetic_requested()) {
                            Offer::Real(list) => {
                                println!("ListDevices: V4L2 found {} device(s)", list.len());
                                list
                            }
                            Offer::Synthetic => {
                                println!("ListDevices: IRIS_BACKEND=mock, offering the synthetic device");
                                MockUvcBackend::new().enumerate_devices().await.unwrap_or_default()
                            }
                            Offer::Nothing => {
                                println!("ListDevices: {}", iris_capture::backend::NO_CAMERA);
                                Vec::new()
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
                IpcCommand::ResumeCapture => tell_capture(&cmd_sender, CaptureCommand::Resume).await,
                IpcCommand::PauseCapture => tell_capture(&cmd_sender, CaptureCommand::Pause).await,
                IpcCommand::SetFps { fps } => tell_capture(&cmd_sender, CaptureCommand::SetFps(fps)).await,
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
                    tell_capture(&cmd_sender, CaptureCommand::SetRoi(Some(roi))).await
                }
                IpcCommand::ClearRoi => tell_capture(&cmd_sender, CaptureCommand::SetRoi(None)).await,
                IpcCommand::SetResolution { .. } | IpcCommand::SetPixelFormat { .. } => not_implemented(
                    "changing the capture mode while Iris runs (set capture.width, capture.height or \
                     capture.pixel_format in the config and restart Iris)",
                ),
                IpcCommand::SelectDevice { device_id } => {
                    let _ = tokio::time::timeout(SELECT_WAIT, device.wait_for(|d| d.is_some())).await;
                    let open = device.borrow().clone();
                    select_device_answer(open.as_ref(), &device_id)
                }
                IpcCommand::GetDeviceCapabilities => {
                    let open = device.borrow().clone();
                    match (open, probe) {
                        (None, _) => IpcResponse::Error {
                            code: 503,
                            message: "no camera is open to probe".into(),
                        },
                        (Some(_), None) => not_implemented("probing a camera's formats on this platform"),
                        (Some(cam), Some(probe)) => {
                            let id = cam.id.clone();
                            match tokio::task::spawn_blocking(move || probe(&id)).await {
                                Ok(Ok(json)) => IpcResponse::Ok(ResponseData::DeviceCapabilities { capabilities: json }),
                                Ok(Err(e)) => IpcResponse::Error {
                                    code: 500,
                                    message: format!("probing {} ({}) failed: {e}", cam.name, cam.id),
                                },
                                Err(e) => IpcResponse::Error { code: 500, message: format!("the probe did not finish: {e}") },
                            }
                        }
                    }
                }
                IpcCommand::Subscribe | IpcCommand::Unsubscribe { .. } => {
                    not_implemented("a frame subscription over IPC (GetFrame returns the newest frame)")
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
                                mirror.load(std::sync::atomic::Ordering::Relaxed),
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
                IpcCommand::GetStreamStats => match stream.stats().await {
                    Ok(s) => IpcResponse::Ok(ResponseData::StreamStats {
                        frames_delivered: s.frames_delivered,
                        frames_dropped: s.frames_dropped,
                        subscriber_count: s.subscriber_count,
                        ring_buffer_usage: s.ring_buffer_usage,
                    }),
                    Err(e) => IpcResponse::Error { code: 503, message: format!("stream statistics are unavailable: {e}") },
                },
                IpcCommand::GetConfig | IpcCommand::ReloadConfig | IpcCommand::UpdateConfig { .. } => {
                    not_implemented("configuration over IPC (edit the config file and restart Iris)")
                }
                IpcCommand::LoadProfile { .. } | IpcCommand::SaveProfile { .. } => {
                    not_implemented("camera profiles over IPC (use Camera controls in the Iris window)")
                }
                IpcCommand::StartCapture => tell_capture(&cmd_sender, CaptureCommand::Resume).await,
                // StopCapture ends capture for good: nothing restarts it yet (ROADMAP.md).
                IpcCommand::StopCapture => tell_capture(&cmd_sender, CaptureCommand::Stop).await,
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
                other => {
                    let name = format!("{other:?}");
                    not_implemented(name.split([' ', '{', '(']).next().unwrap_or("this command"))
                }
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
        // Select capture backend at runtime. `IRIS_BACKEND` forces a specific
        // one; with nothing set, Iris uses the camera if there is one and says
        // so. With no camera it uses `NoCameraBackend` on Linux and says so
        // (`default_linux_backend`); the Windows branch below still falls back
        // to the mock, a follow-up for the Windows fork.
        // Increase capture telemetry capacity to reduce lag/dropped messages
        // during high-frequency or bursty capture periods in tests.
        let (capture_telemetry_tx, _capture_telemetry_rx) = broadcast::channel(4096);
        let backend_name = std::env::var("IRIS_BACKEND")
            .unwrap_or_default()
            .to_lowercase();
        // The cameras present, by name, for GetStatus's source label.
        let cameras: Vec<String> = {
            #[cfg(target_os = "linux")]
            {
                iris_hal::v4l2_backend::v4l2::V4l2UvcBackend::enumerate_sync()
                    .map(|list| list.into_iter().map(|d| d.name).collect())
                    .unwrap_or_default()
            }
            #[cfg(windows)]
            {
                iris_hal::wmf_backend::wmf::WmfUvcBackend::enumerate_sync()
                    .map(|list| list.into_iter().map(|d| d.name).collect())
                    .unwrap_or_default()
            }
            #[cfg(not(any(windows, target_os = "linux")))]
            {
                Vec::new()
            }
        };
        let source_label = capture_source_label(&backend_name, &cameras);
        println!("Capture source: {source_label}");

        // Box up a dynamic backend so `CaptureService` can be instantiated at runtime.
        // IRIS_BACKEND: unset (the camera, if present) | "mock" (synthetic) |
        // "dxgi" (Windows screen) | "wmf" (Windows camera) | "v4l2" (Linux camera).
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
                    default_linux_backend(have_camera, capture_cfg.clone())
                }
                #[cfg(windows)]
                {
                    // The same treatment as the Linux branch above: ask whether
                    // a camera exists, use it if so, and say which backend was
                    // chosen either way. Windows landed on the mock
                    // unconditionally before this, so a normal desktop launch
                    // showed the grey rectangle described above.
                    //
                    // Enumeration and capture take different routes on purpose.
                    // `WmfUvcBackend::enumerate_sync` initialises COM, lists the
                    // devices and tears COM down inside the one call, so it is
                    // safe to ask from this thread. `backend::WmfBackend` is the
                    // opposite: it owns thread-scoped COM/MF state
                    // (`CoInitializeEx` + `MFStartup` in `new`, `MFShutdown` +
                    // `CoUninitialize` in `Drop`), so it is built on a dedicated
                    // parked thread exactly as the `IRIS_BACKEND=wmf` branch
                    // does. It must not be built on the main thread: winit needs
                    // that one free for its STA `OleInitialize`, and an MTA main
                    // panics the window with `RPC_E_CHANGED_MODE`.
                    use iris_hal::wmf_backend::wmf::WmfUvcBackend;
                    let have_camera = matches!(
                        WmfUvcBackend::enumerate_sync(),
                        Ok(ref devices) if !devices.is_empty()
                    );
                    if have_camera {
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
                            Ok(Ok(wmf)) => {
                                println!("Capture backend: wmf (a camera is present)");
                                Box::new(iris_capture::backend::UvcCaptureBackend::new(
                                    wmf,
                                    capture_cfg.clone(),
                                ))
                            }
                            // A camera was listed but Media Foundation would not
                            // start. Falling back keeps Iris usable, but the
                            // message must not read like "no camera found" —
                            // that would send someone to check their cable for a
                            // software fault.
                            other => {
                                let why = match other {
                                    Ok(Err(e)) => format!("{e:?}"),
                                    Err(e) => format!("channel: {e:?}"),
                                    Ok(Ok(_)) => unreachable!(),
                                };
                                println!(
                                    "Capture backend: mock — a camera is present but Media \
                                     Foundation would not start ({why}). The preview will be a \
                                     flat grey test image, not a fault."
                                );
                                Box::new(iris_capture::backend::MockCaptureBackend::new(
                                    capture_cfg.clone(),
                                ))
                            }
                        }
                    } else {
                        println!(
                            "Capture backend: mock — no camera found. \
                             The preview will be a flat grey test image, not a fault."
                        );
                        Box::new(iris_capture::backend::MockCaptureBackend::new(
                            capture_cfg.clone(),
                        ))
                    }
                }
                #[cfg(not(any(target_os = "linux", windows)))]
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
        // `[telemetry]` from the config, which reached nothing before 2026-09-04:
        // `enabled` and `max_events_per_second` were serialised, documented and
        // validated, and no code outside `iris-core` read either. Setting
        // `enabled = false` left telemetry running at full rate.
        let mut stream_service = stream_service;
        stream_service.set_telemetry_policy(
            config.telemetry.enabled,
            config.telemetry.max_events_per_second,
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
        // One flag, shared by the window, the IPC dispatcher and the HTTP
        // endpoint, so a mirror toggled in the UI is the image the agent gets.
        let mirror = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
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
        let dispatcher = IrisDispatcher::new(
            cmd_tx.clone(),
            frames_ring.clone(),
            mirror.clone(),
            capture_handle.state_rx.clone(),
            stream_handle.clone(),
            source_label.clone(),
            capture_handle.device_rx.clone(),
        );

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
        let http_mirror = mirror.clone();
            let svc = make_service_fn(move |_conn| {
                let http_frames = http_frames.clone();
                let http_mirror = http_mirror.clone();
                async move {
                    let http_frames = http_frames.clone();
                    let http_mirror = http_mirror.clone();
                    Ok::<_, Infallible>(service_fn(move |req: Request<Body>| {
                        let http_frames = http_frames.clone();
                        let http_mirror = http_mirror.clone();
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
                            // The guard is for a DEAD feed, not a slow one.
                            //
                            // A stale frame is the one real hazard here: if
                            // capture has stopped, the ring still holds an image
                            // from minutes ago, and a model handed that will
                            // describe it with complete confidence and nothing
                            // in the answer to say otherwise.
                            //
                            // But latency is expected and normal — frames are
                            // sampled from a feed, not delivered one for one —
                            // so the limit is deliberately generous. Ten seconds
                            // catches "capture is not running" while never
                            // tripping on a feed that is merely slow. Every
                            // answer carries `age_ms` regardless, so a caller
                            // that wants to be stricter can be, and
                            // max_age_ms=0 turns the check off entirely.
                            let max_age_ms = param("max_age_ms").unwrap_or(10_000);
                            // Refuse a frame that carries no information, in
                            // tenths of a unit of luma standard deviation so
                            // the knob stays an integer like the others.
                            //
                            // Default 5 (= 0.5). Chosen from measurement, not
                            // intuition: a live frame from a **dark room** on
                            // this box measured 3.17 and is a perfectly real
                            // picture, while the synthetic frame measured
                            // 0.000. Anything higher starts refusing real
                            // pictures of dim scenes. What is provable here is
                            // the *absence* of variation, not the presence of a
                            // subject — so this catches the blank frame and
                            // deliberately nothing more.
                            //
                            // `min_detail=0` disables it, as `max_age_ms=0`
                            // disables the staleness check.
                            let min_detail_tenths = param("min_detail").unwrap_or(5);

                            let latest = match http_frames.lock() {
                                Ok(rb) => rb.read_latest().cloned(),
                                Err(_) => None,
                            };
                            let age_ms = latest.as_ref().map(|slot| {
                                iris_capture::frame::CaptureFrame::now_us()
                                    .saturating_sub(slot.timestamp_us)
                                    / 1_000
                            });
                            let latest = match (latest, age_ms) {
                                (Some(slot), Some(age)) if max_age_ms == 0 || age <= max_age_ms as u64 => Some(slot),
                                (Some(_), Some(age)) => {
                                    return Ok::<_, Infallible>(
                                        Response::builder()
                                            .status(503)
                                            .header("content-type", "application/json")
                                            .body(Body::from(format!(
                                                concat!(
                                                    "{{\"error\":\"the newest frame is {}ms old ",
                                                    "(limit {}ms) — is capture running?\",",
                                                    "\"age_ms\":{}}}"
                                                ),
                                                age, max_age_ms, age
                                            )))
                                            .unwrap(),
                                    );
                                }
                                _ => None,
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
                                    // The window's toggle, unless the caller
                                    // overrides it for this one request.
                                    let mirror = param("mirror")
                                        .map(|v| v != 0)
                                        .unwrap_or_else(|| {
                                            http_mirror.load(
                                                std::sync::atomic::Ordering::Relaxed,
                                            )
                                        });
                                    match iris_capture::snapshot::snapshot(
                                        &frame, max_width, quality, mirror,
                                    ) {
                                        // A frame with no variation is worse
                                        // than no frame: a vision model given a
                                        // featureless image invents content
                                        // rather than abstaining, in the same
                                        // confident tone as a true description,
                                        // and nothing downstream can tell them
                                        // apart. Refusing costs a caller one
                                        // 503 it already handles; serving costs
                                        // it a fabrication it cannot detect.
                                        Ok(snap)
                                            if min_detail_tenths > 0
                                                && (snap.detail * 10.0)
                                                    < min_detail_tenths as f32 =>
                                        {
                                            (
                                                503,
                                                format!(
                                                    concat!(
                                                        "{{\"error\":\"the frame carries no ",
                                                        "detail (luma sd {:.3}, floor {:.1}) — ",
                                                        "a covered lens, an unlit scene or a ",
                                                        "synthetic backend. A vision model will ",
                                                        "describe this frame rather than report ",
                                                        "it as blank.\",",
                                                        "\"detail\":{:.3},\"min_detail\":{:.1}}}"
                                                    ),
                                                    snap.detail,
                                                    min_detail_tenths as f32 / 10.0,
                                                    snap.detail,
                                                    min_detail_tenths as f32 / 10.0,
                                                ),
                                            )
                                        }
                                        Ok(snap) => (
                                            200,
                                            format!(
                                                concat!(
                                                    "{{\"sequence\":{},\"width\":{},",
                                                    "\"height\":{},\"captured_us\":{},",
                                                    "\"age_ms\":{},\"mirrored\":{},",
                                                    "\"detail\":{:.3},",
                                                    "\"mime\":\"{}\",\"data_url\":\"{}\"}}"
                                                ),
                                                slot.sequence,
                                                snap.width,
                                                snap.height,
                                                slot.timestamp_us,
                                                age_ms.unwrap_or(0),
                                                mirror,
                                                snap.detail,
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
                        // The cheap layer: numbers, no image.
                        //
                        // `/frame` encodes a JPEG and base64s it — tens of
                        // kilobytes, and the wrong price for "has anything
                        // changed?". This is what a loop polls: brightness and
                        // detail from the newest frame, and motion against an
                        // earlier one from the ring. A vision model gets woken
                        // only once these say something happened.
                        //
                        // Motion is the reason this endpoint exists at all.
                        // `/frame` reads only `read_latest`, so nothing in Iris
                        // has ever compared two frames — and a model handed a
                        // single frame can see a person but not that the person
                        // moved.
                        "/sense" => {
                            let q = req.uri().query().unwrap_or("");
                            let param = |k: &str| -> Option<u64> {
                                q.split('&')
                                    .filter_map(|kv| kv.split_once('='))
                                    .find(|(name, _)| *name == k)
                                    .and_then(|(_, v)| v.parse().ok())
                            };
                            let work_width = param("width")
                                .unwrap_or(iris_capture::snapshot::SENSE_WIDTH as u64)
                                as u32;
                            // How far back to compare. 1 = the previous frame.
                            let back = param("back").unwrap_or(1).max(1) as usize;

                            let (latest, prev) = match http_frames.lock() {
                                Ok(rb) => (rb.read_latest().cloned(), rb.read_by_age(back).cloned()),
                                Err(_) => (None, None),
                            };
                            let (status, body) = match latest {
                                None => (
                                    503,
                                    "{\"error\":\"no frame captured yet — is capture running?\"}"
                                        .to_string(),
                                ),
                                Some(slot) => {
                                    let age_ms = iris_capture::frame::CaptureFrame::now_us()
                                        .saturating_sub(slot.timestamp_us)
                                        / 1_000;
                                    let to_frame = |s: &iris_stream::ring_buffer::RingSlot| {
                                        iris_capture::frame::CaptureFrame {
                                            sequence: s.sequence,
                                            width: s.width,
                                            height: s.height,
                                            format: s.format.clone(),
                                            data: s.data.clone(),
                                            timestamp_us: s.timestamp_us,
                                            is_cropped: false,
                                        }
                                    };
                                    let cur = to_frame(&slot);
                                    let earlier = prev.as_ref().map(to_frame);
                                    match iris_capture::snapshot::sense(
                                        &cur,
                                        earlier.as_ref(),
                                        work_width,
                                    ) {
                                        Ok(sn) => (
                                            200,
                                            format!(
                                                concat!(
                                                    "{{\"sequence\":{},\"age_ms\":{},",
                                                    "\"width\":{},\"height\":{},",
                                                    "\"brightness\":{:.3},\"detail\":{:.3},",
                                                    "\"motion\":{},\"motion_raw\":{},",
                                                    "\"light_shift\":{}}}"
                                                ),
                                                slot.sequence,
                                                age_ms,
                                                sn.width,
                                                sn.height,
                                                sn.brightness,
                                                sn.detail,
                                                // `null`, not 0: "no comparison
                                                // frame" and "nothing moved" are
                                                // different facts, and a caller
                                                // thresholding on 0 would read
                                                // the first as the second.
                                                //
                                                // `motion` is the LOCAL figure —
                                                // the uniform component removed —
                                                // because that is the one worth
                                                // thresholding on. Auto-exposure
                                                // moves the whole frame and would
                                                // otherwise read as movement.
                                                // `motion_raw` is kept beside it
                                                // so the difference is visible
                                                // rather than mysterious.
                                                match sn.motion {
                                                    Some(m) => format!("{:.3}", m.local),
                                                    None => "null".to_string(),
                                                },
                                                match sn.motion {
                                                    Some(m) => format!("{:.3}", m.raw),
                                                    None => "null".to_string(),
                                                },
                                                match sn.motion {
                                                    Some(m) => format!("{:.3}", m.shift),
                                                    None => "null".to_string(),
                                                }
                                            ),
                                        ),
                                        Err(e) => (
                                            500,
                                            format!("{{\"error\":\"sense failed: {e}\"}}"),
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

        // `controls.default_profile`, which reached nothing before 2026-09-04:
        // the field was serialised and documented while `load_profile` — which
        // has existed in `iris-control` since the crate was built — had no
        // caller at startup. Saving a profile worked and it was never applied.
        //
        // A failure is reported and not fatal: a missing or corrupt profile
        // should not stop the camera coming up, and saying so is better than a
        // silent fall-through to whatever the device happened to hold.
        if !config.controls.default_profile.trim().is_empty() {
            if let Some(handle) = &control_handle {
                let name = config.controls.default_profile.clone();
                match handle.load_profile(&name).await {
                    Ok(n) => tracing::info!("applied control profile '{name}' ({n} controls)"),
                    Err(e) => tracing::warn!(
                        "control profile '{name}' could not be applied ({e}); \
                         the camera keeps its current settings"
                    ),
                }
            } else {
                tracing::warn!(
                    "controls.default_profile is set to '{}' but no control service is \
                     running, so it cannot be applied",
                    config.controls.default_profile
                );
            }
        }

        Ok(Self {
            app_state,
            ipc_handle,
            capture_handle,
            stream_handle,
            mirror,
            control_handle,
            _tasks: tasks,
            _capture_telemetry_tx: capture_telemetry_tx.clone(),
            _capture_telemetry_keepalive: capture_keepalive,
        })
    }
}

#[cfg(test)]
mod no_camera_tests {
    use super::{offer, Offer};

    // With no camera, ListDevices offered the mock's "Mock Camera", the window
    // selected it, and flat grey frames reached the window and agents alike.

    #[test]
    fn no_camera_offers_nothing() {
        assert!(matches!(offer::<u8, ()>(Ok(vec![]), false), Offer::Nothing));
        assert!(matches!(offer::<u8, &str>(Err("v4l2 failed"), false), Offer::Nothing));
    }

    #[test]
    fn the_synthetic_device_only_when_asked_for() {
        assert!(matches!(offer::<u8, ()>(Ok(vec![]), true), Offer::Synthetic));
        assert!(matches!(offer::<u8, &str>(Err("v4l2 failed"), true), Offer::Synthetic));
    }

    #[test]
    fn real_cameras_are_offered_whatever_was_asked() {
        for synthetic in [false, true] {
            match offer::<u8, ()>(Ok(vec![7]), synthetic) {
                Offer::Real(list) => assert_eq!(list, vec![7]),
                other => panic!("a real camera must be offered, got {other:?}"),
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn with_no_camera_the_default_backend_is_not_the_mock() {
        use iris_capture::backend::{CaptureConfig, DropPolicy, NO_CAMERA};
        let cfg = CaptureConfig {
            width: 8,
            height: 8,
            target_fps: 30,
            format: iris_hal::device::PixelFormat::Rgb24,
            max_queue_depth: 4,
            drop_policy: DropPolicy::Oldest,
            roi: None,
        };
        let mut backend = super::default_linux_backend(false, cfg);
        // The mock starts and then yields grey frames; this must refuse, naming why.
        let err = backend.start().await.expect_err("no camera must not start a capture");
        assert!(err.to_string().contains(NO_CAMERA), "{err}");
        assert!(backend.next_frame().await.is_err(), "no camera must yield no frame");
    }
}

#[cfg(test)]
mod get_status_tests {
    use super::{capture_source_label, capture_state_text, ring_rate, IrisDispatcher};
    use iris_capture::frame::CaptureFrame;
    use iris_capture::service::CaptureServiceState;
    use iris_hal::device::PixelFormat;
    use iris_ipc::{response::IpcResponse, response::ResponseData, Dispatcher};

    /// Capture numbers frames from 1: the ring treats sequence 0 as an empty
    /// slot, so a frame 0 would silently not count.
    fn frame(seq: u64, ts: u64) -> CaptureFrame {
        assert!(seq > 0, "sequence 0 is an empty ring slot");
        CaptureFrame {
            sequence: seq,
            width: 2,
            height: 2,
            format: PixelFormat::Rgb24,
            data: vec![seq as u8; 12],
            timestamp_us: ts,
            is_cropped: false,
        }
    }

    #[test]
    fn the_state_is_the_capture_services() {
        assert_eq!(capture_state_text(&CaptureServiceState::Capturing), "Capturing");
        assert_eq!(capture_state_text(&CaptureServiceState::Idle), "Idle");
        assert_eq!(capture_state_text(&CaptureServiceState::Paused), "Paused");
        assert_eq!(
            capture_state_text(&CaptureServiceState::Error("start failed: device error: no camera found".into())),
            "Error: start failed: device error: no camera found"
        );
    }

    #[test]
    fn fps_is_measured_from_the_frames_held() {
        let now = 50_000_000u64;
        let mut rb = iris_stream::RingBuffer::new(8);
        for i in 0..4u64 {
            rb.write(&frame(i + 1, now - 100_000 + i * 33_333)); // ~30 fps, newest ~now
        }
        let (fps, total) = ring_rate(&rb, now);
        assert!((fps - 30.0).abs() < 0.1, "fps was {fps}");
        assert_eq!(total, 4);
    }

    #[test]
    fn no_rate_when_nothing_is_arriving() {
        let now = 50_000_000u64;
        let mut rb = iris_stream::RingBuffer::new(8);
        assert_eq!(ring_rate(&rb, now), (0.0, 0), "an empty ring");
        rb.write(&frame(1, now));
        assert_eq!(ring_rate(&rb, now), (0.0, 1), "one frame is not a rate");
        rb.write(&frame(2, now + 33_333));
        assert!(ring_rate(&rb, now + 33_333).0 > 29.0, "control: two fresh frames are a rate");
        assert_eq!(ring_rate(&rb, now + 33_333 + 2_000_000), (0.0, 2), "the newest is 2 s old");
    }

    #[test]
    fn the_source_is_named_never_mock_camera() {
        let cams = vec!["Logitech C920".to_string()];
        assert_eq!(capture_source_label("", &cams), "Logitech C920");
        assert_eq!(capture_source_label("mock", &cams), "synthetic test source (IRIS_BACKEND=mock)");
        #[cfg(target_os = "linux")]
        assert_eq!(capture_source_label("", &[]), "none: no camera found");
        for name in ["", "mock", "v4l2", "wmf", "dxgi"] {
            assert_ne!(capture_source_label(name, &[]), "Mock Camera");
        }
    }

    #[tokio::test]
    async fn get_status_reports_what_is_true() {
        let (state_tx, state_rx) = tokio::sync::watch::channel(CaptureServiceState::Error(
            "start failed: device error: no camera found".into(),
        ));
        let (_frames_tx, frames_rx) = tokio::sync::mpsc::channel(4);
        let (telemetry_tx, _telemetry_rx) = tokio::sync::broadcast::channel(16);
        let (service, stream) = iris_stream::StreamService::new(frames_rx, telemetry_tx, iris_stream::StreamMode::Pull, 4, 2);
        tokio::spawn(service.run());
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(4);
        let ring = stream.ring_buffer.clone();
        let mirror = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (_device_tx, device_rx) = tokio::sync::watch::channel(None);
        let mut d = IrisDispatcher::new(cmd_tx, ring, mirror, state_rx, stream, "none: no camera found".into(), device_rx);
        match d.dispatch(iris_ipc::command::IpcCommand::GetStatus).await {
            IpcResponse::Ok(ResponseData::Status { capture_state, device_name, fps, frame_count, subscriber_count }) => {
                assert_eq!(device_name, "none: no camera found");
                assert_eq!(capture_state, "Error: start failed: device error: no camera found");
                assert_eq!((fps, frame_count, subscriber_count), (0.0, 0, 0));
            }
            other => panic!("GetStatus answered {other:?}"),
        }
        // The state is read at each request, not fixed at start-up.
        state_tx.send(CaptureServiceState::Capturing).unwrap();
        match d.dispatch(iris_ipc::command::IpcCommand::GetStatus).await {
            IpcResponse::Ok(ResponseData::Status { capture_state, .. }) => assert_eq!(capture_state, "Capturing"),
            other => panic!("GetStatus answered {other:?}"),
        }
    }
}

#[cfg(test)]
mod truthful_answer_tests {
    use super::{IrisDispatcher, CAPTURE_NOT_RUNNING};
    use iris_capture::backend::OpenedCamera;
    use iris_capture::service::{CaptureCommand, CaptureServiceState};
    use iris_ipc::command::IpcCommand;
    use iris_ipc::{response::IpcResponse, response::ResponseData, Dispatcher};
    use std::time::{Duration, Instant};
    use tokio::sync::{mpsc, watch};

    fn cam(id: &str, name: &str) -> OpenedCamera {
        OpenedCamera { id: id.into(), name: name.into() }
    }

    struct Rig {
        d: IrisDispatcher,
        device_tx: Option<watch::Sender<Option<OpenedCamera>>>,
        cmd_rx: Option<mpsc::Receiver<CaptureCommand>>,
        stream: iris_stream::StreamHandle,
        /// Held so the capture state and the stream's frame channel stay open.
        _held: (watch::Sender<CaptureServiceState>, mpsc::Sender<iris_capture::frame::CaptureFrame>),
    }

    /// A dispatcher over a live stream service, with `open` as the open camera
    /// and "the first camera found" as the start-up label.
    fn rig(open: Option<OpenedCamera>) -> Rig {
        let (state_tx, state_rx) = watch::channel(CaptureServiceState::Capturing);
        let (frames_tx, frames_rx) = mpsc::channel(4);
        let (telemetry_tx, _telemetry_rx) = tokio::sync::broadcast::channel(16);
        let (service, stream) =
            iris_stream::StreamService::new(frames_rx, telemetry_tx, iris_stream::StreamMode::Pull, 4, 2);
        tokio::spawn(service.run());
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (device_tx, device_rx) = watch::channel(open);
        let ring = stream.ring_buffer.clone();
        let mirror = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d = IrisDispatcher::new(
            cmd_tx, ring, mirror, state_rx, stream.clone(), "the first camera found".into(), device_rx,
        );
        Rig { d, device_tx: Some(device_tx), cmd_rx: Some(cmd_rx), stream, _held: (state_tx, frames_tx) }
    }

    fn error(resp: IpcResponse) -> (u32, String) {
        match resp {
            IpcResponse::Error { code, message } => (code, message),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ping_reports_how_long_iris_has_run() {
        let mut r = rig(None);
        let uptime = |resp| match resp {
            IpcResponse::Ok(ResponseData::Pong { uptime_ms }) => uptime_ms,
            other => panic!("Ping answered {other:?}"),
        };
        let first = uptime(r.d.dispatch(IpcCommand::Ping).await);
        tokio::time::sleep(Duration::from_millis(40)).await;
        let second = uptime(r.d.dispatch(IpcCommand::Ping).await);
        assert!(first < 5_000, "a dispatcher made just now reported {first} ms");
        assert!(second >= first + 35, "40 ms later the uptime went from {first} to {second}");
    }

    #[tokio::test]
    async fn select_device_is_ok_only_for_the_open_camera() {
        let mut r = rig(Some(cam("/dev/video0", "Cam A")));
        let ok = r.d.dispatch(IpcCommand::SelectDevice { device_id: "/dev/video0".into() }).await;
        assert_eq!(ok, IpcResponse::Ok(ResponseData::Empty));
        let (code, why) = error(r.d.dispatch(IpcCommand::SelectDevice { device_id: "/dev/video2".into() }).await);
        assert_eq!(code, 501);
        assert!(why.contains("Cam A (/dev/video0)") && why.contains("IRIS_DEVICE=/dev/video2"), "{why}");
    }

    #[tokio::test]
    async fn select_device_waits_for_capture_to_open_its_camera() {
        let mut r = rig(None);
        let tx = r.device_tx.take().unwrap();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            tx.send(Some(cam("/dev/video0", "Cam A"))).unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await; // keep the sender alive
        });
        let ok = r.d.dispatch(IpcCommand::SelectDevice { device_id: "/dev/video0".into() }).await;
        assert_eq!(ok, IpcResponse::Ok(ResponseData::Empty), "the window asked before capture had opened it");
    }

    #[tokio::test]
    async fn select_device_does_not_wait_for_a_capture_that_has_ended() {
        let mut r = rig(None);
        drop(r.device_tx.take()); // the capture service has gone
        let started = Instant::now();
        let (code, _) = error(r.d.dispatch(IpcCommand::SelectDevice { device_id: "/dev/video0".into() }).await);
        assert_eq!(code, 503);
        assert!(started.elapsed() < Duration::from_secs(1), "waited {:?} on an ended capture", started.elapsed());
    }

    #[tokio::test]
    async fn get_status_names_the_camera_actually_open() {
        let mut r = rig(Some(cam("/dev/video2", "Cam B")));
        match r.d.dispatch(IpcCommand::GetStatus).await {
            IpcResponse::Ok(ResponseData::Status { device_name, .. }) => assert_eq!(device_name, "Cam B"),
            other => panic!("GetStatus answered {other:?}"),
        }
    }

    fn probed(id: &str) -> Result<String, String> {
        Ok(format!("{{\"probed\":\"{id}\"}}"))
    }
    fn busy(_id: &str) -> Result<String, String> {
        Err("Device or resource busy".into())
    }

    #[tokio::test]
    async fn get_device_capabilities_probes_the_open_camera() {
        let mut r = rig(Some(cam("/dev/video2", "Cam B")));
        r.d = r.d.with_probe(Some(probed));
        assert_eq!(
            r.d.dispatch(IpcCommand::GetDeviceCapabilities).await,
            IpcResponse::Ok(ResponseData::DeviceCapabilities { capabilities: "{\"probed\":\"/dev/video2\"}".into() })
        );
        r.d = r.d.with_probe(Some(busy));
        let (code, why) = error(r.d.dispatch(IpcCommand::GetDeviceCapabilities).await);
        assert!(code == 500 && why.contains("Cam B (/dev/video2)") && why.contains("busy"), "{code} {why}");
        r.d = r.d.with_probe(None);
        assert_eq!(error(r.d.dispatch(IpcCommand::GetDeviceCapabilities).await).0, 501);

        let mut none_open = rig(None);
        none_open.d = none_open.d.with_probe(Some(probed));
        assert_eq!(error(none_open.d.dispatch(IpcCommand::GetDeviceCapabilities).await).0, 503);
    }

    #[tokio::test]
    async fn capture_commands_reach_capture_or_say_it_is_not_running() {
        let mut r = rig(None);
        let mut rx = r.cmd_rx.take().unwrap();
        // Control: while capture listens, each command arrives as itself.
        for (cmd, expect) in [
            (IpcCommand::StartCapture, "Resume"),
            (IpcCommand::PauseCapture, "Pause"),
            (IpcCommand::ResumeCapture, "Resume"),
            (IpcCommand::SetFps { fps: 15 }, "SetFps(15)"),
            (IpcCommand::ClearRoi, "SetRoi(None)"),
            (IpcCommand::StopCapture, "Stop"),
        ] {
            assert_eq!(r.d.dispatch(cmd).await, IpcResponse::Ok(ResponseData::Empty));
            assert_eq!(format!("{:?}", rx.recv().await.unwrap()), expect);
        }
        drop(rx); // capture has ended
        for cmd in [
            IpcCommand::StartCapture,
            IpcCommand::StopCapture,
            IpcCommand::PauseCapture,
            IpcCommand::ResumeCapture,
            IpcCommand::SetFps { fps: 15 },
            IpcCommand::SetRoi { x: 0, y: 0, width: 2, height: 2 },
            IpcCommand::ClearRoi,
        ] {
            let (code, why) = error(r.d.dispatch(cmd).await);
            assert_eq!((code, why.as_str()), (503, CAPTURE_NOT_RUNNING));
        }
    }

    #[tokio::test]
    async fn get_stream_stats_are_the_stream_services() {
        let mut r = rig(None);
        let _sub = r.stream.subscribe().await.unwrap();
        match r.d.dispatch(IpcCommand::GetStreamStats).await {
            IpcResponse::Ok(ResponseData::StreamStats { subscriber_count, .. }) => assert_eq!(subscriber_count, 1),
            other => panic!("GetStreamStats answered {other:?}"),
        }
    }

    #[tokio::test]
    async fn commands_that_do_nothing_say_so() {
        let mut r = rig(Some(cam("/dev/video0", "Cam A")));
        for cmd in [
            IpcCommand::Subscribe,
            IpcCommand::Unsubscribe { subscriber_id: 1 },
            IpcCommand::SetResolution { width: 640, height: 480 },
            IpcCommand::SetPixelFormat { format: "mjpeg".into() },
            IpcCommand::GetConfig,
            IpcCommand::ReloadConfig,
            IpcCommand::UpdateConfig { section: "capture".into(), json: "{}".into() },
            IpcCommand::LoadProfile { name: "p".into() },
            IpcCommand::SaveProfile { name: "p".into() },
            IpcCommand::Init { config_path: None },
            IpcCommand::Shutdown,
            IpcCommand::DisconnectDevice,
            IpcCommand::ListControls,
            IpcCommand::GetControl { control: "brightness".into() },
            IpcCommand::SetControl { control: "brightness".into(), value: 1 },
            IpcCommand::ResetControl { control: "brightness".into() },
        ] {
            let name = format!("{cmd:?}");
            let (code, why) = error(r.d.dispatch(cmd).await);
            assert_eq!(code, 501, "{name}: {why}");
            assert!(why.ends_with("is not implemented"), "{name}: {why}");
        }
    }
}
