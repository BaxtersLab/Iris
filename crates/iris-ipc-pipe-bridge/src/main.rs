use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;
use std::time::Duration;
use std::sync::mpsc as std_mpsc;
use std::thread;

use iris_ipc::{
    envelope::IpcEnvelope,
    server::{IpcHandle, IpcServer},
};
use tokio::sync::mpsc as tokio_mpsc;
use tokio::task;

#[cfg(windows)]
mod pipe {
    //! Async named-pipe acceptor using tokio's Windows named-pipe APIs.
    #[cfg(windows)]
    pub async fn accept_client(
        name: &str,
    ) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
        use tokio::net::windows::named_pipe::ServerOptions;

        // Create a new instance for a client and wait for connection.
        // Ensure we pass a full pipe path accepted by ServerOptions::create
        // (for example "\\\\.\\pipe\\<name>"). If the caller supplied
        // a short name (no backslashes), prefix it.
        let full_name = if name.contains('\\') {
            name.to_string()
        } else {
            format!("\\\\.\\pipe\\{}", name)
        };

        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&full_name)?;
        server.connect().await.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("connect error: {:?}", e)))?;
        Ok(server)
    }

    #[cfg(not(windows))]
    pub async fn accept_client(_name: &str) -> std::io::Result<()> {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "named pipes only supported on Windows"))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1) Create in-process IpcServer
    let (ipc_server, ipc_handle, telemetry_tx) = IpcServer::new(256);
    // Wrap handle in Arc so it can be cheaply cloned into per-request tasks
    let ipc_handle = Arc::new(ipc_handle);

    // 2) Spawn dispatcher loop - minimal echo behavior for commands
    task::spawn(async move {
        ipc_server.run_with_dispatcher(iris_dispatcher::Dispatcher::new()).await;
    });

    // 3) Telemetry forwarding channel (async)
    let (telemetry_out_tx, telemetry_out_rx) = tokio_mpsc::unbounded_channel::<iris_ipc::telemetry::TelemetryEnvelope>();
    let mut telemetry_sub = telemetry_tx.subscribe();
    task::spawn(async move {
        loop {
            match telemetry_sub.recv().await {
                Ok(env) => { let _ = telemetry_out_tx.send(env); }
                Err(e) => { eprintln!("telemetry recv error: {:?}", e); break; }
            }
        }
    });

    #[cfg(windows)]
    {
        let pipe_name = r"\\.\\pipe\\iris-stream";
        // prefer readable literal for CreateNamedPipeW
        let pipe_name = r"\\.\pipe\iris-stream";
        eprintln!("iris-ipc-pipe-bridge: waiting for client on {pipe_name}...");
        let server = pipe::accept_client(pipe_name).await?;
        eprintln!("iris-ipc-pipe-bridge: client connected.");

        run_json_line_protocol(server, ipc_handle.clone(), telemetry_out_rx, telemetry_tx.clone()).await?;
    }

    // Linux/unix transport: a unix-domain socket in place of the named pipe
    // (per the Iris design doc: named pipes on Windows, unix sockets on Linux).
    // Same accept-one-client / JSON-lines semantics as the Windows path.
    #[cfg(unix)]
    {
        let sock_path = unix_socket_path();
        // remove a stale socket from a previous run
        let _ = std::fs::remove_file(&sock_path);
        let listener = tokio::net::UnixListener::bind(&sock_path)?;
        eprintln!("iris-ipc-pipe-bridge: waiting for client on {}...", sock_path.display());
        let (stream, _addr) = listener.accept().await?;
        eprintln!("iris-ipc-pipe-bridge: client connected.");

        let result =
            run_json_line_protocol(stream, ipc_handle.clone(), telemetry_out_rx, telemetry_tx.clone())
                .await;
        let _ = std::fs::remove_file(&sock_path);
        result?;
    }

    Ok(())
}

/// Socket path: $IRIS_IPC_SOCKET, else $XDG_RUNTIME_DIR/iris-stream.sock,
/// else /tmp/iris-stream.sock.
#[cfg(unix)]
fn unix_socket_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("IRIS_IPC_SOCKET") {
        return std::path::PathBuf::from(p);
    }
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(dir).join("iris-stream.sock")
}

mod iris_dispatcher {
    use iris_ipc::response::{IpcResponse, ResponseData};
    use iris_ipc::command::IpcCommand;
    use iris_ipc::server::IpcHandle;
    use std::pin::Pin;
    use std::future::Future;

    pub struct Dispatcher;
    impl Dispatcher {
        pub fn new() -> Self { Self }
    }

