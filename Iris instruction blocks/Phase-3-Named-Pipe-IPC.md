================================================================================
PHASE 3 — REAL WINDOWS NAMED PIPE IPC
Baxter's Screen Record — Agent 2 Execution Block
================================================================================

PHASE:          3 of 4
MODULE:         E — IPC (bsr-ipc)
CRATE:          bsr-ipc
DEPENDS ON:     Phase 2 complete (pipeline wired), existing IpcCommand/
                IpcResponse/TelemetryEvent enums, MuxerConfig
PRIOR STATE:    IpcServer and IpcClient are MOCK in-process implementations
                using tokio mpsc channels. All commands and response types
                are defined and working. 4 tests pass. No real Named Pipes.

================================================================================
PURPOSE
================================================================================

Replace the mock in-process IPC with real Windows Named Pipes so that:

  1. The BSR recording service can run as a background process
  2. The UI (or Hot Rod Tuner, or CLI tools) can control recording from
     a separate process via named pipes
  3. Multiple clients can connect (semaphore-limited)
  4. Telemetry flows from the service to any connected listener

This enables the split-process architecture where:
  - bsr-service.exe runs headless (capture → encode → mux)
  - bsr-ui.exe connects via named pipe to control it
  - bsr-hrt.exe (Hot Rod Tuner) connects via named pipe for monitoring

================================================================================
PIPE ARCHITECTURE
================================================================================

Two named pipes, both using newline-delimited JSON:

    \\.\pipe\bsr-command
        Direction:  Client → Server
        Protocol:   Client sends IpcCommand as JSON + '\n'
                    Server replies with IpcResponse as JSON + '\n'
        Pattern:    Request-response (synchronous per-message)

    \\.\pipe\bsr-telemetry
        Direction:  Server → Client (push)
        Protocol:   Server sends TelemetryEvent as JSON + '\n'
        Pattern:    Streaming (server pushes events continuously)

Both pipes:
  - JSON serialization (serde_json) — already used throughout workspace
  - Newline-delimited (each message is one line terminated by \n)
  - Max message size: 64 KB (reject larger messages)
  - Connection limit: 4 simultaneous clients (semaphore)

================================================================================
STEP-BY-STEP IMPLEMENTATION
================================================================================

STEP 1: Add tokio Named Pipe dependencies
-------------------------------------------

bsr-ipc/Cargo.toml already has tokio. Ensure feature flags include:

    tokio = { version = "1", features = ["full", "net"] }

On Windows, tokio provides:
    tokio::net::windows::named_pipe::{ServerOptions, NamedPipeServer,
                                       ClientOptions, NamedPipeClient}

No additional crates needed — tokio's named pipe support is built-in.

Also add:
    tokio = { version = "1", features = ["io-util"] }   # for AsyncBufReadExt

Verify: cargo check -p bsr-ipc

STEP 2: Create named_pipe module
----------------------------------

Create: crates/bsr-ipc/src/named_pipe.rs

Add to lib.rs:
    #[cfg(windows)]
    pub mod named_pipe;

