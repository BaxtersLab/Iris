use crate::device::DeviceInfo;
use crate::error::HalResult;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub struct HotplugEvent {
    pub device: DeviceInfo,
    pub connected: bool,
}

pub struct HotplugHandle {
    _tx: mpsc::UnboundedSender<HotplugEvent>,
}

impl HotplugHandle {
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<HotplugEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        // In a real implementation we'd wire this to the monitor; for mock tests we just return a receiver
        drop(tx);
        rx
    }
}

pub struct HotplugMonitor {
    // placeholder internal state
    _state: Arc<Mutex<()>>,
}

impl HotplugMonitor {
    pub fn new() -> Self {
        HotplugMonitor { _state: Arc::new(Mutex::new(())) }
    }

    pub async fn run(&self, _mut_handle: HotplugHandle) -> HalResult<()> {
        // stub: real implementation would watch OS device notifications
        Ok(())
    }
}
