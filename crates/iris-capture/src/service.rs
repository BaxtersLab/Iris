use crate::backend::{CaptureBackend, CaptureConfig, DropPolicy};
use crate::frame::{CaptureFrame, Roi};
use crate::telemetry::CaptureTelemetry;
use chrono::Utc;
use iris_core::error::IrisResult;
use iris_hal::device::PixelFormat;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch, Mutex, Notify};
use tracing::debug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureServiceState {
    Idle,
    Capturing,
    Paused,
    Error(String),
}

pub struct CaptureService<B: CaptureBackend + 'static> {
    backend: B,
    config: CaptureConfig,
    /// Sender used by forwarder to deliver frames to consumers.
    telemetry_tx: broadcast::Sender<CaptureTelemetry>,
    state_tx: watch::Sender<CaptureServiceState>,
    frame_count: Arc<AtomicU64>,
    drop_count: Arc<AtomicU64>,
    roi: Option<Roi>,
    buffer: Arc<Mutex<VecDeque<CaptureFrame>>>,
    notify: Arc<Notify>,
}

pub struct CaptureHandle {
    pub frame_rx: mpsc::Receiver<CaptureFrame>,
    cmd_tx: mpsc::Sender<CaptureCommand>,
    pub state_rx: watch::Receiver<CaptureServiceState>,
    pub frame_count: Arc<AtomicU64>,
    pub drop_count: Arc<AtomicU64>,
}

#[derive(Debug)]
pub enum CaptureCommand {
    Pause,
    Resume,
    Stop,
    SetRoi(Option<Roi>),
    SetFps(u32),
}

impl CaptureHandle {
    /// Swap the frame receiver, returning the previous one.
    ///
    /// Exists so the stream service can be inserted between capture and the
    /// window: it takes the raw receiver, and the window is handed a
    /// subscription in its place. The handle's command channel and counters
    /// still belong to the capture service, so only this one field moves.
    pub fn swap_frame_rx(
        &mut self,
        rx: mpsc::Receiver<CaptureFrame>,
    ) -> mpsc::Receiver<CaptureFrame> {
        std::mem::replace(&mut self.frame_rx, rx)
    }

    pub async fn send(&self, cmd: CaptureCommand) -> IrisResult<()> {
        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|e| iris_core::error::IrisError::Ipc(format!("cmd send failed: {}", e)))?;
        Ok(())
    }

    pub fn command_sender(&self) -> mpsc::Sender<CaptureCommand> {
        self.cmd_tx.clone()
    }

    pub fn state(&self) -> CaptureServiceState {
        self.state_rx.borrow().clone()
    }

    pub fn frame_count(&self) -> u64 {
        self.frame_count.load(Ordering::Relaxed)
    }
    pub fn drop_count(&self) -> u64 {
        self.drop_count.load(Ordering::Relaxed)
    }
}

