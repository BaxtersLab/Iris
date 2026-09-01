use eframe::egui::{self, CentralPanel, ScrollArea, TopBottomPanel};
use eframe::egui::{ColorImage, TextureHandle, Key, Color32, Stroke, Rounding};
use iris_capture::frame::CaptureFrame;
use iris_capture::service::CaptureHandle;
use iris_ipc::command::IpcCommand;
use iris_ipc::response::{DeviceEntry, IpcResponse, ResponseData};
use iris_ipc::IpcHandle;
use iris_ipc::LoggedTelemetryReceiver;
use std::sync::{Arc, Mutex};

/// Frames taken off the capture channel in a single repaint, at most.
///
/// The UI thread must never do work proportional to how far behind it has
/// fallen. Draining is cheap — a pointer move per frame — so this bound is far
/// above any real backlog (`max_queue_depth` defaults to 4); it exists so the
/// loop is *provably* terminating even against a producer that outruns it.
pub const MAX_DRAIN_PER_REPAINT: usize = 256;

/// What one drain of the capture channel found.
#[derive(Debug, Default)]
pub struct DrainOutcome {
    /// The newest frame available, or `None` if the channel was empty.
    pub newest: Option<CaptureFrame>,
    /// How many frames were taken off the channel, including `newest`.
    pub received: usize,
    /// The sender is gone.
    pub disconnected: bool,
}

/// Take every queued capture frame and keep only the newest.
///
/// The preview shows one image, so converting the frames behind it is work
/// thrown away — and this used to be done inline, converting **each** frame as
/// it came off the channel, before checking whether another was waiting.
///
/// That is not merely wasteful, it can livelock the UI thread. The loop only
/// ends when `try_recv` returns `Empty`, so if a conversion costs about as much
/// as the interval between frames the producer refills the channel as fast as
/// the loop empties it and `update()` never returns to the event loop.
/// **Measured on 2026-08-31**, debug build, mock backend, 640x480 NV12:
/// at `target_fps = 2` the UI repainted 647 times in 15 s; at `target_fps = 30`
/// — the shipped default — it repainted **once**, then pinned a core and drew
/// nothing further for the rest of the run. The window was frozen, and MJPEG
/// makes it worse, since each conversion is then a full JPEG decode.
///
/// Popping without converting is cheap enough that the drain always outruns the
/// producer, and [`MAX_DRAIN_PER_REPAINT`] bounds it regardless.
pub fn drain_to_newest(
    rx: &mut tokio::sync::mpsc::Receiver<CaptureFrame>,
) -> DrainOutcome {
    let mut out = DrainOutcome::default();
    for _ in 0..MAX_DRAIN_PER_REPAINT {
        match rx.try_recv() {
            Ok(frame) => {
                out.received += 1;
                out.newest = Some(frame);
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                out.disconnected = true;
                break;
            }
        }
    }
    out
}

