//! Loopback smoke client for the unix-socket transport (Linux).
//!
//! Connects to the bridge's socket, sends Ping then StartCapture as JSON-line
//! envelopes, and verifies a Pong response plus at least one FrameCaptured
//! telemetry line arrives. Exit 0 = transport round-trip proven.
//!
//! Usage: uds_smoke_client <socket-path>

#[cfg(unix)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/iris-stream.sock".to_string());
    let stream = tokio::net::UnixStream::connect(&path).await?;
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    // 1) Ping
    write_half
        .write_all(b"{\"id\":1,\"payload\":{\"type\":\"Command\",\"body\":{\"cmd\":\"Ping\"}}}\n")
        .await?;
    // 2) StartCapture (spawns the mock 30fps telemetry producer)
    write_half
        .write_all(
            b"{\"id\":2,\"payload\":{\"type\":\"Command\",\"body\":{\"cmd\":\"StartCapture\"}}}\n",
        )
        .await?;
    write_half.flush().await?;

    let mut got_pong = false;
    let mut got_frame = false;
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    while !(got_pong && got_frame) {
        let line = tokio::select! {
            l = lines.next_line() => l?,
            _ = tokio::time::sleep_until(deadline) => break,
        };
        let Some(line) = line else { break };
        if line.contains("\"Pong\"") {
            println!("client: got Pong");
            got_pong = true;
        }
        if line.contains("\"FrameCaptured\"") {
            if !got_frame {
                println!("client: got FrameCaptured telemetry");
            }
            got_frame = true;
        }
    }

    if got_pong && got_frame {
        println!("UDS SMOKE OK");
        Ok(())
    } else {
        anyhow::bail!("UDS SMOKE FAIL: pong={got_pong} frame={got_frame}")
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("uds_smoke_client is unix-only");
}
