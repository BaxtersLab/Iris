Block H-1 — iris-ui
===================

Objective
---------
Implement the iris-ui crate: full egui/eframe viewer application with charcoal
theme, live camera preview, camera controls panel, telemetry event log, diagnostics
panel, device selector, and system tray icon. Mirrors BSR's bsr-ui (AppWindow,
charcoal theme, collapsible panels, diagnostics with timestamps, hover text) but
adds webcam-specific panels.

Prerequisites
-------------
All crate blocks (A-1 through G-1) must be complete.

Theme — Charcoal Palette
-------------------------
Exact same palette as BSR:

```rust
pub struct CharcoalTheme;

impl CharcoalTheme {
    // Background tiers
    pub const BG_DARKEST: egui::Color32 = egui::Color32::from_rgb(30, 30, 30);
    pub const BG_DARK: egui::Color32 = egui::Color32::from_rgb(40, 40, 40);
    pub const BG_MID: egui::Color32 = egui::Color32::from_rgb(50, 50, 50);
    pub const BG_LIGHT: egui::Color32 = egui::Color32::from_rgb(60, 60, 60);

    // Text
    pub const TEXT_PRIMARY: egui::Color32 = egui::Color32::from_rgb(220, 220, 220);
    pub const TEXT_SECONDARY: egui::Color32 = egui::Color32::from_rgb(160, 160, 160);
    pub const TEXT_MUTED: egui::Color32 = egui::Color32::from_rgb(100, 100, 100);

    // Accents
    pub const ACCENT_BLUE: egui::Color32 = egui::Color32::from_rgb(80, 140, 220);
    pub const ACCENT_GREEN: egui::Color32 = egui::Color32::from_rgb(80, 200, 120);
    pub const ACCENT_RED: egui::Color32 = egui::Color32::from_rgb(220, 80, 80);
    pub const ACCENT_YELLOW: egui::Color32 = egui::Color32::from_rgb(220, 180, 60);
    pub const ACCENT_ORANGE: egui::Color32 = egui::Color32::from_rgb(220, 140, 40);

    // Borders & separators
    pub const BORDER: egui::Color32 = egui::Color32::from_rgb(70, 70, 70);
    pub const SEPARATOR: egui::Color32 = egui::Color32::from_rgb(55, 55, 55);

    /// Apply the charcoal theme to an egui context.
    pub fn apply(ctx: &egui::Context) {
        let mut style = (*ctx.style()).clone();
        // Set visuals: dark mode base
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Self::BG_DARK;
        visuals.window_fill = Self::BG_MID;
        visuals.faint_bg_color = Self::BG_DARKEST;
        visuals.extreme_bg_color = Self::BG_DARKEST;
        visuals.override_text_color = Some(Self::TEXT_PRIMARY);
        visuals.widgets.inactive.bg_fill = Self::BG_LIGHT;
        visuals.widgets.hovered.bg_fill = Self::ACCENT_BLUE;
        visuals.widgets.active.bg_fill = Self::ACCENT_BLUE;
        visuals.selection.bg_fill = Self::ACCENT_BLUE;
        style.visuals = visuals;
        ctx.set_style(style);
    }
}
```

File: crates/iris-ui/lib.rs
-----------------------------
Public modules: theme, app, panels, preview.

```rust
pub mod theme;
pub mod app;
pub mod panels;
pub mod preview;
```

File: crates/iris-ui/theme.rs
-------------------------------
Contains CharcoalTheme as specified above.

File: crates/iris-ui/preview.rs
---------------------------------
Camera preview widget — renders the latest frame as an egui texture.