STEP 3: Implement NamedPipeServer
----------------------------------

    use tokio::net::windows::named_pipe::{ServerOptions, NamedPipeServer as TokioServer};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::{broadcast, mpsc, Semaphore};
    use std::sync::Arc;

    const COMMAND_PIPE: &str = r"\\.\pipe\bsr-command";
    const TELEMETRY_PIPE: &str = r"\\.\pipe\bsr-telemetry";
    const MAX_CONNECTIONS: usize = 4;
    const MAX_MESSAGE_SIZE: usize = 65536;

    pub struct PipeServer {
        command_handler: Option<tokio::task::JoinHandle<()>>,
        telemetry_handler: Option<tokio::task::JoinHandle<()>>,
        shutdown_tx: mpsc::Sender<()>,
    }

    impl PipeServer {
        /// Start the named pipe server.
        ///
        /// - `command_tx`: channel to forward received IpcCommands to the
        ///   recording pipeline
        /// - `telemetry_rx`: broadcast receiver for TelemetryEvents to push
        ///   to connected clients
        pub async fn start(
            command_tx: mpsc::Sender<super::IpcCommand>,
            response_rx: mpsc::Receiver<super::IpcResponse>,
            telemetry_source: broadcast::Sender<super::TelemetryEvent>,
        ) -> Result<Self, std::io::Error> {
            let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);
            let semaphore = Arc::new(Semaphore::new(MAX_CONNECTIONS));

            // Spawn command pipe listener
            let cmd_sem = semaphore.clone();
            let mut cmd_shutdown = shutdown_tx.subscribe(); // ← see note
            let command_handler = tokio::spawn(async move {
                Self::run_command_pipe(command_tx, cmd_sem).await;
            });

            // Spawn telemetry pipe broadcaster
            let telemetry_handler = tokio::spawn(async move {
                Self::run_telemetry_pipe(telemetry_source, semaphore).await;
            });

            Ok(Self {
                command_handler: Some(command_handler),
                telemetry_handler: Some(telemetry_handler),
                shutdown_tx,
            })
        }

        async fn run_command_pipe(
            command_tx: mpsc::Sender<super::IpcCommand>,
            semaphore: Arc<Semaphore>,
        ) {
            loop {
                // Create a new pipe instance for each connection
                let server = match ServerOptions::new()
                    .first_pipe_instance(false)
                    .create(COMMAND_PIPE)
                {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("Failed to create command pipe: {e}");
                        return;
                    }
                };

                // Wait for a client to connect
                if let Err(e) = server.connect().await {
                    tracing::error!("Command pipe connect error: {e}");
                    continue;
                }

                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        tracing::warn!("Max connections reached, rejecting");
                        drop(server);
                        continue;
                    }
                };

                let tx = command_tx.clone();
                tokio::spawn(async move {
                    Self::handle_command_client(server, tx).await;
                    drop(permit); // release semaphore slot
                });
            }
        }

        async fn handle_command_client(
            pipe: TokioServer,
            command_tx: mpsc::Sender<super::IpcCommand>,
        ) {
            let (reader, mut writer) = tokio::io::split(pipe);
            let mut lines = BufReader::new(reader).lines();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.len() > MAX_MESSAGE_SIZE {
                    tracing::warn!("Message too large ({} bytes), dropping",
                                   line.len());
                    continue;
                }

                match serde_json::from_str::<super::IpcCommand>(&line) {
                    Ok(cmd) => {
                        tracing::debug!("Received command: {cmd:?}");
                        if command_tx.send(cmd).await.is_err() {
                            tracing::error!("Command handler dropped");
                            break;
                        }
                        // Send acknowledgment
                        let resp = super::IpcResponse::Ok;
                        let mut resp_json = serde_json::to_string(&resp)
                            .unwrap_or_default();
                        resp_json.push('\n');
                        if writer.write_all(resp_json.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Invalid command JSON: {e}");
                        let err_resp = super::IpcResponse::Error(
                            format!("Invalid JSON: {e}")
                        );
                        let mut resp_json = serde_json::to_string(&err_resp)
                            .unwrap_or_default();
                        resp_json.push('\n');
                        if writer.write_all(resp_json.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
            }
            tracing::debug!("Command client disconnected");
        }

        async fn run_telemetry_pipe(
            telemetry_source: broadcast::Sender<super::TelemetryEvent>,
            semaphore: Arc<Semaphore>,
        ) {
            loop {
                let server = match ServerOptions::new()
                    .first_pipe_instance(false)
                    .create(TELEMETRY_PIPE)
                {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("Failed to create telemetry pipe: {e}");
                        return;
                    }
                };

                if let Err(e) = server.connect().await {
                    tracing::error!("Telemetry pipe connect error: {e}");
                    continue;
                }

                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        tracing::warn!("Max connections, rejecting telemetry");
                        drop(server);
                        continue;
                    }
                };

                let mut rx = telemetry_source.subscribe();
                tokio::spawn(async move {
                    Self::handle_telemetry_client(server, &mut rx).await;
                    drop(permit);
                });
            }
        }

        async fn handle_telemetry_client(
            pipe: TokioServer,
            rx: &mut broadcast::Receiver<super::TelemetryEvent>,
        ) {
            let (_, mut writer) = tokio::io::split(pipe);

            while let Ok(event) = rx.recv().await {
                let mut json = match serde_json::to_string(&event) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::warn!("Failed to serialize telemetry: {e}");
                        continue;
                    }
                };
                json.push('\n');
                if writer.write_all(json.as_bytes()).await.is_err() {
                    break; // client disconnected
                }
            }
            tracing::debug!("Telemetry client disconnected");
        }

        pub async fn stop(&mut self) {
            let _ = self.shutdown_tx.send(()).await;
            if let Some(h) = self.command_handler.take() {
                h.abort(); // pipe listeners loop forever
            }
            if let Some(h) = self.telemetry_handler.take() {
                h.abort();
            }
        }
    }

