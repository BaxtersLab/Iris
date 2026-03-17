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
