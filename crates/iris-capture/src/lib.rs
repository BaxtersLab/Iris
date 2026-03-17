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

// Re-export shared types from `iris-core` to centralize canonical shared types.
pub use iris_core::EncodedPacket;

#[cfg(test)]
mod tests;