STEP 4: Implement NamedPipeClient
-----------------------------------

    pub struct PipeClient {
        command_pipe_name: String,
        telemetry_pipe_name: String,
    }

    impl PipeClient {
        pub fn new() -> Self {
            Self {
                command_pipe_name: COMMAND_PIPE.to_string(),
                telemetry_pipe_name: TELEMETRY_PIPE.to_string(),
            }
        }

        /// Send a command and wait for the response.
        pub async fn send_command(
            &self,
            cmd: &super::IpcCommand,
        ) -> Result<super::IpcResponse, IpcError> {
            use tokio::net::windows::named_pipe::ClientOptions;

            let client = ClientOptions::new()
                .open(&self.command_pipe_name)
                .map_err(|e| IpcError::ConnectionFailed(format!("{e}")))?;

            let (reader, mut writer) = tokio::io::split(client);

            // Send command as JSON line
            let mut json = serde_json::to_string(cmd)
                .map_err(|e| IpcError::SerializationFailed(format!("{e}")))?;
            json.push('\n');
            writer.write_all(json.as_bytes()).await
                .map_err(|e| IpcError::WriteFailed(format!("{e}")))?;

            // Read response line
            let mut lines = BufReader::new(reader).lines();
            let resp_line = lines.next_line().await
                .map_err(|e| IpcError::ReadFailed(format!("{e}")))?
                .ok_or(IpcError::ConnectionClosed)?;

            serde_json::from_str(&resp_line)
                .map_err(|e| IpcError::DeserializationFailed(format!("{e}")))
        }

        /// Subscribe to telemetry events (streaming).
        /// Returns a receiver that yields events as they arrive.
        pub async fn subscribe_telemetry(
            &self,
        ) -> Result<mpsc::Receiver<super::TelemetryEvent>, IpcError> {
            use tokio::net::windows::named_pipe::ClientOptions;

            let client = ClientOptions::new()
                .open(&self.telemetry_pipe_name)
                .map_err(|e| IpcError::ConnectionFailed(format!("{e}")))?;

            let (tx, rx) = mpsc::channel(64);

            tokio::spawn(async move {
                let reader = BufReader::new(client);
                let mut lines = reader.lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    match serde_json::from_str::<super::TelemetryEvent>(&line) {
                        Ok(event) => {
                            if tx.send(event).await.is_err() {
                                break; // receiver dropped
                            }
                        }
                        Err(e) => tracing::warn!("Bad telemetry JSON: {e}"),
                    }
                }
            });

            Ok(rx)
        }
    }

    #[derive(Debug, thiserror::Error)]
    pub enum IpcError {
        #[error("Connection failed: {0}")]
        ConnectionFailed(String),
        #[error("Connection closed")]
        ConnectionClosed,
        #[error("Serialization failed: {0}")]
        SerializationFailed(String),
        #[error("Deserialization failed: {0}")]
        DeserializationFailed(String),
        #[error("Write failed: {0}")]
        WriteFailed(String),
        #[error("Read failed: {0}")]
        ReadFailed(String),
    }

NOTE: Add `thiserror` to bsr-ipc/Cargo.toml if not already present.
If you want to avoid the dependency, implement Display + Error manually.

STEP 5: Keep backward compatibility — in-process mode
-------------------------------------------------------

DO NOT remove the existing mock IpcServer/IpcClient. Instead:

    pub enum IpcTransport {
        /// In-process mock (for tests and single-process mode)
        InProcess {
            server: IpcServer,
            client: IpcClient,
        },
        /// Real Windows Named Pipes (for production split-process)
        #[cfg(windows)]
        NamedPipe {
            server: named_pipe::PipeServer,
            client: named_pipe::PipeClient,
        },
    }

Or simpler: keep both modules, let the caller choose. Tests continue to
use the mock IpcServer::new_pair(). Production uses PipeServer/PipeClient.

