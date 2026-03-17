#[tokio::main]
async fn main() {
    // Initialize logging from `RUST_LOG` (so debug output reaches stdout)
    let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    if let Err(e) = iris_core::logging::init_logging(&log_level, false, ".") {
        eprintln!("failed to init logging: {}", e);
    }

    // Try to bootstrap the Iris runtime with default config
    let cfg = iris_core::config::IrisConfig::default();
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
                Box::new(move |_cc| {
                    Box::new(iris_ui::ui_app::IrisApp::new(ipc.clone(), capture_handle))
                }),
            );
        }
        Err(e) => {
            eprintln!("Bootstrap failed: {}", e);
        }
    }
}