```rust
use egui::{ColorImage, TextureHandle, TextureOptions};
use iris_capture::frame::CaptureFrame;
use iris_hal::device::PixelFormat;

pub struct PreviewWidget {
    texture: Option<TextureHandle>,
    last_sequence: u64,
}

impl PreviewWidget {
    pub fn new() -> Self { ... }

    /// Update the preview with a new frame.
    /// Converts the raw frame data to RGBA for egui rendering.
    pub fn update_frame(&mut self, ctx: &egui::Context, frame: &CaptureFrame) {
        if frame.sequence <= self.last_sequence {
            return; // Already showing this or newer
        }
        let rgba = match frame.format {
            PixelFormat::Bgra8 => bgra_to_rgba(&frame.data),
            PixelFormat::Yuy2 => yuy2_to_rgba(&frame.data, frame.width, frame.height),
            PixelFormat::Nv12 => nv12_to_rgba(&frame.data, frame.width, frame.height),
            PixelFormat::Mjpeg => {
                // Decode JPEG to RGBA (use image crate if available, else placeholder)
                vec![128u8; (frame.width * frame.height * 4) as usize]
            }
        };
        let image = ColorImage::from_rgba_unmultiplied(
            [frame.width as usize, frame.height as usize],
            &rgba,
        );
        self.texture = Some(ctx.load_texture("preview", image, TextureOptions::LINEAR));
        self.last_sequence = frame.sequence;
    }

    /// Render the preview in the given UI area.
    pub fn show(&self, ui: &mut egui::Ui, scale: f32) {
        if let Some(ref tex) = self.texture {
            let size = tex.size_vec2() * scale;
            ui.image((tex.id(), size));
        } else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new("No camera feed")
                        .color(theme::CharcoalTheme::TEXT_MUTED)
                        .size(18.0),
                );
            });
        }
    }
}

// Pixel format conversion helpers
fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    bgra.chunks_exact(4)
        .flat_map(|px| [px[2], px[1], px[0], px[3]])
        .collect()
}

fn yuy2_to_rgba(yuy2: &[u8], width: u32, height: u32) -> Vec<u8> {
    // YUY2 to RGBA conversion (2 pixels per 4 bytes)
    // Implement standard YUV→RGB conversion
    ...
}

fn nv12_to_rgba(nv12: &[u8], width: u32, height: u32) -> Vec<u8> {
    // NV12 to RGBA conversion (Y plane + interleaved UV)
    // Implement standard YUV→RGB conversion
    ...
}
```

File: crates/iris-ui/panels.rs
-------------------------------
Collapsible side panels.

### Submodules
```
panels/
    mod.rs
    device_selector.rs
    controls_panel.rs
    telemetry_panel.rs
    diagnostics_panel.rs
    status_bar.rs
```

OR as a single file with sections — agent's choice, but must contain all panels.

### Device Selector Panel
- Dropdown listing available cameras (from iris-hal device enumeration)
- "Refresh" button to re-enumerate
- Shows selected device name, vendor, and connection status
- "Connect" / "Disconnect" button

### Controls Panel
- Sliders for each camera control (brightness, contrast, exposure, etc.)
- Each slider shows: name, current value, min/max range
- Auto checkboxes for controls that support auto mode
- "Load Profile" / "Save Profile" buttons
- Profile name text input

### Telemetry Panel
- Scrolling list of recent telemetry events (last 200)
- Each event: timestamp, event type, key fields
- Color-coded: info=TEXT_SECONDARY, warning=ACCENT_YELLOW, error=ACCENT_RED
- Pause/resume scrolling toggle
- Clear button

### Diagnostics Panel
- Real-time stats with timestamps (same pattern as BSR):
  - Capture FPS (actual vs target)
  - Frame count / drop count
  - Queue depth
  - Subscriber count
  - Ring buffer usage
  - USB bandwidth %
  - CPU / Memory usage
  - Device name and status
- Each stat shows value + timestamp of last update
- Hover text explains each metric

### Status Bar
- Bottom bar showing: capture state, device name, FPS, frame count, elapsed time
- Green dot when capturing, yellow when paused, red on error, gray when disconnected

File: crates/iris-ui/app.rs
-----------------------------
Main application window.

