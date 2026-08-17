// SPDX-License-Identifier: MIT
// Iris — iris-harness
//
// Generates test inputs for downstream consumers (RemoteDexter agents,
// bridge clients): a real MPEG-TS stream when ffmpeg is available, or a
// deterministic JSONL file of real `TelemetryEnvelope` records.

use chrono::TimeZone;
use iris_ipc::telemetry::{TelemetryEnvelope, TelemetryEvent};
use std::io::Write;
use std::path::Path;

/// Generate a short MPEG-TS test stream via the ffmpeg CLI (testsrc pattern).
/// Errors (as a String) when ffmpeg is missing or fails, so callers can fall
/// back to the deterministic writer.
pub fn spawn_ffmpeg_stream(out: &Path, seconds: u32) -> Result<(), String> {
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=duration={seconds}:size=640x480:rate=30"),
            "-c:v",
            "mpeg2video",
            "-f",
            "mpegts",
        ])
        .arg(out)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("ffmpeg spawn failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("ffmpeg exited with {status}"))
    }
}

/// Write 30 deterministic `TelemetryEnvelope` JSON lines (fixed timestamps,
/// fixed sizes) — byte-identical on every run, for golden-file style tests.
pub fn write_deterministic_telemetry(path: &Path) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    for seq in 0..30u64 {
        let env = TelemetryEnvelope {
            timestamp: chrono::Utc
                .timestamp_opt(1_700_000_000 + seq as i64, 0)
                .single()
                .expect("fixed timestamp valid"),
            sequence: seq,
            event: TelemetryEvent::FrameCaptured {
                sequence: seq + 1,
                width: 1920,
                height: 1080,
                size_bytes: 3_110_400, // 1920x1080 NV12
            },
        };
        let line = serde_json::to_string(&env)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        writeln!(f, "{line}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_telemetry_is_parseable_and_stable() {
        let td = tempfile::tempdir().unwrap();
        let a = td.path().join("a.jsonl");
        let b = td.path().join("b.jsonl");
        write_deterministic_telemetry(&a).unwrap();
        write_deterministic_telemetry(&b).unwrap();

        let text_a = std::fs::read_to_string(&a).unwrap();
        let text_b = std::fs::read_to_string(&b).unwrap();
        assert_eq!(text_a, text_b, "output must be byte-identical across runs");

        let lines: Vec<&str> = text_a.lines().collect();
        assert_eq!(lines.len(), 30);
        for line in lines {
            let env: TelemetryEnvelope = serde_json::from_str(line).unwrap();
            match env.event {
                TelemetryEvent::FrameCaptured { size_bytes, .. } => {
                    assert_eq!(size_bytes, 3_110_400)
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
    }

    /// ffmpeg is a required test dependency (see DEPENDENCIES.md), so this
    /// asserts `Ok` outright rather than tolerating the error branch. The
    /// earlier dual-branch version passed whether or not ffmpeg existed, which
    /// meant it proved nothing on a box without it.
    #[test]
    fn ffmpeg_produces_a_real_mpegts_stream() {
        let td = tempfile::tempdir().unwrap();
        let ts = td.path().join("s.ts");
        spawn_ffmpeg_stream(&ts, 1)
            .expect("ffmpeg must be installed to run this suite — see DEPENDENCIES.md");
        assert!(ts.exists(), "ffmpeg reported success but wrote no file");
        assert!(
            ts.metadata().unwrap().len() > 0,
            "ffmpeg wrote an empty stream"
        );
    }
}
