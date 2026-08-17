Block H-2 — WebSocket Bridge
=============================

Objective
---------
Add an optional WebSocket bridge to Iris behind a `websocket` Cargo feature flag.
This allows external telemetric apps to connect via WebSocket and receive:
1. Real-time telemetry events as JSON
2. Optionally, compressed frame snapshots on demand

This is entirely additive — no existing code is modified.

Prerequisites
-------------
Blocks B-1, G-1, and (optionally) G-2 must be complete.

Feature Flag Setup
------------------

### crates/iris-ipc/Cargo.toml — add:
```toml
[features]
default = []
websocket = ["dep:tokio-tungstenite", "dep:futures-util"]

[dependencies]
tokio-tungstenite = { version = "0.21", optional = true }
futures-util = { version = "0.3", optional = true }
```

File: crates/iris-ipc/ws_bridge.rs
------------------------------------
Only compiled with `websocket` feature.

```rust
#![cfg(feature = "websocket")]

use tokio::sync::broadcast;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use futures_util::{SinkExt, StreamExt};
use super::telemetry::TelemetryEnvelope;
use super::command::IpcCommand;
use super::response::IpcResponse;
use super::envelope::{IpcEnvelope, IpcPayload};

/// Configuration for the WebSocket bridge.
pub struct WsBridgeConfig {
    /// Bind address (e.g., "127.0.0.1:9100").
    pub bind_addr: String,
    /// Maximum connected clients.
    pub max_clients: usize,
    /// Whether to forward telemetry events to WebSocket clients.
    pub forward_telemetry: bool,
    /// Whether to accept commands from WebSocket clients.
    pub accept_commands: bool,
}

impl Default for WsBridgeConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:9100".into(),
            max_clients: 8,
            forward_telemetry: true,
            accept_commands: false, // Read-only by default for security
        }
    }
}

/// The WebSocket bridge server.
pub struct WsBridge {
    config: WsBridgeConfig,
    /// Subscribe to telemetry for forwarding.
    telemetry_rx: broadcast::Receiver<TelemetryEnvelope>,
    /// Send commands received from WebSocket clients (if accept_commands=true).
    cmd_tx: Option<tokio::sync::mpsc::Sender<(IpcCommand, tokio::sync::oneshot::Sender<IpcResponse>)>>,
}

impl WsBridge {
    pub fn new(
        config: WsBridgeConfig,
        telemetry_rx: broadcast::Receiver<TelemetryEnvelope>,
        cmd_tx: Option<tokio::sync::mpsc::Sender<(IpcCommand, tokio::sync::oneshot::Sender<IpcResponse>)>>,
    ) -> Self { ... }

    /// Run the WebSocket bridge.
    /// 1. Bind TCP listener on config.bind_addr
    /// 2. Accept connections (up to max_clients)
    /// 3. For each client:
    ///    a. Subscribe to telemetry broadcast
    ///    b. Forward telemetry events as JSON text messages
    ///    c. If accept_commands: parse incoming text messages as IpcCommand JSON,
    ///       dispatch through cmd_tx, send IpcResponse back
    ///    d. On client disconnect: clean up
    /// 4. On shutdown signal: close listener, drop all clients
    pub async fn run(self) -> std::io::Result<()> {
        let listener = TcpListener::bind(&self.config.bind_addr).await?;
        tracing::info!("WebSocket bridge listening on {}", self.config.bind_addr);

        // Track active client count
        let client_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        loop {
            let (stream, addr) = listener.accept().await?;
            let current = client_count.load(std::sync::atomic::Ordering::Relaxed);
            if current >= self.config.max_clients {
                tracing::warn!("Rejecting WebSocket client {addr}: max clients reached");
                drop(stream);
                continue;
            }

            client_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let cc = client_count.clone();

            // Clone what we need for the client task
            let telemetry_rx = self.telemetry_rx.resubscribe();
            let forward_telemetry = self.config.forward_telemetry;

            tokio::spawn(async move {
                match accept_async(stream).await {
                    Ok(ws_stream) => {
                        let (mut write, mut read) = ws_stream.split();
                        tracing::info!("WebSocket client connected: {addr}");

                        if forward_telemetry {
                            let mut rx = telemetry_rx;
                            // Forward telemetry loop
                            loop {
                                match rx.recv().await {
                                    Ok(envelope) => {
                                        if let Ok(json) = serde_json::to_string(&envelope) {
                                            if write.send(
                                                tokio_tungstenite::tungstenite::Message::Text(json)
                                            ).await.is_err() {
                                                break; // Client disconnected
                                            }
                                        }
                                    }
                                    Err(broadcast::error::RecvError::Lagged(n)) => {
                                        tracing::warn!("WebSocket client {addr} lagged {n} events");
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("WebSocket handshake error for {addr}: {e}");
                    }
                }
                cc.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                tracing::info!("WebSocket client disconnected: {addr}");
            });
        }
    }
}

/// Handle for controlling the WebSocket bridge from outside.
pub struct WsBridgeHandle {
    /// Shutdown signal.
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

impl WsBridgeHandle {
    /// Signal the bridge to shut down.
    pub fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
    }
}
```

### Update iris-ipc lib.rs

Add:
```rust
#[cfg(feature = "websocket")]
pub mod ws_bridge;
```

### Security Considerations

1. By default, `accept_commands` is false — WebSocket clients can only RECEIVE
   telemetry, not send commands. This prevents unauthorized control.
2. Bind to 127.0.0.1 by default — local only. User must explicitly change to
   0.0.0.0 to expose to network.
3. Max client limit prevents resource exhaustion.
4. No authentication in this initial implementation. Document that authentication
   should be added before exposing to untrusted networks.

Unit Tests
----------
File: crates/iris-ipc/ws_bridge_tests.rs

```rust
#![cfg(feature = "websocket")]
```

### Required Tests

1. `test_ws_bridge_config_defaults` — verify default config values (bind=127.0.0.1:9100, max_clients=8, forward=true, commands=false)
2. `test_ws_bridge_telemetry_forward` — start bridge, connect client, emit telemetry, verify client receives JSON
3. `test_ws_bridge_max_clients` — set max_clients=1, connect 2 clients, verify second is rejected
4. `test_ws_bridge_client_disconnect` — connect client, disconnect, verify cleanup
5. `test_ws_bridge_json_format` — verify telemetry forwarded as valid TelemetryEnvelope JSON

### How to run tests for this block
```
cargo test -p iris-ipc --features websocket
```

Acceptance Criteria
-------------------
1. `cargo check -p iris-ipc` passes (without feature)
2. `cargo check -p iris-ipc --features websocket` passes
3. `cargo test -p iris-ipc --features websocket` — all 5 tests pass
4. Existing B-1 tests still pass without the feature
5. WebSocket bridge forwards telemetry as JSON text frames
6. Client limit is enforced
7. Default config is secure (localhost-only, read-only)
8. No code outside `#[cfg(feature = "websocket")]` is changed
