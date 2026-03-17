use iris_capture::backend::{CaptureBackend, CaptureConfig};
use iris_capture::DxgiCaptureBackend;
use iris_core::error::IrisResult;

#[tokio::main]
async fn main() -> IrisResult<()> {
    let cfg = CaptureConfig {
        width: 1920,
        height: 1080,
        target_fps: 30,
        format: iris_hal::device::PixelFormat::Bgr24,
        max_queue_depth: 2,
        drop_policy: iris_capture::backend::DropPolicy::Oldest,
        roi: None,
    };
    let mut backend = DxgiCaptureBackend::new(cfg);

    println!("Starting DxgiCaptureBackend test...");
    backend.start().await?;
    for _ in 0..5 {
        let frame = backend.next_frame().await?;
        println!(
            "Captured frame {} {}x{} ({} bytes)",
            frame.sequence,
            frame.width,
            frame.height,
            frame.data.len()
        );
    }
    backend.stop().await?;
    println!("Dxgi test finished.");
    Ok(())
}
