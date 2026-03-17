#![cfg(windows)]

use regex::Regex;
use std::process::Child;
use std::process::Command;
use std::time::{Duration, Instant};

#[tokio::test]
async fn gui_smoke_force_rebase_and_metrics() {
    // Start iris-ui via `cargo run -p iris-ui` in a child process.
    let mut child: Child = Command::new("cargo")
        .args(["run", "-p", "iris-ui", "--quiet"])
        .spawn()
        .expect("failed to start iris-ui");

    let client = reqwest::Client::new();
    let metrics_url = "http://127.0.0.1:9180/metrics";
    let rebase_url = "http://127.0.0.1:9180/debug/force_rebase";

    // Wait for metrics endpoint to become available (30s timeout).
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut ok = false;
    while Instant::now() < deadline {
        match client.get(metrics_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                ok = true;
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }

    assert!(ok, "metrics endpoint did not become available");

    // Trigger an in-process rebase and wait briefly
    client
        .get(rebase_url)
        .send()
        .await
        .expect("failed to call /debug/force_rebase");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Fetch metrics and assert iris_encoder_rebase_total == 1
    let resp = client.get(metrics_url).send().await.expect("metrics request");
    let body = resp.text().await.expect("metrics body");

    let re = Regex::new(r"iris_encoder_rebase_total\{[^}]*\}\s*(\d+)").unwrap();
    let caps = re.captures(&body).expect("metric iris_encoder_rebase_total not found");
    let val: i32 = caps.get(1).unwrap().as_str().parse().unwrap();
    assert_eq!(val, 1, "expected iris_encoder_rebase_total == 1");

    // Clean up child process
    let _ = child.kill();
}
