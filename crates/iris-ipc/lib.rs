// SPDX-License-Identifier: MIT
// Iris — iris-ipc
pub mod client;
pub mod command;
pub mod envelope;
pub mod response;
pub mod server;
pub mod telemetry;

pub use server::{IpcHandle, IpcServer, LoggedTelemetryReceiver};

use std::future::Future;
use std::pin::Pin;

/// Trait for IPC command dispatchers. Implement this in the runtime (iris-ui)
/// to route commands from the `IpcServer` to service handles.
pub trait Dispatcher: Send + 'static {
    fn dispatch(
        &mut self,
        cmd: command::IpcCommand,
    ) -> Pin<Box<dyn Future<Output = response::IpcResponse> + Send>>;
}

#[cfg(test)]
mod tests;