/// Convert one captured frame to the RGBA8 buffer egui wants for a texture.
///
/// Returns an **empty** `Vec` when the frame cannot be converted — an
/// undersized buffer, or an MJPEG payload that fails to decode or decodes to
/// different dimensions than the frame reports. Callers must check the length
/// against `width * height * 4` before uploading; the preview then holds its
/// previous image rather than rendering garbage.
pub fn frame_to_rgba(frame: &CaptureFrame) -> Vec<u8> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let mut pixels: Vec<u8> = Vec::with_capacity(w * h * 4);
            match frame.format {
                // MJPEG is one compressed JPEG per frame. Decoding
                // lives in `iris_capture::mjpeg` so the HAL keeps
                // handing out the untouched compressed stream and
                // only the preview pays the decode cost.
                //
                // On any failure `pixels` is left empty and the
                // `pixels.len() == w*h*4` guard below skips the
                // texture update, so the preview holds its previous
                // frame rather than rendering garbage.
                iris_hal::device::PixelFormat::Mjpeg => {
                    match iris_capture::mjpeg::decode_to_rgb24(&frame.data)
                    {
                        Ok(d)
                            if d.width as usize == w
                                && d.height as usize == h =>
                        {
                            pixels =
                                iris_capture::mjpeg::rgb24_to_rgba8(&d.rgb24);
                        }
                        // Decoded fine but disagrees with the
                        // telemetry geometry — trusting either one
                        // would index the buffer wrongly.
                        Ok(d) => {
                            tracing::warn!(
                                "MJPEG decoded {}x{} but frame reports {w}x{h}; skipping",
                                d.width,
                                d.height
                            );
                        }
                        Err(e) => {
                            tracing::warn!("MJPEG decode failed: {e}");
                        }
                    }
                }
                // `chunks_exact(3)` rather than `(0..d.len()).step_by(3)`:
                // the index form read `d[i + 1]` and `d[i + 2]` unchecked, so a
                // buffer whose length was not a multiple of 3 — a short read
                // from hardware, a truncated frame — indexed past the end and
                // PANICKED on the UI thread. A short buffer must degrade to a
                // skipped frame, which is what the caller's length check does.
                // `.take(w * h)` likewise ignores any trailing bytes beyond the
                // declared geometry instead of overrunning the image.
                iris_hal::device::PixelFormat::Bgr24 => {
                    for px in frame.data.chunks_exact(3).take(w * h) {
                        pixels.push(px[2]);
                        pixels.push(px[1]);
                        pixels.push(px[0]);
                        pixels.push(255);
                    }
                }
                iris_hal::device::PixelFormat::Rgb24 => {
                    for px in frame.data.chunks_exact(3).take(w * h) {
                        pixels.push(px[0]);
                        pixels.push(px[1]);
                        pixels.push(px[2]);
                        pixels.push(255);
                    }
                }
                iris_hal::device::PixelFormat::Nv12 => {
                    // NV12: Y plane (w*h) then interleaved
                    // UV at half resolution. BT.601.
                    let d = &frame.data;
                    if d.len() >= w * h * 3 / 2 {
                        let uv_base = w * h;
                        for y in 0..h {
                            for x in 0..w {
                                let yv = d[y * w + x] as f32;
                                let uvi = uv_base
                                    + (y / 2) * w
                                    + (x / 2) * 2;
                                let u = d[uvi] as f32 - 128.0;
                                let v = d[uvi + 1] as f32 - 128.0;
                                let c = yv - 16.0;
                                let r = (1.164 * c + 1.596 * v)
                                    .clamp(0.0, 255.0);
                                let g = (1.164 * c - 0.392 * u
                                    - 0.813 * v)
                                    .clamp(0.0, 255.0);
                                let b = (1.164 * c + 2.017 * u)
                                    .clamp(0.0, 255.0);
                                pixels.push(r as u8);
                                pixels.push(g as u8);
                                pixels.push(b as u8);
                                pixels.push(255);
                            }
                        }
                    }
                }
                iris_hal::device::PixelFormat::Yuyv => {
                    // YUYV 4:2:2: Y0 U Y1 V per 2 pixels.
                    let d = &frame.data;
                    if d.len() >= w * h * 2 {
                        for i in (0..w * h * 2).step_by(4) {
                            let y0 = d[i] as f32;
                            let u = d[i + 1] as f32 - 128.0;
                            let y1 = d[i + 2] as f32;
                            let v = d[i + 3] as f32 - 128.0;
                            for yv in [y0, y1] {
                                let c = yv - 16.0;
                                let r = (1.164 * c + 1.596 * v)
                                    .clamp(0.0, 255.0);
                                let g = (1.164 * c - 0.392 * u
                                    - 0.813 * v)
                                    .clamp(0.0, 255.0);
                                let b = (1.164 * c + 2.017 * u)
                                    .clamp(0.0, 255.0);
                                pixels.push(r as u8);
                                pixels.push(g as u8);
                                pixels.push(b as u8);
                                pixels.push(255);
                            }
                        }
                    }
                }
            }
    pixels
}

pub struct IrisApp {
    ipc: Arc<IpcHandle>,
    telemetry_rx: Mutex<LoggedTelemetryReceiver>,
    log: Arc<Mutex<Vec<String>>>,
    devices: Arc<Mutex<Vec<DeviceEntry>>>,
    selected_device: Arc<Mutex<Option<String>>>,
    last_frame_info: Arc<Mutex<Option<(u32, u32)>>>,
    capture_rx: Option<tokio::sync::mpsc::Receiver<CaptureFrame>>,
    preview_texture: Option<TextureHandle>,
    // When capture_rx is closed, give it a short grace period before dropping
    // the receiver so transient races do not remove the preview permanently.
    capture_rx_deadline: Option<std::time::Instant>,
    // Track whether we've set the window size once on startup
    did_set_window_size: bool,
    // Whether capture is currently active (used to highlight Start button)
    is_capturing: bool,
    // (thumbnail fields removed; preview-only)
    // Last time cameras were scanned and result summary
    last_scan_summary: Arc<Mutex<String>>,
    // Preview drain accounting. `frames_received` counts every frame taken off
    // the capture channel; `frames_converted` counts the ones actually turned
    // into an RGBA texture. They are equal only if the UI converts every frame
    // it receives, so the pair is the direct measurement of the drain policy.
    frames_received: u64,
    frames_converted: u64,
    // Wall clock of the last drain-stats line, so the report is periodic rather
    // than per-repaint.
    perf_last_report: Option<std::time::Instant>,
    // Settings panel, opened from the gear at the right of the control strip.
    show_settings: bool,
    /// The camera's controls as last read. `None` until settings is opened —
    /// listing them talks to the device, so it is done on demand rather than
    /// on every repaint.
    controls: Arc<Mutex<Option<Vec<iris_control::ControlCapability>>>>,
    /// Why the control list is unavailable, when it is. Shown verbatim: "no
    /// camera selected" and "the driver refused" are different problems and the
    /// operator can only act on the difference if it is stated.
    controls_error: Arc<Mutex<Option<String>>>,
    control: Option<iris_control::ControlHandle>,
}

