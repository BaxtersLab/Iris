pub mod frame;
pub mod backend;
pub mod service;
pub mod telemetry;
#[cfg(windows)]
pub mod dxgi_backend;

pub use frame::*;
pub use backend::*;
pub use service::*;
pub use telemetry::*;
#[cfg(windows)]
pub use dxgi_backend::DxgiCaptureBackend;

#[cfg(test)]
mod tests;
