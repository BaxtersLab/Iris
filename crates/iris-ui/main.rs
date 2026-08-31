#[tokio::main]
async fn main() {
    // Initialize logging from `RUST_LOG` (so debug output reaches stdout)
    let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    if let Err(e) = iris_core::logging::init_logging(&log_level, false, ".") {
        eprintln!("failed to init logging: {}", e);
    }

    // Load `iris.toml` (next to the executable) if present, else fall back to
    // defaults. Previously this called `IrisConfig::default()` unconditionally,
    // so the whole config-file mechanism — `IrisConfig::load()`/`save()`/
    // `config_path()` and any `iris.toml` on disk — was unreachable, and capture
    // resolution was permanently pinned to the 3840x2160 default. Article XI §3:
    // route through the configurable mechanism that already exists.
    let cfg = match iris_core::config::IrisConfig::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("config load failed ({e}); falling back to defaults");
            iris_core::config::IrisConfig::default()
        }
    };

    // Validate what was loaded. `IrisConfig::validate()` has existed since the
    // first Iris config and had **no caller outside its own tests** — a file
    // with a 99999-pixel width, an unknown drop policy or a nonsense log level
    // was accepted in full and surfaced later as strange behaviour rather than
    // an error. A bad file falls back to defaults instead of refusing to start,
    // matching how a parse failure above is handled.
    let cfg = match cfg.validate() {
        Ok(()) => cfg,
        Err(e) => {
            eprintln!("config invalid ({e}); falling back to defaults");
            iris_core::config::IrisConfig::default()
        }
    };
    match iris_ui::bootstrap::IrisRuntime::bootstrap(cfg).await {
        Ok(_rt) => {
            // Keep runtime alive while UI runs; pass the IPC handle into the UI so it can send commands and subscribe to telemetry.
            let rt = _rt; // keep ownership
            let ipc = std::sync::Arc::new(rt.ipc_handle);
            let capture_handle = rt.capture_handle;

            // If headless mode is requested, run a scripted interaction sequence and exit.
            if std::env::var("IRIS_UI_HEADLESS").unwrap_or_default() == "1" {
                // subscribe to telemetry and print a few events while exercising commands
                let mut telem_sub = ipc.subscribe_telemetry();
                let ipc_clone = ipc.clone();
                tokio::spawn(async move {
                    // Drain telemetry in background
                    while let Ok(env) = telem_sub.recv().await {
                        println!("TELEMETRY: {:?}", env);
                    }
                });

                // Run scripted commands
                let ic = ipc_clone.clone();
                tokio::spawn(async move {
                    use iris_ipc::command::IpcCommand;
                    // List devices
                    if let Ok(resp) = ic.send_command(IpcCommand::ListDevices).await {
                        println!("Headless: ListDevices -> {:?}", resp);
                    }
                    // Subscribe (just to get an id)
                    if let Ok(resp) = ic.send_command(IpcCommand::Subscribe).await {
                        println!("Headless: Subscribe -> {:?}", resp);
                    }
                    // Start capture
                    let _ = ic.send_command(IpcCommand::StartCapture).await;
                    println!("Headless: StartCapture sent");
                    // wait a bit to generate telemetry
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    // Stop capture
                    let _ = ic.send_command(IpcCommand::StopCapture).await;
                    println!("Headless: StopCapture sent");
                    // Unsubscribe
                    let _ = ic
                        .send_command(IpcCommand::Unsubscribe { subscriber_id: 1 })
                        .await;
                    println!("Headless: Unsubscribe sent");
                    // Get status
                    if let Ok(resp) = ic.send_command(IpcCommand::GetStatus).await {
                        println!("Headless: GetStatus -> {:?}", resp);
                    }
                });

                // give the headless script a short time to run
                tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                return;
            }

            let options = eframe::NativeOptions::default();
            let _ = eframe::run_native(
                "Iris",
                options,
                Box::new(move |cc| {
                    // Apply a charcoal/dark visuals and white text on creation
                    let ctx = &cc.egui_ctx;
                    let mut visuals = egui::Visuals::dark();
                    // Charcoal background
                    visuals.window_fill = egui::Color32::from_rgb(45, 48, 52);
                    visuals.panel_fill = egui::Color32::from_rgb(45, 48, 52);
                    visuals.override_text_color = Some(egui::Color32::WHITE);
                    ctx.set_visuals(visuals);

                    Box::new(iris_ui::ui_app::IrisApp::new(ipc.clone(), capture_handle))
                }),
            );
        }
        Err(e) => {
            eprintln!("Bootstrap failed: {}", e);
        }
    }
}