impl IrisApp {
    pub fn new(
        ipc: Arc<IpcHandle>,
        capture: CaptureHandle,
        control: Option<iris_control::ControlHandle>,
    ) -> Self {
        let telemetry_rx = ipc.subscribe_telemetry();
        let app = Self {
            ipc: ipc.clone(),
            telemetry_rx: Mutex::new(telemetry_rx),
            log: Arc::new(Mutex::new(Vec::new())),
            devices: Arc::new(Mutex::new(Vec::new())),
            selected_device: Arc::new(Mutex::new(None)),
            last_frame_info: Arc::new(Mutex::new(None)),
            capture_rx: Some(capture.frame_rx),
            preview_texture: None,
            capture_rx_deadline: None,
            did_set_window_size: false,
            is_capturing: false,
            last_scan_summary: Arc::new(Mutex::new("Not scanned yet".to_string())),
            frames_received: 0,
            frames_converted: 0,
            perf_last_report: None,
            show_settings: false,
            controls: Arc::new(Mutex::new(None)),
            controls_error: Arc::new(Mutex::new(None)),
            control,
        };

        // fetch initial device list and auto-select the first device if present
        let ipc_clone = ipc.clone();
        let devices_ref = app.devices.clone();
        let selected_ref = app.selected_device.clone();
        tokio::spawn(async move {
            if let Ok(IpcResponse::Ok(ResponseData::DeviceList { devices })) =
                ipc_clone.send_command(IpcCommand::ListDevices).await
            {
                if let Ok(mut dv) = devices_ref.lock() {
                    *dv = devices.clone();
                }
                if !devices.is_empty() {
                    // Prefer a real camera device over mock/virtual ones
                    let mut chosen: Option<String> = None;
                    for d in devices.iter() {
                        let name = d.name.to_lowercase();
                        if !(name.contains("mock") || name.contains("virtual") || name.contains("loopback")) {
                            chosen = Some(d.id.clone());
                            break;
                        }
                    }
                    let first_id = chosen.unwrap_or_else(|| devices[0].id.clone());
                    // attempt to tell the backend to select the device
                    let _ = ipc_clone
                        .send_command(IpcCommand::SelectDevice { device_id: first_id.clone() })
                        .await;
                    if let Ok(mut s) = selected_ref.lock() {
                        *s = Some(first_id.clone());
                    }
                    // Kick off capture automatically when a physical device was selected
                    // (backend may ignore if selection failed)
                    let _ = ipc_clone.send_command(IpcCommand::StartCapture).await;
                    // mark UI capturing state so Start button highlights appropriately
                    // Note: we can't mutate self here because we're in a spawned task; rely on telemetry
                }
            }
        });

        app
    }
}

impl IrisApp {
    /// Rescan for cameras. Shared by the strip button and the R shortcut, which
    /// had two copies of this before the control strip landed.
    fn refresh_devices(&self) {
        let ipc = Arc::clone(&self.ipc);
        let devices_ref = self.devices.clone();
        let summary_ref = self.last_scan_summary.clone();
        let log_ref = self.log.clone();
        tokio::spawn(async move {
            match ipc.send_command(IpcCommand::ListDevices).await {
                Ok(IpcResponse::Ok(ResponseData::DeviceList { devices })) => {
                    let names: Vec<String> = devices.iter().map(|d| d.name.clone()).collect();
                    let summary =
                        format!("Found {} device(s): {}", devices.len(), names.join(", "));
                    println!("Detect Cameras: {summary}");
                    if let Ok(mut lg) = log_ref.lock() {
                        lg.push(format!("[Scan] {summary}"));
                    }
                    if let Ok(mut s) = summary_ref.lock() {
                        *s = summary;
                    }
                    if let Ok(mut dv) = devices_ref.lock() {
                        *dv = devices;
                    }
                }
                Err(e) => {
                    let msg = format!("Scan error: {e:?}");
                    println!("Detect Cameras: {msg}");
                    if let Ok(mut lg) = log_ref.lock() {
                        lg.push(format!("[Scan] {msg}"));
                    }
                    if let Ok(mut s) = summary_ref.lock() {
                        *s = msg;
                    }
                }
                _ => {}
            }
        });
    }

    /// Ask the camera what controls it has.
    ///
    /// On demand rather than per repaint: every call talks to the device, and
    /// a 60 Hz repaint loop would hammer the driver with QUERYCTRL for a panel
    /// that is usually closed.
    fn refresh_controls(&self) {
        let Some(control) = self.control.clone() else {
            if let Ok(mut e) = self.controls_error.lock() {
                *e = Some(
                    "camera controls are unavailable — no control service is running \
                     (this build has no control backend for the selected device)"
                        .to_string(),
                );
            }
            return;
        };
        let controls_ref = self.controls.clone();
        let error_ref = self.controls_error.clone();
        tokio::spawn(async move {
            match control.list_controls().await {
                Ok(list) => {
                    if let Ok(mut e) = error_ref.lock() {
                        *e = if list.is_empty() {
                            Some("this camera exposes no adjustable controls".to_string())
                        } else {
                            None
                        };
                    }
                    if let Ok(mut c) = controls_ref.lock() {
                        *c = Some(list);
                    }
                }
                Err(err) => {
                    if let Ok(mut e) = error_ref.lock() {
                        *e = Some(format!("{err}"));
                    }
                }
            }
        });
    }

