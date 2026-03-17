pub mod backend;
#[cfg(windows)]
pub mod dxgi_backend;
pub mod frame;
pub mod service;
pub mod telemetry;

pub use backend::*;
#[cfg(windows)]
pub use dxgi_backend::DxgiCaptureBackend;
pub use frame::*;
pub use service::*;
pub use telemetry::*;

#[cfg(test)]
mod tests;
