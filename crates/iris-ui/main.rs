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
    // One Iris at a time. Checked BEFORE bootstrap so a refused instance never
    // opens the camera, never binds the metrics port, and never starts a
    // capture thread it is about to throw away.
    //
    // Held for the whole run: `_instance` must stay in scope, because dropping
    // it closes the descriptor and releases the lock.
    let _instance = match iris_ui::single_instance::acquire() {
        iris_ui::single_instance::Instance::Acquired(lock) => Some(lock),
        iris_ui::single_instance::Instance::AlreadyRunning { pid, path } => {
            match pid {
                Some(pid) => eprintln!("Iris is already running (pid {pid}); not starting a second copy."),
                None => eprintln!("Iris is already running; not starting a second copy."),
            }
            eprintln!("  lock: {}", path.display());
            eprintln!("  Two instances contend for the same camera and the same metrics port.");
            // Exit 0: the user's intent — Iris running — is already satisfied,
            // and a desktop launcher must not raise an error dialog for a
            // double click on an app that is already up.
            return;
        }
        iris_ui::single_instance::Instance::Unavailable(e) => {
            // Deliberately not fatal. See single_instance's module docs: a lock
            // that cannot be evaluated is a weaker guarantee, not a reason to
            // refuse to start.
            eprintln!("single-instance guard unavailable ({e}); continuing without it");
            None
        }
    };

    match iris_ui::bootstrap::IrisRuntime::bootstrap(cfg).await {
        Ok(_rt) => {
            // Keep runtime alive while UI runs; pass the IPC handle into the UI so it can send commands and subscribe to telemetry.
            let rt = _rt; // keep ownership
            let ipc = std::sync::Arc::new(rt.ipc_handle);
            let capture_handle = rt.capture_handle;
            let control_handle = rt.control_handle;
            let mirror = rt.mirror;

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

            // Pin the Wayland app_id / X11 WM_CLASS instead of leaving it to
            // whatever winit derives. egui's own documentation for
            // `with_app_id` says it "should match the .desktop file
            // distributed with your program", and the packaged desktop entry's
            // StartupWMClass is set to the same string. Without it the shell
            // cannot match a running window to its launcher and shows a
            // generic placeholder — the defect GGUF Chatbox hit and fixed the
            // same way.
            //
            // Set rather than observed: this session could not read the app_id
            // back on a Wayland seat (there is no xprop for a Wayland surface,
            // and the app does not appear in _NET_CLIENT_LIST), so pinning it
            // is what makes the match true by construction instead of by
            // assumption.
            // Window sizing.
            //
            // A minimum, because the control strip must never be clipped: the
            // buttons are the one part of the window with no keyboard-only
            // substitute for a new user, and a window that can be dragged
            // smaller than them hides the app's only controls. 380x300 keeps
            // Start / Stop / Detect Cameras and the settings gear on one row,
            // with room left for a usable preview and the three activity lines.
            //
            // A modest default, because the layout now collapses cleanly: the
            // preview fills whatever is left, so a smaller starting window
            // shows the same things, just smaller.
            let mut viewport = egui::ViewportBuilder::default()
                .with_app_id("baxters-iris")
                .with_min_inner_size([380.0, 300.0])
                .with_inner_size([720.0, 540.0]);
            match load_window_icon() {
                Ok(icon) => viewport = viewport.with_icon(icon),
                // Not fatal. A window with the default icon is a cosmetic
                // problem; refusing to start over it is not a trade worth
                // making, and the reason is printed so it is not a mystery.
                Err(e) => eprintln!("window icon unavailable: {e}"),
            }
            let options = eframe::NativeOptions {
                viewport,
                ..Default::default()
            };
            // `run_native`'s error was discarded here. If the window could not
            // be created — no display, no GL context, a compositor refusing the
            // surface — the process carried on with the capture pipeline
            // running, no window, and not one word on stderr. It looked like a
            // hang. Report it, and exit non-zero so a launcher or a script can
            // tell that the app did not come up.
            let run = eframe::run_native(
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

                    Box::new(iris_ui::ui_app::IrisApp::new(
                        ipc.clone(),
                        capture_handle,
                        control_handle.clone(),
                        mirror.clone(),
                    ))
                }),
            );
            if let Err(e) = run {
                eprintln!("failed to open the Iris window: {e}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            // Same reasoning as the window failure above: a bootstrap that
            // cannot start is not a successful run, so do not exit 0 on it.
            eprintln!("Bootstrap failed: {}", e);
            std::process::exit(1);
        }
    }
}

/// Decode the embedded application icon into the RGBA form eframe wants.
///
/// The PNG is compiled in rather than read from disk: an installed binary and
/// a `cargo run` from the source tree have different working directories and
/// different install layouts, and an icon that silently fails to load in one of
/// them is exactly the kind of thing nobody notices until a screenshot.
fn load_window_icon() -> Result<egui::IconData, String> {
    const ICON_PNG: &[u8] = include_bytes!("assets/icon.png");

    let decoder = png::Decoder::new(ICON_PNG);
    let mut reader = decoder.read_info().map_err(|e| format!("png header: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("png data: {e}"))?;

    if info.color_type != png::ColorType::Rgba {
        return Err(format!(
            "icon must be RGBA, got {:?} — regenerate with packaging/generate_icon.py",
            info.color_type
        ));
    }
    buf.truncate(info.buffer_size());

    Ok(egui::IconData {
        rgba: buf,
        width: info.width,
        height: info.height,
    })
}