    /// The camera-control section of the settings panel.
    ///
    /// A slider per control, built from the driver's own reported range and
    /// step. `set_control` refuses a value off the step grid, so the slider
    /// snaps with `clamp_value` before sending — a slider that can produce a
    /// rejected value is a slider that sometimes does nothing.
    fn settings_controls_ui(&mut self, ui: &mut egui::Ui) {
        if let Ok(err) = self.controls_error.lock() {
            if let Some(msg) = err.as_ref() {
                ui.label(egui::RichText::new(msg).italics().weak());
                return;
            }
        }

        let snapshot = match self.controls.lock() {
            Ok(g) => g.clone(),
            Err(_) => None,
        };
        let Some(list) = snapshot else {
            ui.label(egui::RichText::new("reading controls from the camera…").weak());
            return;
        };
        if list.is_empty() {
            ui.label(egui::RichText::new("this camera exposes no adjustable controls").weak());
            return;
        }

        let mut pending: Vec<(iris_control::CameraControl, i64)> = Vec::new();
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .id_source("controls_scroll")
            .show(ui, |ui| {
                for cap in &list {
                    let mut value = cap.current;
                    ui.horizontal(|ui| {
                        ui.label(cap.control.name());
                        if cap.auto.is_toggleable() {
                            ui.label(egui::RichText::new("(auto available)").small().weak());
                        }
                    });
                    let resp = ui.add(
                        egui::Slider::new(&mut value, cap.min..=cap.max)
                            .step_by(cap.step.max(1) as f64)
                            .show_value(true),
                    );
                    if resp.changed() {
                        let snapped = cap.clamp_value(value);
                        pending.push((cap.control.clone(), snapped));
                    }
                }
            });

        for (control, value) in pending {
            self.apply_control(control, value);
        }
    }

    /// Send one control change, then re-read so the panel shows what the camera
    /// actually took rather than what was asked for.
    fn apply_control(&self, control: iris_control::CameraControl, value: i64) {
        let Some(handle) = self.control.clone() else {
            return;
        };
        let controls_ref = self.controls.clone();
        let error_ref = self.controls_error.clone();
        let log_ref = self.log.clone();
        tokio::spawn(async move {
            let name = control.name();
            match handle.set_control(control, value).await {
                Ok(()) => {
                    if let Ok(mut lg) = log_ref.lock() {
                        lg.push(format!("[Control] {name} = {value}"));
                    }
                    // Re-read: a driver may clamp or refuse, and the panel must
                    // show the camera's state, not the request.
                    if let Ok(list) = handle.list_controls().await {
                        if let Ok(mut c) = controls_ref.lock() {
                            *c = Some(list);
                        }
                    }
                }
                Err(e) => {
                    if let Ok(mut lg) = log_ref.lock() {
                        lg.push(format!("[Control] {name} = {value} refused: {e}"));
                    }
                    if let Ok(mut err) = error_ref.lock() {
                        *err = Some(format!("{e}"));
                    }
                }
            }
        });
    }

    /// Keep the window repainting while there is something live to show.
    ///
    /// egui is an immediate-mode GUI with a **reactive** run loop: eframe calls
    /// `update()` in response to input and window events, and otherwise sleeps.
    /// Nothing in this app asked it to do otherwise, so the camera preview only
    /// advanced while the pointer or keyboard was generating events over the
    /// window — leave it alone, or put another window in front of it, and the
    /// preview froze on whatever frame happened to be last while capture kept
    /// running behind it. Measured on 2026-08-31: an unfocused Iris window took
    /// **zero** frames off `capture_rx` across a 30 s run in which the capture
    /// service produced 869 frames.
    ///
    /// A live video preview has to drive its own clock. ~60 Hz comfortably
    /// outpaces any camera rate Iris configures (`IrisConfig::validate` caps
    /// `target_fps` at 240, but real UVC hardware here tops out at 30), so the
    /// preview never trails the source for want of a repaint.
    ///
    /// When capture is not running there is still a telemetry log ticking, so
    /// idle at 4 Hz rather than stopping: enough to keep the log live, cheap
    /// enough not to spin a core on an idle desktop.
    fn drive_repaints(&self, ctx: &egui::Context) {
        const PREVIEW_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);
        const IDLE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
        ctx.request_repaint_after(if self.capture_rx.is_some() {
            PREVIEW_INTERVAL
        } else {
            IDLE_INTERVAL
        });
    }

    /// Emit the preview-drain accounting once every 5 s.
    ///
    /// `received` is frames taken off the capture channel; `converted` is
    /// frames actually turned into a texture. `converted / received` is the
    /// share of the per-frame conversion cost (for MJPEG, a full JPEG decode)
    /// the UI actually pays. Reported rather than asserted, because the ratio
    /// depends on how far the repaint rate trails the capture rate.
    fn report_drain_stats(&mut self) {
        const REPORT_EVERY: std::time::Duration = std::time::Duration::from_secs(5);
        let now = std::time::Instant::now();
        match self.perf_last_report {
            Some(last) if now.duration_since(last) < REPORT_EVERY => return,
            _ => self.perf_last_report = Some(now),
        }
        // Deliberately reported even when `received` is 0. A preview that is
        // taking no frames at all is the single most useful thing this line can
        // say, and an early return on zero is exactly what hid it: the app
        // looked healthy because the only evidence of the stall was silence.
        let skipped = self.frames_received.saturating_sub(self.frames_converted);
        let pct = if self.frames_received == 0 {
            0.0
        } else {
            (self.frames_converted as f64 / self.frames_received as f64) * 100.0
        };
        let line = format!(
            "[Drain] received={} converted={} skipped={} ({pct:.1}% of frames converted)",
            self.frames_received, self.frames_converted, skipped,
        );
        println!("{line}");
        if let Ok(mut lg) = self.log.lock() {
            lg.push(line);
        }
    }
}

