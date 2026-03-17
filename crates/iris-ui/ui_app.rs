use eframe::egui::{self, CentralPanel, ScrollArea, TopBottomPanel};
use iris_ipc::telemetry::TelemetryEnvelope;
use iris_ipc::IpcHandle;
use iris_ipc::LoggedTelemetryReceiver;
use iris_ipc::command::IpcCommand;
use iris_ipc::response::{DeviceEntry, ResponseData, IpcResponse};
use tokio::sync::broadcast;
use std::sync::{Arc, Mutex};
use iris_capture::service::CaptureHandle;
use iris_capture::frame::CaptureFrame;
use eframe::egui::{TextureHandle, ColorImage};

pub struct IrisApp {
    ipc: Arc<IpcHandle>,
    telemetry_rx: Mutex<LoggedTelemetryReceiver>,
    log: Mutex<Vec<String>>,
    devices: Arc<Mutex<Vec<DeviceEntry>>>,
    selected_device: Arc<Mutex<Option<String>>>,
    last_frame_info: Arc<Mutex<Option<(u32,u32)>>>,
    capture_rx: Option<tokio::sync::mpsc::Receiver<CaptureFrame>>,
    preview_texture: Option<TextureHandle>,
}

impl IrisApp {
    pub fn new(ipc: Arc<IpcHandle>, capture: CaptureHandle) -> Self {
        let telemetry_rx = ipc.subscribe_telemetry();
        let app = Self { ipc: ipc.clone(), telemetry_rx: Mutex::new(telemetry_rx), log: Mutex::new(Vec::new()), devices: Arc::new(Mutex::new(Vec::new())), selected_device: Arc::new(Mutex::new(None)), last_frame_info: Arc::new(Mutex::new(None)), capture_rx: Some(capture.frame_rx), preview_texture: None };

        // fetch initial device list
        let ipc_clone = ipc.clone();
        let devices_ref = app.devices.clone();
        tokio::spawn(async move {
            if let Ok(resp) = ipc_clone.send_command(IpcCommand::ListDevices).await {
                match resp {
                    IpcResponse::Ok(ResponseData::DeviceList { devices }) => {
                        if let Ok(mut dv) = devices_ref.lock() { *dv = devices; }
                    }
                    _ => {}
                }
            }
        });

        app
    }
}

impl eframe::App for IrisApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // drain any telemetry messages into a temporary buffer, then append to log
        let mut new_entries: Vec<String> = Vec::new();
        if let Ok(mut rx) = self.telemetry_rx.lock() {
            loop {
                match rx.try_recv() {
                    Ok(env) => {
                        let s = format!("{} - {:?}", env.timestamp, env.event);
                        // capture last frame info if present
                        match &env.event {
                            iris_ipc::telemetry::TelemetryEvent::FrameCaptured { sequence: _, width, height, size_bytes: _ } => {
                                if let Ok(mut lf) = self.last_frame_info.lock() { *lf = Some((*width, *height)); }
                            }
                            _ => {}
                        }
                        new_entries.push(s);
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                    Err(e) => {
                        // Log try_recv errors (including Lagged) and stop draining for now
                        println!("IrisApp: telemetry try_recv error: {:?}", e);
                        break;
                    }
                }
            }
        }
        if !new_entries.is_empty() {
            if let Ok(mut lg) = self.log.lock() {
                lg.extend(new_entries);
                let len = lg.len();
                if len > 1000 { lg.drain(0..len-500); }
            }
        }

        TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Iris — Mock UI");
                if ui.button("Start Capture").clicked() {
                    let ipc = Arc::clone(&self.ipc);
                    tokio::spawn(async move {
                        let _ = ipc.send_command(IpcCommand::StartCapture).await;
                    });
                }
                if ui.button("Stop Capture").clicked() {
                    let ipc = Arc::clone(&self.ipc);
                    tokio::spawn(async move {
                        let _ = ipc.send_command(IpcCommand::StopCapture).await;
                    });
                }
            });
        });

        CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading("Devices");
                    if ui.button("Refresh").clicked() {
                        let ipc = Arc::clone(&self.ipc);
                        let devices_ref = self.devices.clone();
                        tokio::spawn(async move {
                            if let Ok(resp) = ipc.send_command(IpcCommand::ListDevices).await {
                                match resp {
                                    IpcResponse::Ok(ResponseData::DeviceList { devices }) => {
                                        if let Ok(mut dv) = devices_ref.lock() { *dv = devices; }
                                    }
                                    _ => {}
                                }
                            }
                        });
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
                                            let _ = ipc.send_command(IpcCommand::SelectDevice { device_id: id.clone() }).await;
                                            if let Ok(mut s) = sel.lock() { *s = Some(id); }
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
                                    // convert BGR24 or other formats to RGBA
                                    let w = frame.width as usize;
                                    let h = frame.height as usize;
                                    let mut pixels: Vec<u8> = Vec::with_capacity(w*h*4);
                                    match frame.format {
                                        iris_hal::device::PixelFormat::Bgr24 => {
                                            let d = frame.data;
                                            for i in (0..d.len()).step_by(3) {
                                                let b = d[i]; let g = d[i+1]; let r = d[i+2];
                                                pixels.push(r); pixels.push(g); pixels.push(b); pixels.push(255);
                                            }
                                        }
                                        iris_hal::device::PixelFormat::Rgb24 => {
                                            let d = frame.data;
                                            for i in (0..d.len()).step_by(3) {
                                                let r = d[i]; let g = d[i+1]; let b = d[i+2];
                                                pixels.push(r); pixels.push(g); pixels.push(b); pixels.push(255);
                                            }
                                        }
                                            // Other formats not explicitly handled fall through
                                        _ => {}
                                    }

                                    if pixels.len() == w*h*4 {
                                        let image = ColorImage::from_rgba_unmultiplied([frame.width as usize, frame.height as usize], &pixels);
                                        // create or replace texture
                                        let tex = ctx.load_texture("iris_preview", image, egui::TextureOptions::LINEAR);
                                        self.preview_texture = Some(tex);
                                        if let Ok(mut lf) = self.last_frame_info.lock() { *lf = Some((frame.width, frame.height)); }
                                    }
                                }
                                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                                Err(_) => { self.capture_rx = None; break; }
                            }
                        }
                    }

                    if let Some(tex) = &self.preview_texture {
                        let size = tex.size();
                        let w = size[0] as f32;
                        let h = size[1] as f32;
                        ui.add(egui::Image::new(&*tex).fit_to_exact_size(egui::vec2(w.min(640.0), h.min(360.0))));
                    } else if let Ok(lf) = self.last_frame_info.lock() {
                        if let Some((w,h)) = *lf { ui.label(format!("Last frame: {}x{}", w, h)); } else { ui.label("No frames yet"); }
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
    }
}