STEP 6: Add integration test
------------------------------

    #[cfg(windows)]
    #[tokio::test]
    async fn test_named_pipe_roundtrip() {
        use super::named_pipe::{PipeServer, PipeClient};

        let (cmd_tx, mut cmd_rx) = mpsc::channel(4);
        let (telemetry_tx, _) = broadcast::channel(16);

        // Start server
        let mut server = PipeServer::start(
            cmd_tx, telemetry_tx.clone()
        ).await.unwrap();

        // Give pipe time to be created
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Connect client and send command
        let client = PipeClient::new();
        let resp = client.send_command(
            &super::IpcCommand::GetStatus
        ).await.unwrap();

        // Server should have received the command
        let received = cmd_rx.try_recv().unwrap();
        assert!(matches!(received, super::IpcCommand::GetStatus));

        // Response should be Ok
        assert!(matches!(resp, super::IpcResponse::Ok));

        server.stop().await;
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_named_pipe_telemetry_stream() {
        use super::named_pipe::{PipeServer, PipeClient};

        let (cmd_tx, _) = mpsc::channel(4);
        let (telemetry_tx, _) = broadcast::channel(16);

        let mut server = PipeServer::start(
            cmd_tx, telemetry_tx.clone()
        ).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let client = PipeClient::new();
        let mut telemetry_rx = client.subscribe_telemetry().await.unwrap();

        // Wait for subscription to establish
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Server broadcasts a telemetry event
        telemetry_tx.send(super::TelemetryEvent::FpsUpdate(30.0)).ok();

        // Client should receive it
        let event = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            telemetry_rx.recv(),
        ).await.unwrap().unwrap();

        assert!(matches!(event, super::TelemetryEvent::FpsUpdate(fps) if fps == 30.0));

        server.stop().await;
    }

================================================================================
SECURITY CONSIDERATIONS
================================================================================

- Named pipes are accessible to any process on the same machine by default.
  For BSR this is acceptable because:
    - The command pipe only accepts IpcCommand variants (no arbitrary code exec)
    - Telemetry is read-only (no mutation possible)
    - Max message size prevents DoS via large payloads
    - Semaphore prevents connection flooding

- If future versions need cross-user security, add a SECURITY_ATTRIBUTES
  with a discretionary ACL that restricts to the current user's SID. For
  now, default ACL is sufficient.

- JSON deserialization uses serde_json which rejects malformed input safely.

================================================================================
ACCEPTANCE CRITERIA
================================================================================

1.  cargo check --workspace compiles clean
2.  cargo test --workspace — all existing tests pass (≥34 + Phase 2 additions)
3.  test_named_pipe_roundtrip passes
4.  test_named_pipe_telemetry_stream passes
5.  Existing in-process IPC tests still pass (backward compat)
6.  PipeServer handles max 4 connections (rejects 5th)
7.  Messages > 64KB are rejected with a warning
8.  Client reconnects cleanly after server restart

================================================================================
NOTES FOR BUILDER AGENT
================================================================================

- tokio's named pipe API is Windows-only. Gate all named_pipe code behind
  #[cfg(windows)].
- The ServerOptions/ClientOptions API changed between tokio versions.
  Check the version in the workspace Cargo.lock and use matching docs.
- Named pipe server must create a NEW pipe instance before calling
  .connect() each time. This is unlike TCP where accept() returns new
  connections — with named pipes, each instance serves one client.
- BufReader::lines() reads until '\n'. This matches our newline-delimited
  JSON protocol perfectly.
- If split() is not available on NamedPipeServer, use Arc<Mutex<>> or
  separate read/write half types depending on tokio version.
- Run cargo test --workspace after every change.

================================================================================
VERIFICATION COMMANDS
================================================================================

    cd '<screen-recorder-project-root>'
    $env:VCPKG_ROOT = "C:\tools\vcpkg"
    $env:LIBCLANG_PATH = "C:\tools\LLVM\bin"

    cargo check --workspace
    cargo test --workspace
    cargo test -p bsr-ipc test_named_pipe -- --nocapture

================================================================================
COMMIT MESSAGE
================================================================================

    Phase-3: Real Windows Named Pipe IPC — command + telemetry pipes, JSON protocol

================================================================================
END OF PHASE 3
================================================================================