```rust
use eframe::App;

pub struct IrisApp {
    /// Preview widget.
    preview: PreviewWidget,
    /// Panel visibility toggles.
    show_controls: bool,
    show_telemetry: bool,
    show_diagnostics: bool,
    show_device_selector: bool,
    /// Telemetry event buffer (last 200 events).
    telemetry_log: Vec<TelemetryEnvelopeDisplay>,
    /// Diagnostics snapshot.
    diagnostics: DiagnosticsSnapshot,
    /// IPC handle for sending commands and receiving telemetry.
    // Will be wired in I-1: ipc_handle: Option<IpcHandle>,
    /// Stream handle for pulling frames.
    // Will be wired in I-1: stream_handle: Option<StreamHandle>,
}

impl IrisApp {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        CharcoalTheme::apply(&cc.egui_ctx);
        Self {
            preview: PreviewWidget::new(),
            show_controls: true,
            show_telemetry: true,
            show_diagnostics: false,
            show_device_selector: true,
            telemetry_log: Vec::new(),
            diagnostics: DiagnosticsSnapshot::default(),
        }
    }
}

impl App for IrisApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. Poll for new frames from stream handle → update preview
        // 2. Poll for telemetry events → append to telemetry_log (cap at 200)
        // 3. Update diagnostics snapshot

        // Top menu bar
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("View", |ui| {
                    ui.checkbox(&mut self.show_device_selector, "Device Selector");
                    ui.checkbox(&mut self.show_controls, "Controls");
                    ui.checkbox(&mut self.show_telemetry, "Telemetry");
                    ui.checkbox(&mut self.show_diagnostics, "Diagnostics");
                });
            });
        });

        // Status bar at bottom
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            // Render status bar
        });

        // Left side panel: device selector + controls
        if self.show_device_selector || self.show_controls {
            egui::SidePanel::left("left_panel")
                .default_width(280.0)
                .show(ctx, |ui| {
                    if self.show_device_selector {
                        // Device selector panel
                    }
                    if self.show_controls {
                        // Controls panel
                    }
                });
        }

        // Right side panel: telemetry + diagnostics
        if self.show_telemetry || self.show_diagnostics {
            egui::SidePanel::right("right_panel")
                .default_width(320.0)
                .show(ctx, |ui| {
                    if self.show_telemetry {
                        // Telemetry panel
                    }
                    if self.show_diagnostics {
                        // Diagnostics panel
                    }
                });
        }

        // Central panel: camera preview
        egui::CentralPanel::default().show(ctx, |ui| {
            self.preview.show(ui, 1.0);
        });

        // Request repaint for live preview
        ctx.request_repaint();
    }
}
```

File: crates/iris-ui/main.rs
------------------------------

```rust
use eframe::NativeOptions;

fn main() -> eframe::Result<()> {
    // Init logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Iris — 4K Vision Interface")
            .with_inner_size([1280.0, 720.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Iris",
        options,
        Box::new(|cc| Ok(Box::new(iris_ui::app::IrisApp::new(cc)))),
    )
}
```

Unit Tests
----------
File: crates/iris-ui/tests.rs

### Required Tests

1. `test_charcoal_theme_colors` — verify all color constants are valid (non-zero RGB values)
2. `test_bgra_to_rgba_conversion` — 1-pixel BGRA [B,G,R,A] → RGBA [R,G,B,A]
3. `test_preview_widget_no_frame` — PreviewWidget::new() has no texture, last_sequence=0
4. `test_telemetry_log_cap` — add 300 events to telemetry_log, verify capped at 200
5. `test_diagnostics_snapshot_default` — DiagnosticsSnapshot::default() has zeroed fields
6. `test_iris_app_creation` — IrisApp can be created (panel visibility defaults correct)

Note: Full UI rendering tests require an egui test harness. The above unit tests
cover data/logic paths. Visual testing is manual.

Acceptance Criteria
-------------------
1. `cargo check -p iris-ui` passes
2. `cargo test -p iris-ui` — all 6 tests pass
3. `cargo run -p iris-ui` launches a window with:
   - Charcoal dark theme
   - "No camera feed" placeholder in center
   - Menu bar with View toggle
   - Left panel (device selector, controls)
   - Right panel (telemetry, diagnostics)
   - Status bar at bottom
4. All panels are collapsible via View menu
5. Pixel format conversions produce correct RGBA output
6. Preview widget skips duplicate frames (sequence check)
7. Telemetry log capped at 200 entries