impl<B: CaptureBackend + Send + 'static> CaptureService<B> {
    pub fn new(
        backend: B,
        config: CaptureConfig,
        telemetry_tx: broadcast::Sender<CaptureTelemetry>,
    ) -> (Self, CaptureHandle) {
        let (out_tx, frame_rx) = mpsc::channel(config.max_queue_depth);
        let (cmd_tx, _cmd_rx) = mpsc::channel(8);
        let (state_tx, state_rx) = watch::channel(CaptureServiceState::Idle);
        let buffer = Arc::new(Mutex::new(VecDeque::with_capacity(config.max_queue_depth)));
        let notify = Arc::new(Notify::new());
        let svc = CaptureService {
            backend,
            config: config.clone(),
            telemetry_tx,
            state_tx: state_tx.clone(),
            frame_count: Arc::new(AtomicU64::new(0)),
            drop_count: Arc::new(AtomicU64::new(0)),
            roi: config.roi,
            buffer: buffer.clone(),
            notify: notify.clone(),
        };
        let handle = CaptureHandle {
            frame_rx,
            cmd_tx,
            state_rx,
            frame_count: svc.frame_count.clone(),
            drop_count: svc.drop_count.clone(),
        };

        // Spawn forwarder task: move frames from internal buffer to the external channel
        tokio::spawn(async move {
            loop {
                notify.notified().await;
                loop {
                    let mut maybe_frame = None;
                    {
                        let mut buf = buffer.lock().await;
                        if let Some(f) = buf.pop_front() {
                            maybe_frame = Some(f);
                        }
                    }
                    if let Some(f) = maybe_frame {
                        if out_tx.send(f).await.is_err() {
                            // receiver dropped — exit forwarder
                            return;
                        }
                    } else {
                        break;
                    }
                }
            }
        });

        (svc, handle)
    }

    pub async fn run(mut self, mut cmd_rx: mpsc::Receiver<CaptureCommand>) {
        if let Err(e) = self.backend.start().await {
            // Also SAY so. This only set a watch-channel state that nothing
            // reads and printed nothing, so a capture backend that failed to
            // start produced a permanently blank preview and not one word
            // anywhere explaining it.
            eprintln!("CaptureService: backend failed to start: {e}");
            tracing::error!("capture backend failed to start: {e}");
            self.state_tx
                .send(CaptureServiceState::Error(format!("start failed: {}", e)))
                .ok();
            return;
        }
        self.state_tx.send(CaptureServiceState::Capturing).ok();

        let mut frame_errors: u64 = 0;
        let mut last_error_report = std::time::Instant::now();

        loop {
            let next_fut = self.backend.next_frame();
            tokio::select! {
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(CaptureCommand::Pause) => { println!("CaptureService: received Pause"); debug!("CaptureService: Pause command received"); self.state_tx.send(CaptureServiceState::Paused).ok(); continue; }
                        Some(CaptureCommand::Resume) => { println!("CaptureService: received Resume"); debug!("CaptureService: Resume command received"); self.state_tx.send(CaptureServiceState::Capturing).ok(); }
                        Some(CaptureCommand::Stop) | None => { println!("CaptureService: received Stop"); debug!("CaptureService: Stop command received"); let _ = self.backend.stop().await; self.state_tx.send(CaptureServiceState::Idle).ok(); break; }
                        Some(CaptureCommand::SetRoi(r)) => { println!("CaptureService: received SetRoi {:?}", r); debug!("CaptureService: SetRoi command received: {:?}", r); self.roi = r; }
                        Some(CaptureCommand::SetFps(f)) => { println!("CaptureService: received SetFps {}", f); debug!("CaptureService: SetFps command received: {}", f); self.config.target_fps = f; }
                    }
                }
                frame_res = next_fut => {
                    match frame_res {
                        Ok(mut frame) => {
                            // apply ROI cropping for supported formats
                            if let Some(roi) = self.roi {
                                if roi.validate(frame.width, frame.height) {
                                    Self::apply_roi(&mut frame, roi);
                                }
                            }

                            // capture authoritative per-frame facts before the
                            // frame moves into the buffer (the backend may
                            // deliver a different mode than configured)
                            let frame_size_bytes = frame.size_bytes();
                            let frame_width = frame.width;
                            let frame_height = frame.height;
                            let frame_format = frame.format.clone();

                            // push into internal buffer with drop policy handling
                            let mut dropped = false;
                            {
                                let mut buf = self.buffer.lock().await;
                                if buf.len() >= self.config.max_queue_depth {
                                    match self.config.drop_policy {
                                        DropPolicy::Oldest => {
                                            // evict the oldest and append new frame
                                            buf.pop_front();
                                            buf.push_back(frame);
                                        }
                                        DropPolicy::Newest => {
                                            // drop newest
                                            dropped = true;
                                        }
                                    }
                                } else {
                                    buf.push_back(frame);
                                }
                            }
                            // wake forwarder
                            self.notify.notify_one();
                            if dropped {
                                self.drop_count.fetch_add(1, Ordering::Relaxed);
                            } else {
                                self.frame_count.fetch_add(1, Ordering::Relaxed);
                                let telemetry = CaptureTelemetry {
                                    frames_captured: self.frame_count.load(Ordering::Relaxed),
                                    frames_dropped: self.drop_count.load(Ordering::Relaxed),
                                    current_fps: self.config.target_fps as f64,
                                    target_fps: self.config.target_fps,
                                    resolution: format!("{}x{}", frame_width, frame_height),
                                    format: format!("{}", frame_format),
                                    size_bytes: frame_size_bytes,
                                    queue_depth: self.config.max_queue_depth,
                                    roi_active: self.roi.is_some(),
                                };
                                let emit_ts = Utc::now();
                                let rc = self.telemetry_tx.receiver_count();
                                println!("CaptureService: emitting telemetry ts={} frames_captured={} roi_active={} receivers={}", emit_ts, telemetry.frames_captured, telemetry.roi_active, rc);
                                debug!("CaptureService: emitting telemetry ts={} frames_captured={} roi_active={} receivers={}", emit_ts, telemetry.frames_captured, telemetry.roi_active, rc);
                                // Note: telemetry itself doesn't carry the capture emission timestamp today;
                                // we log it here so forwarder can correlate.
                                let _ = self.telemetry_tx.send(telemetry);
                            }
                        }
                        Err(e) => {
                            // Report it, but not on every frame: a persistent
                            // failure at 30 fps would push thirty identical
                            // lines a second and bury everything else. The
                            // first one is the diagnosis; the rest are a
                            // counter.
                            //
                            // Silence here is what made a dead capture look
                            // like a working app with a grey picture.
                            frame_errors += 1;
                            let report = frame_errors == 1
                                || frame_errors % 100 == 0
                                || last_error_report.elapsed() >= std::time::Duration::from_secs(5);
                            if report {
                                eprintln!(
                                    "CaptureService: frame read failed ({frame_errors} so far): {e}"
                                );
                                tracing::warn!("frame read failed ({frame_errors}): {e}");
                                last_error_report = std::time::Instant::now();
                            }
                            self.state_tx.send(CaptureServiceState::Error(format!("frame error: {}", e))).ok();
                            // Do not spin: a failing read usually returns at
                            // once, and an unpaced retry loop burns a core.
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        }
    }
}

impl<B: CaptureBackend + Send + 'static> CaptureService<B> {
    pub(crate) fn apply_roi(frame: &mut CaptureFrame, roi: Roi) {
        match frame.format {
            // MJPEG is compressed — there is no pixel grid to slice, so cropping
            // requires a full decode. The frame consequently stops being
            // compressed: it becomes RGB24 and is cropped as such, and `format`
            // is updated so telemetry describes what the frame now actually is
            // rather than what it arrived as.
            //
            // If the decode fails the frame is left untouched and reported
            // uncropped, which is the old behaviour — never byte-sliced as if it
            // were raw pixels, since that would corrupt the JPEG.
            PixelFormat::Mjpeg => match crate::mjpeg::decode_to_rgb24(&frame.data) {
                Ok(decoded) => {
                    frame.data = decoded.rgb24;
                    frame.width = decoded.width;
                    frame.height = decoded.height;
                    frame.format = PixelFormat::Rgb24;
                    // It is a pixel grid now, so the RGB24 arm below applies.
                    // Terminates: `format` is no longer Mjpeg.
                    Self::apply_roi(frame, roi);
                }
                Err(e) => {
                    tracing::warn!("MJPEG decode failed, ROI not applied: {e}");
                    frame.is_cropped = false;
                }
            },
            PixelFormat::Rgb24 | PixelFormat::Bgr24 => {
                let bpp = 3usize;
                let src_w = frame.width as usize;
                let roi_x = roi.x as usize;
                let roi_y = roi.y as usize;
                let roi_w = roi.width as usize;
                let roi_h = roi.height as usize;
                let src_stride = src_w * bpp;
                let mut out = Vec::with_capacity(roi_w * roi_h * bpp);
                for row in 0..roi_h {
                    let src_row = roi_y + row;
                    let start = src_row * src_stride + roi_x * bpp;
                    let end = start + roi_w * bpp;
                    if end <= frame.data.len() {
                        out.extend_from_slice(&frame.data[start..end]);
                    }
                }
                frame.data = out;
                frame.width = roi.width;
                frame.height = roi.height;
                frame.is_cropped = true;
            }
            PixelFormat::Yuyv => {
                // packed YUYV (YUY2) 2 bytes per pixel
                let bpp = 2usize;
                let src_w = frame.width as usize;
                let roi_x = roi.x as usize;
                let roi_y = roi.y as usize;
                let roi_w = roi.width as usize;
                let roi_h = roi.height as usize;
                let src_stride = src_w * bpp;
                let mut out = Vec::with_capacity(roi_w * roi_h * bpp);
                for row in 0..roi_h {
                    let src_row = roi_y + row;
                    let start = src_row * src_stride + roi_x * bpp;
                    let end = start + roi_w * bpp;
                    if end <= frame.data.len() {
                        out.extend_from_slice(&frame.data[start..end]);
                    }
                }
                frame.data = out;
                frame.width = roi.width;
                frame.height = roi.height;
                frame.is_cropped = true;
            }
            PixelFormat::Nv12 => {
                // NV12 layout: Y plane (W*H), followed by interleaved UV plane (W * H / 2)
                let src_w = frame.width as usize;
                let src_h = frame.height as usize;
                let roi_x = roi.x as usize;
                let roi_y = roi.y as usize;
                let roi_w = roi.width as usize;
                let roi_h = roi.height as usize;

                // Auto-adjust ROI to even alignment for NV12 (2x2 chroma subsampling)
                let adj_x = roi_x & !1usize;
                let adj_y = roi_y & !1usize;
                // make width/height even by rounding down
                let mut adj_w = roi_w & !1usize;
                let mut adj_h = roi_h & !1usize;

                // clamp to source bounds and ensure evenness
                if adj_x >= src_w {
                    frame.is_cropped = false;
                    return;
                }
                if adj_y >= src_h {
                    frame.is_cropped = false;
                    return;
                }
                if adj_x + adj_w > src_w {
                    adj_w = src_w.saturating_sub(adj_x) & !1usize;
                }
                if adj_y + adj_h > src_h {
                    adj_h = src_h.saturating_sub(adj_y) & !1usize;
                }

                if adj_w == 0 || adj_h == 0 {
                    frame.is_cropped = false;
                    return;
                }

                let y_stride = src_w;
                let uv_stride = src_w; // UV row is src_w bytes (u,v pairs)
                let y_plane_size = src_w * src_h;
                // copy Y rows
                let mut out_y = Vec::with_capacity(adj_w * adj_h);
                for row in 0..adj_h {
                    let src_row = adj_y + row;
                    let start = src_row * y_stride + adj_x;
                    let end = start + adj_w;
                    if end <= y_plane_size && end <= frame.data.len() {
                        out_y.extend_from_slice(&frame.data[start..end]);
                    }
                }
                // copy UV rows (each UV row corresponds to two Y rows)
                let mut out_uv = Vec::with_capacity((adj_w / 2) * (adj_h / 2) * 2);
                let uv_plane_offset = y_plane_size;
                let uv_row_start = adj_y / 2;
                let uv_rows = adj_h / 2;
                let uv_x = adj_x / 2;
                let uv_w = adj_w / 2;
                for row in 0..uv_rows {
                    let src_row = uv_row_start + row;
                    let start = uv_plane_offset + src_row * uv_stride + uv_x * 2;
                    let end = start + uv_w * 2;
                    if end <= frame.data.len() {
                        out_uv.extend_from_slice(&frame.data[start..end]);
                    }
                }
                frame.data = [out_y, out_uv].concat();
                frame.width = adj_w as u32;
                frame.height = adj_h as u32;
                frame.is_cropped = true;
            }
        }
    }
}
