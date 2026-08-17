fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Determine a workspace-relative output path
    let mut out_dir = std::env::current_dir()?;
    out_dir.push("harness-output");
    std::fs::create_dir_all(&out_dir)?;

    // Prefer to generate a real MPEG-TS stream via ffmpeg. If ffmpeg is not
    // available, fall back to the deterministic JSONL telemetry writer.
    let ts_path = out_dir.join("stream.ts");
    match iris_harness::spawn_ffmpeg_stream(&ts_path, 5) {
        Ok(()) => println!("Wrote harness MPEG-TS to {}", ts_path.display()),
        Err(e) => {
            eprintln!("ffmpeg run failed ({}), falling back to JSONL writer", e);
            let json_path = out_dir.join("telemetry.jsonl");
            iris_harness::write_deterministic_telemetry(&json_path)?;
            println!("Wrote deterministic telemetry to {}", json_path.display());
        }
    }
    Ok(())
}
