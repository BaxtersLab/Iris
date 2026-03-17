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
    log: Mutex<Vec<String>>,
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
}

impl IrisApp {
    pub fn new(ipc: Arc<IpcHandle>, capture: CaptureHandle) -> Self {
        let telemetry_rx = ipc.subscribe_telemetry();
        let app = Self {
            ipc: ipc.clone(),
            telemetry_rx: Mutex::new(telemetry_rx),
            log: Mutex::new(Vec::new()),
            devices: Arc::new(Mutex::new(Vec::new())),
            selected_device: Arc::new(Mutex::new(None)),
            last_frame_info: Arc::new(Mutex::new(None)),
            capture_rx: Some(capture.frame_rx),
            preview_texture: None,
            capture_rx_deadline: None,
            did_set_window_size: false,
        };

        // fetch initial device list
        let ipc_clone = ipc.clone();
        let devices_ref = app.devices.clone();
        tokio::spawn(async move {
            if let Ok(IpcResponse::Ok(ResponseData::DeviceList { devices })) =
                ipc_clone.send_command(IpcCommand::ListDevices).await
            {
                if let Ok(mut dv) = devices_ref.lock() {
                    *dv = devices;
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

                let start_resp = ui
                    .add_enabled(has_device, egui::Button::new("Start Capture"))
                    .on_hover_text(if has_device { "Start camera capture" } else { "Select a device first" });
                ui.label("[1]");
                if start_resp.clicked() {
                    start_resp.request_focus();
                    let ipc = Arc::clone(&self.ipc);
                    tokio::spawn(async move {
                        let _ = ipc.send_command(IpcCommand::StartCapture).await;
                    });
                }
                // Visual focus outline if widget has keyboard focus
                if start_resp.has_focus() {
                    let rect = start_resp.rect;
                    let stroke = Stroke::new(1.0, Color32::BLACK);
                    ui.painter().rect_stroke(rect, Rounding::same(4.0), stroke);
                }

                let stop_resp = ui
                    .add_enabled(true, egui::Button::new("Stop Capture"))
                    .on_hover_text("Stop camera capture");
                ui.label("[2]");
                if stop_resp.clicked() {
                    stop_resp.request_focus();
                    let ipc = Arc::clone(&self.ipc);
                    tokio::spawn(async move {
                        let _ = ipc.send_command(IpcCommand::StopCapture).await;
                    });
                }
                if stop_resp.has_focus() {
                    let rect = stop_resp.rect;
                    let stroke = Stroke::new(1.0, Color32::BLACK);
                    ui.painter().rect_stroke(rect, Rounding::same(4.0), stroke);
                }
            });
        });

        CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading("Devices");
                    let refresh_resp = ui
                        .button("Refresh")
                        .on_hover_text("Refresh device list (or press R)");
                    ui.label("[3]");
                    if refresh_resp.clicked() {
                        refresh_resp.request_focus();
                        let ipc = Arc::clone(&self.ipc);
                        let devices_ref = self.devices.clone();
                        tokio::spawn(async move {
                            if let Ok(IpcResponse::Ok(ResponseData::DeviceList { devices })) =
                                ipc.send_command(IpcCommand::ListDevices).await
                            {
                                if let Ok(mut dv) = devices_ref.lock() {
                                    *dv = devices;
                                }
                            }
                        });
                    }
                    if refresh_resp.has_focus() {
                        let rect = refresh_resp.rect;
                        let stroke = Stroke::new(1.0, Color32::BLACK);
                        ui.painter().rect_stroke(rect, Rounding::same(4.0), stroke);
                    }

                    ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        if let Ok(dv) = self.devices.lock() {
                            for dev in dv.iter() {
                                ui.horizontal(|ui| {
                                    ui.label(&dev.name);
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
                                        iris_core::PixelFormat::Bgr24 => {
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
                                        iris_core::PixelFormat::Rgb24 => {
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
                                        // Other formats not explicitly handled fall through
                                        _ => {}
                                    }

                                    if pixels.len() == w * h * 4 {
                                        let image = ColorImage::from_rgba_unmultiplied(
                                            [frame.width as usize, frame.height as usize],
                                            &pixels,
                                        );
                                        // create or replace texture
                                        let tex = ctx.load_texture(
                                            "iris_preview",
                                            image,
                                            egui::TextureOptions::LINEAR,
                                        );
                                        self.preview_texture = Some(tex);
                                        if let Ok(mut lf) = self.last_frame_info.lock() {
                                            *lf = Some((frame.width, frame.height));
                                        }
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
                        ui.add(
                            egui::Image::new(tex)
                                .fit_to_exact_size(egui::vec2(w.min(640.0), h.min(360.0))),
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
                tokio::spawn(async move {
                    if let Ok(IpcResponse::Ok(ResponseData::DeviceList { devices })) =
                        ipc.send_command(IpcCommand::ListDevices).await
                    {
                        if let Ok(mut dv) = devices_ref.lock() {
                            *dv = devices;
                        }
                    }
                });
            }
        });
    }
}
