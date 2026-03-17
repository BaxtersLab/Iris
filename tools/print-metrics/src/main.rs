use std::net::TcpListener;
use std::io::{Read, Write};

fn main() {
    // If SERVE_METRICS is set, increment the rebase metric in-process and serve /metrics on 127.0.0.1:9180
    if std::env::var("SERVE_METRICS").is_ok() {
        iris_core::pipeline::force_increment_rebase_for_test();
        println!("metrics: incremented rebase counter (in-process)");

        // Start a minimal HTTP server to serve /metrics
        let listener = TcpListener::bind("127.0.0.1:9180").expect("bind failed");
        println!("serving metrics on http://127.0.0.1:9180/metrics");
        for stream in listener.incoming() {
            if let Ok(mut stream) = stream {
                let mut buf = [0u8; 1024];
                if let Ok(n) = stream.read(&mut buf) {
                    let req = String::from_utf8_lossy(&buf[..n]);
                    if req.starts_with("GET /metrics") {
                        let body = iris_core::pipeline::prometheus_text();
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(), body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else {
                        let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nnot found";
                        let _ = stream.write_all(resp.as_bytes());
                    }
                }
            }
        }
    } else {
        // Print Prometheus metrics exported by iris-core
        let text = iris_core::pipeline::prometheus_text();
        println!("{}", text);
    }
}