    impl iris_ipc::Dispatcher for Dispatcher {
        fn dispatch(&mut self, cmd: IpcCommand) -> Pin<Box<dyn Future<Output = IpcResponse> + Send>> {
            Box::pin(async move {
                match cmd {
                    IpcCommand::Ping => IpcResponse::Ok(ResponseData::Pong { uptime_ms: 0 }),
                    IpcCommand::ListDevices => {
                        IpcResponse::Ok(ResponseData::DeviceList { devices: vec![] })
                    }
                    IpcCommand::Subscribe => IpcResponse::Ok(ResponseData::SubscriberId { id: 1 }),
                    _ => IpcResponse::Ok(ResponseData::Empty),
                }
            })
        }
    }
}

async fn run_json_line_protocol<S>(
    server: S,
    ipc_handle: Arc<IpcHandle>,
    mut telemetry_rx: tokio_mpsc::UnboundedReceiver<iris_ipc::telemetry::TelemetryEnvelope>,
    telemetry_tx: tokio::sync::broadcast::Sender<iris_ipc::telemetry::TelemetryEnvelope>,
) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::Mutex;

    // Split stream into read/write halves and protect writer with a mutex for atomic writes
    let (read_half, write_half) = tokio::io::split(server);
    let mut reader = BufReader::new(read_half);
    let writer = Arc::new(Mutex::new(write_half));

    // Telemetry forwarder: write telemetry envelopes as JSON lines
    let telemetry_writer = writer.clone();
    tokio::spawn(async move {
        while let Some(env) = telemetry_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&env) {
                let mut w = telemetry_writer.lock().await;
                let _ = w.write_all(json.as_bytes()).await;
                let _ = w.write_all(b"\n").await;
                let _ = w.flush().await;
            }
        }
    });

    // Capture producer control flag (shared)
    use std::sync::atomic::{AtomicBool, Ordering};
    let capture_running = std::sync::Arc::new(AtomicBool::new(false));

    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        let env: IpcEnvelope = match serde_json::from_str(trimmed) {
            Ok(e) => e,
            Err(e) => { eprintln!("Failed to parse IpcEnvelope: {:?} line={} ", e, trimmed); continue; }
        };

        let ipc = ipc_handle.clone();
        let writer_clone = writer.clone();
        let capture_running_clone = capture_running.clone();
        let telemetry_tx_clone = telemetry_tx.clone();

        tokio::spawn(async move {
            if let iris_ipc::envelope::IpcPayload::Command(cmd) = env.payload {
                // Send the command to the in-process IPC server
                match ipc.send_command(cmd.clone()).await {
                    Ok(resp) => {
                        // Write response back to client and flush.
                        let resp_env = iris_ipc::envelope::IpcEnvelope { id: env.id, payload: iris_ipc::envelope::IpcPayload::Response(resp.clone()) };
                        if let Ok(json) = serde_json::to_string(&resp_env) {
                            let mut w = writer_clone.lock().await;
                            let _ = w.write_all(json.as_bytes()).await;
                            let _ = w.write_all(b"\n").await;
                            let _ = w.flush().await;
                        }

                        // Only after the response has been flushed do we start/stop the mock producer.
                        use iris_ipc::command::IpcCommand as Cmd;
                        match cmd {
                            Cmd::StartCapture => {
                                if matches!(resp, iris_ipc::response::IpcResponse::Ok(_)) {
                                    if !capture_running_clone.swap(true, Ordering::SeqCst) {
                                        // spawn a producer that emits FrameCaptured telemetry at ~30fps
                                        let running = capture_running_clone.clone();
                                        let tx = telemetry_tx_clone.clone();
                                        tokio::spawn(async move {
                                            let mut seq: u64 = 0;
                                            while running.load(Ordering::SeqCst) {
                                                let event = iris_ipc::telemetry::TelemetryEvent::FrameCaptured { sequence: seq, width: 1920, height: 1080, size_bytes: 200_000 };
                                                let env = iris_ipc::telemetry::TelemetryEnvelope { timestamp: chrono::Utc::now(), sequence: seq, event };
                                                let _ = tx.send(env);
                                                seq = seq.wrapping_add(1);
                                                tokio::time::sleep(tokio::time::Duration::from_millis(33)).await;
                                            }
                                        });
                                    }
                                }
                            }
                            Cmd::StopCapture | Cmd::PauseCapture => {
                                capture_running_clone.store(false, Ordering::SeqCst);
                            }
                            _ => {}
                        }
                    }
                    Err(e) => {
                        eprintln!("send_command error: {:?}", e);
                    }
                }
            }
        });
    }

    Ok(())
}
