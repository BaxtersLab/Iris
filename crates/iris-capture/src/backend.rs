use crate::frame::{CaptureFrame, Roi};
use async_trait::async_trait;
use iris_core::error::IrisResult;
use iris_hal::device::PixelFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropPolicy {
    Oldest,
    Newest,
}

impl DropPolicy {
    // Prefer the standard `FromStr` trait implementation below. If callers
    // need an Option-returning helper they can use `s.parse().ok()`.
}

impl std::str::FromStr for DropPolicy {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "oldest" => Ok(DropPolicy::Oldest),
            "newest" => Ok(DropPolicy::Newest),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub width: u32,
    pub height: u32,
    pub target_fps: u32,
    pub format: PixelFormat,
    pub max_queue_depth: usize,
    pub drop_policy: DropPolicy,
    pub roi: Option<Roi>,
}

#[async_trait]
pub trait CaptureBackend: Send + Sync {
    async fn start(&mut self) -> IrisResult<()>;
    async fn stop(&mut self) -> IrisResult<()>;
    async fn next_frame(&mut self) -> IrisResult<CaptureFrame>;
    fn is_capturing(&self) -> bool;
}

pub struct MockCaptureBackend {
    capturing: bool,
    pub sequence: u64,
    pub config: CaptureConfig,
}

impl MockCaptureBackend {
    pub fn new(config: CaptureConfig) -> Self {
        MockCaptureBackend {
            capturing: false,
            sequence: 0,
            config,
        }
    }
}

#[async_trait]
impl CaptureBackend for MockCaptureBackend {
    async fn start(&mut self) -> IrisResult<()> {
        self.capturing = true;
        self.sequence = 0;
        Ok(())
    }

    async fn stop(&mut self) -> IrisResult<()> {
        self.capturing = false;
        Ok(())
    }

    async fn next_frame(&mut self) -> IrisResult<CaptureFrame> {
        if !self.capturing {
            return Err(iris_core::error::IrisError::Capture("not capturing".into()));
        }
        tokio::time::sleep(std::time::Duration::from_millis(
            1000 / self.config.target_fps as u64,
        ))
        .await;
        self.sequence += 1;
        let fmt = self.config.format.clone();
        let size = CaptureFrame::expected_size(self.config.width, self.config.height, fmt.clone());
        let data = vec![128u8; size.max(1)];
        Ok(CaptureFrame {
            sequence: self.sequence,
            width: self.config.width,
            height: self.config.height,
            format: fmt,
            data,
            timestamp_us: CaptureFrame::now_us(),
            is_cropped: false,
        })
    }

    fn is_capturing(&self) -> bool {
        self.capturing
    }
}

// Allow boxed dynamic backends to be used where a concrete `B: CaptureBackend` is
// expected by forwarding calls to the inner boxed implementor.
#[async_trait]
impl<T: CaptureBackend + ?Sized + Send + Sync> CaptureBackend for Box<T> {
    async fn start(&mut self) -> IrisResult<()> {
        (&mut **self).start().await
    }

    async fn stop(&mut self) -> IrisResult<()> {
        (&mut **self).stop().await
    }

    async fn next_frame(&mut self) -> IrisResult<CaptureFrame> {
        (&mut **self).next_frame().await
    }

    fn is_capturing(&self) -> bool {
        (**self).is_capturing()
    }
}

/// Adapter: drive any `iris_hal::backend::UvcBackend` (WMF on Windows, V4L2 on
/// Linux) as a `CaptureBackend` — the "open → read_frame loop → CaptureFrame"
/// bridge from the Phase-2 plan.
///
/// Device selection: `IRIS_DEVICE` env (exact id or substring match) or the
/// first enumerated device. Pacing comes from the camera itself (a UVC
/// `read_frame` blocks until the next frame), so no artificial sleep.
/// `CaptureFrame.data.len()` is authoritative for size_bytes; width/height
/// reflect the configured request (the camera's current mode may differ until
/// format negotiation lands in a later block).
pub struct UvcCaptureBackend<U: iris_hal::backend::UvcBackend> {
    uvc: U,
    device: Option<iris_hal::device::DeviceId>,
    capturing: bool,
    sequence: u64,
    pub config: CaptureConfig,
}

impl<U: iris_hal::backend::UvcBackend> UvcCaptureBackend<U> {
    pub fn new(uvc: U, config: CaptureConfig) -> Self {
        UvcCaptureBackend {
            uvc,
            device: None,
            capturing: false,
            sequence: 0,
            config,
        }
    }

    fn map_err(e: iris_hal::error::HalError) -> iris_core::error::IrisError {
        iris_core::error::IrisError::Capture(format!("uvc: {e}"))
    }
}

#[async_trait]
impl<U: iris_hal::backend::UvcBackend> CaptureBackend for UvcCaptureBackend<U> {
    async fn start(&mut self) -> IrisResult<()> {
        let devices = self.uvc.enumerate_devices().await.map_err(Self::map_err)?;
        if devices.is_empty() {
            return Err(iris_core::error::IrisError::Capture(
                "no video capture devices found".into(),
            ));
        }
        let wanted = std::env::var("IRIS_DEVICE").unwrap_or_default();
        let chosen = if wanted.is_empty() {
            devices[0].clone()
        } else {
            devices
                .iter()
                .find(|d| d.id.0 == wanted || d.id.0.contains(&wanted) || d.name.contains(&wanted))
                .cloned()
                .unwrap_or_else(|| devices[0].clone())
        };
        tracing::info!("UvcCaptureBackend: opening {} ({})", chosen.name, chosen.id);
        self.uvc.open_device(&chosen.id).await.map_err(Self::map_err)?;
        // Adopt the format the device is ACTUALLY delivering so telemetry
        // (width/height/format) is authoritative, not the configured request.
        if let Ok(Some(actual)) = self.uvc.current_format(&chosen.id).await {
            if actual.width > 0 && actual.height > 0 {
                tracing::info!(
                    "UvcCaptureBackend: device mode {}x{} {}",
                    actual.width, actual.height, actual.pixel_format
                );
                self.config.width = actual.width;
                self.config.height = actual.height;
                self.config.format = actual.pixel_format;
            }
        }
        self.device = Some(chosen.id);
        self.capturing = true;
        self.sequence = 0;
        Ok(())
    }

    async fn stop(&mut self) -> IrisResult<()> {
        if let Some(id) = self.device.take() {
            // Best effort: the device may already be gone (unplug).
            let _ = self.uvc.close_device(&id).await;
        }
        self.capturing = false;
        Ok(())
    }

    async fn next_frame(&mut self) -> IrisResult<CaptureFrame> {
        if !self.capturing {
            return Err(iris_core::error::IrisError::Capture("not capturing".into()));
        }
        let id = self
            .device
            .clone()
            .ok_or_else(|| iris_core::error::IrisError::Capture("no open device".into()))?;
        let data = self.uvc.read_frame(&id).await.map_err(Self::map_err)?;
        self.sequence += 1;
        Ok(CaptureFrame {
            sequence: self.sequence,
            width: self.config.width,
            height: self.config.height,
            format: self.config.format.clone(),
            data,
            timestamp_us: CaptureFrame::now_us(),
            is_cropped: false,
        })
    }

    fn is_capturing(&self) -> bool {
        self.capturing
    }
}