impl eframe::App for IrisApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Ensure window size is set once to requested small-mid dimensions (Windows)
        if !self.did_set_window_size {
            #[cfg(windows)]
            {
                crate::win32::set_iris_window_size(500, 400);
            }
            self.did_set_window_size = true;
        }
        // drain any telemetry messages into a temporary buffer, then append to log
        let mut new_entries: Vec<String> = Vec::new();
        if let Ok(mut rx) = self.telemetry_rx.lock() {
            loop {
                match rx.try_recv() {
                    Ok(env) => {
                        let s = format!("{} - {:?}", env.timestamp, env.event);
                        // capture last frame info if present
                        if let iris_ipc::telemetry::TelemetryEvent::FrameCaptured {
                            sequence: _,
                            width,
                            height,
                            size_bytes: _,
                        } = &env.event
                        {
                            if let Ok(mut lf) = self.last_frame_info.lock() {
                                *lf = Some((*width, *height));
                            }
                            // presence of frame telemetry implies capture is active
                            self.is_capturing = true;
                        }
                        new_entries.push(s);
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                        println!("IrisApp: telemetry lagged, skipped {} messages", n);
                        break;
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                        println!("IrisApp: telemetry channel closed");
                        break;
                    }
                }
            }
        }
        if !new_entries.is_empty() {
            if let Ok(mut lg) = self.log.lock() {
                lg.extend(new_entries);
                let len = lg.len();
                if len > 1000 {
                    lg.drain(0..len - 500);
                }
            }
        }

        // Title bar. It read "Iris — Mock UI" until 2026-08-31, a label from
        // before there was a real pipeline behind it — and the first thing
        // visible in any screenshot of the app.
        TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Iris");
                ui.separator();
                ui.label("Keyboard: S start · T stop · R refresh devices");
            });
        });

        // The control strip. Everything that DOES something lives along the
        // bottom, with settings pinned to the far right — so the strip reads
        // left to right as "act on the camera", and configuration is out of the
        // way of the actions rather than mixed in with them.
        TopBottomPanel::bottom("control_strip").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                // Start is disabled until a device is chosen: it is the one
                // action here that cannot mean anything without one.
                let has_device = match self.selected_device.lock() {
                    Ok(g) => g.is_some(),
                    Err(_) => false,
                };

                let mut start_button = egui::Button::new("▶ Start");
                if self.is_capturing {
                    start_button = start_button.fill(Color32::from_rgb(76, 175, 80));
                }
                let start_resp = ui.add_enabled(has_device, start_button).on_hover_text(
                    if has_device {
                        "Start camera capture  (S)"
                    } else {
                        "Select a camera first"
                    },
                );
                if start_resp.clicked() {
                    start_resp.request_focus();
                    self.is_capturing = true;
                    let ipc = Arc::clone(&self.ipc);
                    tokio::spawn(async move {
                        let _ = ipc.send_command(IpcCommand::StartCapture).await;
                    });
                }
                if start_resp.has_focus() {
                    ui.painter().rect_stroke(
                        start_resp.rect,
                        Rounding::same(4.0),
                        Stroke::new(1.0_f32, Color32::BLACK),
                    );
                }

                let stop_resp = ui
                    .add_enabled(true, egui::Button::new("■ Stop"))
                    .on_hover_text("Stop camera capture  (T)");
                if stop_resp.clicked() {
                    stop_resp.request_focus();
                    self.is_capturing = false;
                    let ipc = Arc::clone(&self.ipc);
                    tokio::spawn(async move {
                        let _ = ipc.send_command(IpcCommand::StopCapture).await;
                    });
                }
                if stop_resp.has_focus() {
                    ui.painter().rect_stroke(
                        stop_resp.rect,
                        Rounding::same(4.0),
                        Stroke::new(1.0_f32, Color32::BLACK),
                    );
                }

                ui.separator();

                let scan_resp = ui
                    .button("Detect Cameras")
                    .on_hover_text("Scan for connected cameras  (R)");
                if scan_resp.clicked() {
                    scan_resp.request_focus();
                    self.refresh_devices();
                }

                // Settings sits at the FAR RIGHT of the strip. right_to_left
                // lays out from the right edge, so this stays pinned there as
                // the window is resized rather than drifting with the buttons
                // to its left.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let gear = ui
                        .button("⚙")
                        .on_hover_text("Settings — camera controls and capture info");
                    if gear.clicked() {
                        self.show_settings = !self.show_settings;
                        if self.show_settings {
                            self.refresh_controls();
                        }
                    }
                });
            });
            ui.add_space(2.0);
        });

        // Settings. A side panel rather than a modal window: the preview stays
        // visible, which is the whole point of adjusting a camera control — you
        // are watching the effect while you drag.
        if self.show_settings {
            egui::SidePanel::right("settings_panel")
                .min_width(260.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("Settings");
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.button("✕").on_hover_text("Close settings").clicked() {
                                    self.show_settings = false;
                                }
                                if ui.button("⟳").on_hover_text("Re-read from the camera").clicked() {
                                    self.refresh_controls();
                                }
                            },
                        );
                    });
                    ui.separator();

                    ui.label(egui::RichText::new("Camera controls").strong());
                    self.settings_controls_ui(ui);

                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(egui::RichText::new("Capture").strong());
                    if let Ok(lf) = self.last_frame_info.lock() {
                        match *lf {
                            Some((w, h)) => {
                                ui.label(format!("Frames: {w}x{h}"));
                            }
                            None => {
                                ui.label(egui::RichText::new("No frames yet").weak());
                            }
                        }
                    }
                    let converted = if self.frames_received == 0 {
                        0.0
                    } else {
                        (self.frames_converted as f64 / self.frames_received as f64) * 100.0
                    };
                    ui.label(format!(
                        "Preview: {} of {} frames drawn ({converted:.0}%)",
                        self.frames_converted, self.frames_received
                    ));
                });
        }

        CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading("Cameras");

                    // Show last scan result
                    if let Ok(s) = self.last_scan_summary.lock() {
                        ui.label(egui::RichText::new(s.as_str()).small().weak());
                    }

                    ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        if let Ok(dv) = self.devices.lock() {
                            // Filter out mock/virtual; show all real cameras
                            let real_devices: Vec<&DeviceEntry> = dv
                                .iter()
                                .filter(|d| {
                                    let lname = d.name.to_lowercase();
                                    !(d.id.starts_with("mock") || lname.contains("mock") || lname.contains("virtual") || lname.contains("loopback"))
                                })
                                .collect();

                            if real_devices.is_empty() {
                                ui.label(egui::RichText::new("No cameras detected — click Detect Cameras").italics().weak());
                            } else {
                                for dev in real_devices.iter() {
                                    ui.horizontal(|ui| {
                                        // Highlight if currently selected
                                        let is_selected = match self.selected_device.lock() {
                                            Ok(s) => s.as_deref() == Some(dev.id.as_str()),
                                            Err(_) => false,
                                        };
                                        let label = if is_selected {
                                            egui::RichText::new(format!("\u{25CF} {}", dev.name)).color(Color32::from_rgb(76, 175, 80))
                                        } else {
                                            egui::RichText::new(&dev.name)
                                        };
                                        ui.label(label);
                                        if ui.button("Select").clicked() {
                                            let ipc = Arc::clone(&self.ipc);
                                            let id = dev.id.clone();
                                            let sel = self.selected_device.clone();
                                            tokio::spawn(async move {
                                                let _ = ipc
                                                    .send_command(IpcCommand::SelectDevice {
                                                        device_id: id.clone(),
                                                    })
                                                    .await;
                                                if let Ok(mut s) = sel.lock() {
                                                    *s = Some(id);
                                                }
                                            });
                                        }
                                    });
                                }
                            }
                        }
                    });
                });

                ui.separator();

                ui.vertical(|ui| {
                    ui.heading("Preview");
                    // Drain any available capture frames and update preview texture
                    if let Some(rx) = &mut self.capture_rx {
                        // Drain first, convert once. See `drain_to_newest`.
                        let drained = crate::ui_app::drain_to_newest(rx);
                        self.frames_received += drained.received as u64;

                        if drained.received > 0 {
                            // A frame arrived, so any pending close is stale.
                            self.capture_rx_deadline = None;
                        }

                        if let Some(frame) = drained.newest {
                            let w = frame.width as usize;
                            let h = frame.height as usize;
                            let pixels = frame_to_rgba(&frame);
                            if pixels.len() == w * h * 4 {
                                self.frames_converted += 1;
                                let image = ColorImage::from_rgba_unmultiplied([w, h], &pixels);
                                // Replace the EXISTING texture's contents in place.
                                //
                                // This used to call `ctx.load_texture(...)` every frame,
                                // which ALLOCATES A NEW TEXTURE each time — it does not
                                // replace, despite the old comment saying so. Overwriting
                                // `self.preview_texture` did not free the previous one, so
                                // the app leaked one full RGBA image (width*height*4) per
                                // captured frame: ~27 MB/s at 30 fps, 788 MB -> 4.8 GB in
                                // under a minute. Confirmed with heaptrack: 584.91 MB
                                // leaked over 476 calls from this exact line, = 1.23 MB
                                // each = 640*480*4 exactly. `TextureHandle::set` reuses
                                // the allocation instead.
                                match &mut self.preview_texture {
                                    Some(tex) => {
                                        tex.set(image, egui::TextureOptions::LINEAR);
                                    }
                                    None => {
                                        self.preview_texture = Some(ctx.load_texture(
                                            "iris_preview",
                                            image,
                                            egui::TextureOptions::LINEAR,
                                        ));
                                    }
                                }
                                if let Ok(mut lf) = self.last_frame_info.lock() {
                                    *lf = Some((frame.width, frame.height));
                                }
                            }
                        }

                        if drained.disconnected {
                            // Start a short grace period before dropping the receiver
                            if self.capture_rx_deadline.is_none() {
                                self.capture_rx_deadline = Some(
                                    std::time::Instant::now() + std::time::Duration::from_secs(1),
                                );
                                println!("IrisApp: capture_rx closed; will drop receiver after 1s unless recovered");
                            } else if let Some(d) = self.capture_rx_deadline {
                                if std::time::Instant::now() >= d {
                                    println!("IrisApp: dropping closed capture_rx after grace period");
                                    self.capture_rx = None;
                                    self.capture_rx_deadline = None;
                                }
                            }
                        }

                        self.report_drain_stats();
                    }

                    if let Some(tex) = &self.preview_texture {
                        let size = tex.size();
                        let w = size[0] as f32;
                        let h = size[1] as f32;
                        let max_w = 320.0;
                        let max_h = 180.0;
                        ui.add(
                            egui::Image::new(tex)
                                .fit_to_exact_size(egui::vec2(w.min(max_w), h.min(max_h))),
                        );
                    } else if let Ok(lf) = self.last_frame_info.lock() {
                        if let Some((w, h)) = *lf {
                            ui.label(format!("Last frame: {}x{}", w, h));
                        } else {
                            ui.label("No frames yet");
                        }
                    }
                });
            });

            ui.separator();

            ui.heading("Telemetry Log");
            ScrollArea::vertical().show(ui, |ui| {
                if let Ok(lg) = self.log.lock() {
                    for line in lg.iter().rev().take(200) {
                        ui.label(line);
                    }
                }
            });
        });

        // Keyboard shortcuts for accessibility: allow quick Start/Stop/Refresh
        // S = Start (when device selected), T = Stop, R = Refresh devices
        ctx.input(|input| {
            if input.key_pressed(Key::S) {
                let has_device = match self.selected_device.lock() {
                    Ok(g) => g.is_some(),
                    Err(_) => false,
                };
                if has_device {
                    let ipc = Arc::clone(&self.ipc);
                    tokio::spawn(async move {
                        let _ = ipc.send_command(IpcCommand::StartCapture).await;
                    });
                }
            }
            if input.key_pressed(Key::T) {
                let ipc = Arc::clone(&self.ipc);
                tokio::spawn(async move {
                    let _ = ipc.send_command(IpcCommand::StopCapture).await;
                });
            }
            if input.key_pressed(Key::R) {
                self.refresh_devices();
            }
        });

        self.drive_repaints(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_hal::device::PixelFormat;
    use tokio::sync::mpsc;

    /// The same 16x16 baseline JPEG `iris-capture` decodes against, referenced
    /// rather than copied so the two crates cannot drift onto different bytes.
    const TINY_JPEG: &[u8] = include_bytes!("../iris-capture/tests/fixtures/tiny16.jpg");

    fn frame(seq: u64, w: u32, h: u32, format: PixelFormat, data: Vec<u8>) -> CaptureFrame {
        CaptureFrame {
            sequence: seq,
            width: w,
            height: h,
            format,
            data,
            timestamp_us: 0,
            is_cropped: false,
        }
    }

    fn blank(seq: u64) -> CaptureFrame {
        frame(seq, 1, 1, PixelFormat::Rgb24, vec![0, 0, 0])
    }

    // ---- drain_to_newest -------------------------------------------------

    #[test]
    fn drain_reports_nothing_on_an_empty_channel() {
        let (_tx, mut rx) = mpsc::channel::<CaptureFrame>(4);
        let out = drain_to_newest(&mut rx);
        assert!(out.newest.is_none());
        assert_eq!(out.received, 0);
        assert!(!out.disconnected);
    }

    #[test]
    fn drain_passes_a_single_frame_straight_through() {
        let (tx, mut rx) = mpsc::channel::<CaptureFrame>(4);
        tx.try_send(blank(7)).unwrap();
        let out = drain_to_newest(&mut rx);
        assert_eq!(out.received, 1);
        assert_eq!(out.newest.expect("frame").sequence, 7);
    }

    /// The point of the change: a backlog costs one conversion, not one per
    /// queued frame. `received` counts what came off the channel, and exactly
    /// one frame — the last — survives to be converted.
    #[test]
    fn drain_keeps_only_the_newest_of_a_backlog() {
        let (tx, mut rx) = mpsc::channel::<CaptureFrame>(16);
        for seq in 1..=10 {
            tx.try_send(blank(seq)).unwrap();
        }
        let out = drain_to_newest(&mut rx);
        assert_eq!(out.received, 10, "all ten must be taken off the channel");
        assert_eq!(
            out.newest.expect("frame").sequence,
            10,
            "the newest frame is the one kept"
        );
        assert_eq!(rx.try_recv().is_err(), true, "channel must be left empty");
    }

    /// REGRESSION — the livelock.
    ///
    /// The old drain looped until `try_recv` returned `Empty` while converting
    /// each frame inline, so a producer that refilled the channel as fast as
    /// the loop emptied it kept the UI thread inside `update()` forever: 30 s
    /// at 640x480 NV12 @30 fps produced exactly **one** repaint. The invariant
    /// that prevents it is that one drain does a bounded amount of work and
    /// returns to the event loop regardless of how far behind the UI is.
    #[test]
    fn drain_is_bounded_and_always_returns() {
        let over = MAX_DRAIN_PER_REPAINT + 50;
        let (tx, mut rx) = mpsc::channel::<CaptureFrame>(over);
        for seq in 0..over {
            tx.try_send(blank(seq as u64)).unwrap();
        }
        let out = drain_to_newest(&mut rx);
        assert_eq!(
            out.received, MAX_DRAIN_PER_REPAINT,
            "one repaint must take at most MAX_DRAIN_PER_REPAINT frames"
        );
        assert!(!out.disconnected, "a full channel is not a disconnect");
        // The rest stay queued for the next repaint rather than holding the
        // UI thread until the producer happens to pause.
        assert_eq!(out.newest.expect("frame").sequence, MAX_DRAIN_PER_REPAINT as u64 - 1);
    }

    #[test]
    fn drain_flags_a_dropped_sender() {
        let (tx, mut rx) = mpsc::channel::<CaptureFrame>(4);
        tx.try_send(blank(1)).unwrap();
        drop(tx);
        let out = drain_to_newest(&mut rx);
        assert_eq!(out.received, 1, "queued frames are still delivered");
        assert!(out.disconnected, "the closed sender must be reported");
    }

    // ---- frame_to_rgba ---------------------------------------------------

    #[test]
    fn rgb24_gains_an_opaque_alpha_in_place() {
        let f = frame(0, 2, 1, PixelFormat::Rgb24, vec![10, 20, 30, 40, 50, 60]);
        assert_eq!(
            frame_to_rgba(&f),
            vec![10, 20, 30, 255, 40, 50, 60, 255]
        );
    }

    #[test]
    fn bgr24_is_channel_swapped() {
        let f = frame(0, 1, 1, PixelFormat::Bgr24, vec![30, 20, 10]);
        assert_eq!(frame_to_rgba(&f), vec![10, 20, 30, 255], "B,G,R -> R,G,B,A");
    }

    /// The old index-stepping form read `d[i + 1]` unchecked. A buffer whose
    /// length is not a multiple of 3 panicked the UI thread; it must now come
    /// back short so the caller skips the frame.
    #[test]
    fn a_ragged_rgb_buffer_does_not_panic() {
        let f = frame(0, 2, 1, PixelFormat::Rgb24, vec![1, 2, 3, 4]);
        let px = frame_to_rgba(&f);
        assert_ne!(px.len(), 2 * 1 * 4, "must not be accepted as a full frame");
        assert_eq!(px, vec![1, 2, 3, 255], "the one whole pixel converts");
    }

    #[test]
    fn an_undersized_nv12_buffer_converts_to_nothing() {
        // 2x2 NV12 needs 6 bytes; give it 5.
        let f = frame(0, 2, 2, PixelFormat::Nv12, vec![16; 5]);
        assert!(frame_to_rgba(&f).is_empty());
    }

    #[test]
    fn an_undersized_yuyv_buffer_converts_to_nothing() {
        // 2x1 YUYV needs 4 bytes; give it 3.
        let f = frame(0, 2, 1, PixelFormat::Yuyv, vec![16; 3]);
        assert!(frame_to_rgba(&f).is_empty());
    }

    /// BT.601 limited range: Y=16 is black, Y=235 is white, U=V=128 is neutral.
    /// Chosen because the answers are known independently of the code.
    #[test]
    fn yuyv_maps_limited_range_luma_to_black_and_white() {
        let f = frame(0, 2, 1, PixelFormat::Yuyv, vec![16, 128, 235, 128]);
        let px = frame_to_rgba(&f);
        assert_eq!(px.len(), 8);
        assert_eq!(&px[0..4], &[0, 0, 0, 255], "Y=16 neutral chroma is black");
        assert_eq!(&px[4..8], &[254, 254, 254, 255], "Y=235 neutral chroma is white");
    }

    #[test]
    fn nv12_maps_limited_range_luma_to_black() {
        // 2x2: four Y samples then one interleaved UV pair.
        let f = frame(0, 2, 2, PixelFormat::Nv12, vec![16, 16, 16, 16, 128, 128]);
        let px = frame_to_rgba(&f);
        assert_eq!(px.len(), 2 * 2 * 4);
        assert!(px.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
    }

    #[test]
    fn mjpeg_decodes_to_a_full_rgba_image() {
        let f = frame(0, 16, 16, PixelFormat::Mjpeg, TINY_JPEG.to_vec());
        let px = frame_to_rgba(&f);
        assert_eq!(px.len(), 16 * 16 * 4, "a decoded MJPEG frame is complete");
        let first = px[0];
        assert!(
            px.iter().any(|&b| b != first),
            "a uniform buffer means the decode produced nothing real"
        );
    }

    /// Decoding fine but disagreeing with the telemetry geometry is the case
    /// that would index the buffer wrongly, so it is refused rather than
    /// stretched to fit.
    #[test]
    fn mjpeg_that_disagrees_with_the_frame_geometry_is_refused() {
        let f = frame(0, 32, 32, PixelFormat::Mjpeg, TINY_JPEG.to_vec());
        assert!(
            frame_to_rgba(&f).is_empty(),
            "16x16 JPEG in a frame declaring 32x32 must not be converted"
        );
    }

    #[test]
    fn undecodable_mjpeg_converts_to_nothing() {
        let f = frame(0, 16, 16, PixelFormat::Mjpeg, vec![0xFF, 0xD8, 0x00, 0x01]);
        assert!(frame_to_rgba(&f).is_empty());
    }
}
