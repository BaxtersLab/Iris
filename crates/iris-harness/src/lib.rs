use std::path::Path;

pub fn write_deterministic_telemetry(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let events = vec![
        serde_json::json!({"sequence": 1, "event": "encoder_start", "ts": 0}),
        serde_json::json!({"sequence": 2, "event": "frame_captured", "ts": 33333}),
        serde_json::json!({"sequence": 3, "event": "encoder_rebase", "ts": 66666, "reason": "wrap"}),
    ];

    let mut f = std::fs::File::create(path)?;
    for e in events {
        let s = serde_json::to_string(&e)?;
        use std::io::Write;
        writeln!(f, "{}", s)?;
    }

    Ok(())
}

/// Spawn `ffmpeg` to produce a short MPEG-TS file at `out_path`.
///
/// This tries to use a test video source so it doesn't require external
/// inputs. `duration_secs` controls how long the generated stream will be.
pub fn spawn_ffmpeg_stream(out_path: &Path, duration_secs: u64) -> Result<(), Box<dyn std::error::Error>> {
    // Ensure output directory exists
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Check ffmpeg availability
    match std::process::Command::new("ffmpeg").arg("-version").output() {
        Ok(o) if o.status.success() => {},
        _ => return Err("ffmpeg not found on PATH".into()),
    }

    // Build ffmpeg command:
    // ffmpeg -f lavfi -i testsrc=size=640x480:rate=30 -t <duration> -c:v libx264 -preset ultrafast -an -f mpegts <out>
    let duration_arg = format!("{}", duration_secs);
    let out_str = out_path.to_string_lossy().to_string();

    let status = std::process::Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("warning")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg("testsrc=size=640x480:rate=30")
        .arg("-t")
        .arg(&duration_arg)
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("ultrafast")
        .arg("-an")
        .arg("-f")
        .arg("mpegts")
        .arg(&out_str)
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("ffmpeg exited with status: {}", status).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_writes_expected_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("telemetry.jsonl");
        write_deterministic_telemetry(&file)?;
        let content = std::fs::read_to_string(&file)?;
        assert!(content.contains("\"encoder_start\""));
        assert_eq!(content.lines().count(), 3);
        Ok(())
    }
}
