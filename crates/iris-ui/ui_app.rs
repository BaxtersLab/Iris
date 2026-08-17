use eframe::egui::{self, CentralPanel, ScrollArea, TopBottomPanel};
use eframe::egui::{ColorImage, TextureHandle, Key, Color32, Stroke, Rounding};
use iris_capture::frame::CaptureFrame;
use iris_capture::service::CaptureHandle;
use iris_ipc::command::IpcCommand;
use iris_ipc::response::{DeviceEntry, IpcResponse, ResponseData};
use iris_ipc::IpcHandle;
use iris_ipc::LoggedTelemetryReceiver;
use std::sync::{Arc, Mutex};

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
}

impl IrisApp {
    pub fn new(ipc: Arc<IpcHandle>, capture: CaptureHandle) -> Self {
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

        TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Iris — Mock UI");
                ui.label("(Keyboard: S=start, T=stop, R=refresh devices)  Tab order: [1] Start  [2] Stop  [3] Refresh");

                // Enable Start only when a device is selected
                let has_device = match self.selected_device.lock() {
                    Ok(g) => g.is_some(),
                    Err(_) => false,
                };

                let mut start_button = egui::Button::new("Start Capture");
                if self.is_capturing {
                    start_button = start_button.fill(Color32::from_rgb(76, 175, 80));
                }
                let start_resp = ui
                    .add_enabled(has_device, start_button)
                    .on_hover_text(if has_device { "Start camera capture" } else { "Select a device first" });
                ui.label("[1]");
                if start_resp.clicked() {
                    start_resp.request_focus();
                    self.is_capturing = true;
                    let ipc = Arc::clone(&self.ipc);
                    tokio::spawn(async move {
                        let _ = ipc.send_command(IpcCommand::StartCapture).await;
                    });
                }
                // Visual focus outline if widget has keyboard focus
                if start_resp.has_focus() {
                    let rect = start_resp.rect;
                    let stroke = Stroke::new(1.0_f32, Color32::BLACK);
                    ui.painter().rect_stroke(rect, Rounding::same(4.0), stroke);
                }

                let stop_resp = ui
                    .add_enabled(true, egui::Button::new("Stop Capture"))
                    .on_hover_text("Stop camera capture");
                ui.label("[2]");
                if stop_resp.clicked() {
                    stop_resp.request_focus();
                    self.is_capturing = false;
                    let ipc = Arc::clone(&self.ipc);
                    tokio::spawn(async move {
                        let _ = ipc.send_command(IpcCommand::StopCapture).await;
                    });
                }
                if stop_resp.has_focus() {
                    let rect = stop_resp.rect;
                    let stroke = Stroke::new(1.0_f32, Color32::BLACK);
                    ui.painter().rect_stroke(rect, Rounding::same(4.0), stroke);
                }
            });
        });

        CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading("Cameras");
                    let scan_resp = ui
                        .button("Detect Cameras")
                        .on_hover_text("Scan for physical cameras (or press R)");
                    ui.label("[3]");
                    if scan_resp.clicked() {
                        scan_resp.request_focus();
                        let ipc = Arc::clone(&self.ipc);
                        let devices_ref = self.devices.clone();
                        let summary_ref = self.last_scan_summary.clone();
                        let log_ref = self.log.clone();
                        tokio::spawn(async move {
                            match ipc.send_command(IpcCommand::ListDevices).await {
                                Ok(IpcResponse::Ok(ResponseData::DeviceList { devices })) => {
                                    let count = devices.len();
                                    let names: Vec<String> = devices.iter().map(|d| d.name.clone()).collect();
                                    let summary = format!("Found {} device(s): {}", count, names.join(", "));
                                    println!("Detect Cameras: {}", summary);
                                    if let Ok(mut lg) = log_ref.lock() {
                                        lg.push(format!("[Scan] {}", summary));
                                    }
                                    if let Ok(mut s) = summary_ref.lock() {
                                        *s = summary;
                                    }
                                    if let Ok(mut dv) = devices_ref.lock() {
                                        *dv = devices;
                                    }
                                }
                                Err(e) => {
                                    let msg = format!("Scan error: {:?}", e);
                                    println!("Detect Cameras: {}", msg);
                                    if let Ok(mut lg) = log_ref.lock() {
                                        lg.push(format!("[Scan] {}", msg));
                                    }
                                    if let Ok(mut s) = summary_ref.lock() {
                                        *s = msg;
                                    }
                                }
                                _ => {}
                            }
                        });
                    }
                    if scan_resp.has_focus() {
                        let rect = scan_resp.rect;
                        let stroke = Stroke::new(1.0_f32, Color32::BLACK);
                        ui.painter().rect_stroke(rect, Rounding::same(4.0), stroke);
                    }

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
                        loop {
                            match rx.try_recv() {
                                Ok(frame) => {
                                    // On successful frame, clear any pending deadline
                                    self.capture_rx_deadline = None;

                                    // convert BGR24 or other formats to RGBA
                                    let w = frame.width as usize;
                                    let h = frame.height as usize;
                                    let mut pixels: Vec<u8> = Vec::with_capacity(w * h * 4);
                                    match frame.format {
                                        // MJPEG is a compressed JPEG per frame; turning it
                                        // into RGBA needs a JPEG decoder, which Iris does not
                                        // depend on (dependency-light by design). Leave
                                        // `pixels` empty — the `pixels.len() == w*h*4` guard
                                        // below then skips the texture update, so the preview
                                        // holds its previous frame instead of rendering
                                        // garbage. See ROADMAP.md.
                                        iris_hal::device::PixelFormat::Mjpeg => {}
                                        iris_hal::device::PixelFormat::Bgr24 => {
                                            let d = frame.data;
                                            for i in (0..d.len()).step_by(3) {
                                                let b = d[i];
                                                let g = d[i + 1];
                                                let r = d[i + 2];
                                                pixels.push(r);
                                                pixels.push(g);
                                                pixels.push(b);
                                                pixels.push(255);
                                            }
                                        }
                                        iris_hal::device::PixelFormat::Rgb24 => {
                                            let d = frame.data;
                                            for i in (0..d.len()).step_by(3) {
                                                let r = d[i];
                                                let g = d[i + 1];
                                                let b = d[i + 2];
                                                pixels.push(r);
                                                pixels.push(g);
                                                pixels.push(b);
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

                                    if pixels.len() == w * h * 4 {
                                        let image = ColorImage::from_rgba_unmultiplied(
                                            [frame.width as usize, frame.height as usize],
                                            &pixels,
                                        );
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

                                        // (thumbnail generation removed) keep only the main preview texture
                                    }
                                }
                                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                                    // Start a short grace period before dropping the receiver
                                    if self.capture_rx_deadline.is_none() {
                                        self.capture_rx_deadline =
                                            Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
                                        println!("IrisApp: capture_rx closed; will drop receiver after 1s unless recovered");
                                    } else if let Some(d) = self.capture_rx_deadline {
                                        if std::time::Instant::now() >= d {
                                            println!("IrisApp: dropping closed capture_rx after grace period");
                                            self.capture_rx = None;
                                            self.capture_rx_deadline = None;
                                        }
                                    }
                                    break;
                                }
                            }
                        }
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
                let ipc = Arc::clone(&self.ipc);
                let devices_ref = self.devices.clone();
                let summary_ref = self.last_scan_summary.clone();
                let log_ref = self.log.clone();
                tokio::spawn(async move {
                    match ipc.send_command(IpcCommand::ListDevices).await {
                        Ok(IpcResponse::Ok(ResponseData::DeviceList { devices })) => {
                            let count = devices.len();
                            let names: Vec<String> = devices.iter().map(|d| d.name.clone()).collect();
                            let summary = format!("Found {} device(s): {}", count, names.join(", "));
                            if let Ok(mut lg) = log_ref.lock() { lg.push(format!("[Scan] {}", summary)); }
                            if let Ok(mut s) = summary_ref.lock() { *s = summary; }
                            if let Ok(mut dv) = devices_ref.lock() { *dv = devices; }
                        }
                        Err(e) => {
                            let msg = format!("Scan error: {:?}", e);
                            if let Ok(mut lg) = log_ref.lock() { lg.push(format!("[Scan] {}", msg)); }
                            if let Ok(mut s) = summary_ref.lock() { *s = msg; }
                        }
                        _ => {}
                    }
                });
            }
        });
    }
}
