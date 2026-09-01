// The std::io reader/writer traits, Duration, std::sync::mpsc and std::thread
// were all imported here and never used — this crate handles its streams with
// tokio's async equivalents, and `handle_stream` imports `tokio::io::BufReader`
// locally where it is actually needed. `IpcHandle` stays: it is used as a type
// below. What was redundant was a second, function-local `use` of it.
use std::sync::Arc;

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
        // One binding, not two. This was declared twice — the first with a
        // double-escaped literal (`\\.\\pipe\\...`, which is not the pipe
        // name Windows wants) and immediately shadowed by the correct one, so
        // it was dead and warned on every Windows build.
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
    resolve_unix_socket_path(
        std::env::var("IRIS_IPC_SOCKET").ok(),
        std::env::var("XDG_RUNTIME_DIR").ok(),
    )
}

/// The socket-path decision, with the environment passed in rather than read.
///
/// `#[cfg(unix)]` because there is no unix socket on Windows — that side uses a
/// named pipe. Without the gate this warned "function is never used" on every
/// Windows build, which is how it was found: the zero-warning bar this crate
/// was brought into the workspace to hold was only ever checked on Linux.
///
/// Split out so it can be tested: reading the environment inside the function
/// makes the test mutate process-global state, which races every other test in
/// the binary. The precedence is `IRIS_IPC_SOCKET`, then
/// `$XDG_RUNTIME_DIR/iris-stream.sock`, then `/tmp/iris-stream.sock`.
///
/// **A unix socket path must fit in `sockaddr_un.sun_path`** — 108 bytes on
/// Linux including the terminator — and `bind` fails with "path must be shorter
/// than SUN_LEN" when it does not. That is a real failure mode, not a
/// hypothetical: it was hit while first proving this transport, because the
/// obvious scratch directory was 96 characters deep on its own. The default
/// paths here are short, but an `IRIS_IPC_SOCKET` pointing somewhere deep will
/// fail at bind time.
#[cfg(unix)]
fn resolve_unix_socket_path(
    explicit: Option<String>,
    runtime_dir: Option<String>,
) -> std::path::PathBuf {
    if let Some(p) = explicit.filter(|p| !p.is_empty()) {
        return std::path::PathBuf::from(p);
    }
    let dir = runtime_dir.filter(|d| !d.is_empty()).unwrap_or_else(|| "/tmp".to_string());
    std::path::PathBuf::from(dir).join("iris-stream.sock")
}

/// Unix-only, for two reasons that are easy to conflate. The function under
/// test does not exist on Windows — but even if it did, these assertions
/// compare against hardcoded forward-slash strings, and `PathBuf::join` inserts
/// a backslash on Windows. The round-2 Windows agent found exactly that: three
/// of these failed there with `Some("/tmp\\iris-stream.sock")`, the right
/// directory with the wrong separator. Written on Linux, never compiled for
/// Windows, shipped failing.
#[cfg(all(test, unix))]
mod socket_path_tests {
    use super::resolve_unix_socket_path;

    #[test]
    fn an_explicit_socket_wins() {
        let p = resolve_unix_socket_path(Some("/run/custom.sock".into()), Some("/run/user/1000".into()));
        assert_eq!(p.to_str(), Some("/run/custom.sock"));
    }

    #[test]
    fn the_runtime_dir_is_used_when_no_explicit_socket() {
        let p = resolve_unix_socket_path(None, Some("/run/user/1000".into()));
        assert_eq!(p.to_str(), Some("/run/user/1000/iris-stream.sock"));
    }

    #[test]
    fn tmp_is_the_last_resort() {
        let p = resolve_unix_socket_path(None, None);
        assert_eq!(p.to_str(), Some("/tmp/iris-stream.sock"));
    }

    /// An env var set to the empty string is set, and `std::env::var` returns
    /// `Ok("")` for it — which would otherwise bind a socket at `/iris-stream.sock`
    /// or at the empty path.
    #[test]
    fn empty_env_values_are_treated_as_unset() {
        assert_eq!(
            resolve_unix_socket_path(Some(String::new()), Some(String::new())).to_str(),
            Some("/tmp/iris-stream.sock")
        );
    }

    /// The default paths must leave room inside sockaddr_un's 108-byte
    /// sun_path, or the bridge cannot bind at all on a normal system.
    #[test]
    fn the_default_paths_fit_in_sun_path() {
        const SUN_PATH_MAX: usize = 108;
        for p in [
            resolve_unix_socket_path(None, None),
            resolve_unix_socket_path(None, Some("/run/user/1000".into())),
        ] {
            let len = p.as_os_str().len();
            assert!(len < SUN_PATH_MAX, "{p:?} is {len} bytes, too long to bind");
        }
    }
}

mod iris_dispatcher {
    use iris_ipc::response::{IpcResponse, ResponseData};
    use iris_ipc::command::IpcCommand;
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
    let reader = BufReader::new(read_half);
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
