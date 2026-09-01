use crate::error::{IrisError, IrisResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The `capture.pixel_format` values `validate` accepts.
///
/// Every name here must parse via `iris_hal::device::PixelFormat::from_config_name`,
/// which a test in `iris-hal` asserts — the two lists used to be unrelated,
/// because nothing parsed the field at all. `yuy2` is kept as the Windows
/// spelling of `yuyv`; `bgra8` was removed because no Iris backend produces it.
pub const ALLOWED_PIXEL_FORMATS: &[&str] =
    &["rgb24", "bgr24", "nv12", "yuyv", "yuy2", "mjpeg"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct IrisConfig {
    pub device: DeviceConfig,
    pub capture: CaptureConfig,
    pub controls: ControlsConfig,
    pub stream: StreamConfig,
    pub telemetry: TelemetryConfig,
    pub ui: UiConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceConfig {
    pub preferred_device: String,
    pub auto_reconnect: bool,
    pub max_reconnect_attempts: u32,
    pub reconnect_delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaptureConfig {
    pub width: u32,
    pub height: u32,
    pub target_fps: u32,
    pub pixel_format: String,
    pub max_queue_depth: usize,
    pub drop_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ControlsConfig {
    pub auto_exposure: bool,
    pub auto_focus: bool,
    pub auto_white_balance: bool,
    pub default_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamConfig {
    pub default_mode: String,
    pub ring_buffer_capacity: usize,
    pub max_subscribers: usize,
    pub ipc_pipe_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TelemetryConfig {
    pub enabled: bool,
    pub output_mode: String,
    pub file_path: String,
    pub max_events_per_second: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiConfig {
    pub show_on_start: bool,
    pub preview_scale: f32,
    pub show_telemetry_panel: bool,
    pub show_diagnostics_panel: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LoggingConfig {
    pub level: String,
    pub log_to_file: bool,
    pub log_dir: String,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            preferred_device: String::new(),
            auto_reconnect: true,
            max_reconnect_attempts: 5,
            reconnect_delay_ms: 2000,
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            width: 3840,
            height: 2160,
            target_fps: 30,
            pixel_format: "nv12".to_string(),
            max_queue_depth: 4,
            drop_policy: "oldest".to_string(),
        }
    }
}

impl Default for ControlsConfig {
    fn default() -> Self {
        Self {
            auto_exposure: true,
            auto_focus: true,
            auto_white_balance: true,
            default_profile: String::new(),
        }
    }
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            default_mode: "pull".to_string(),
            ring_buffer_capacity: 8,
            max_subscribers: 4,
            ipc_pipe_name: "\\\\.\\pipe\\iris-stream".to_string(),
        }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            output_mode: "ipc".to_string(),
            file_path: "logs/telemetry.jsonl".to_string(),
            max_events_per_second: 120,
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_on_start: true,
            preview_scale: 0.5,
            show_telemetry_panel: true,
            show_diagnostics_panel: false,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            log_to_file: true,
            log_dir: "logs".to_string(),
        }
    }
}

// `Default` is derived for `IrisConfig` above.

impl IrisConfig {
    pub fn config_path() -> IrisResult<PathBuf> {
        let exe = std::env::current_exe().map_err(IrisError::Io)?;
        let mut p = exe
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        p.push("iris.toml");
        Ok(p)
    }

    /// Every place an `iris.toml` is looked for, in priority order.
    ///
    /// The executable's own directory stays **first** so nothing that worked
    /// before this changes, but it cannot be the only one: an installed build
    /// puts the binary somewhere the user cannot write (`/usr/bin`), and a dev
    /// build hides it in `target/release/`, which is nobody's idea of where
    /// configuration lives. The XDG locations are the conventional answer on
    /// Linux and are what a packaged Iris will use.
    ///
    /// `$XDG_CONFIG_HOME` is honoured only when absolute, per the XDG basedir
    /// spec, which says a relative value must be ignored.
    pub fn config_search_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        if let Ok(p) = Self::config_path() {
            paths.push(p);
        }

        if let Some(dir) = Self::per_user_config_dir() {
            paths.push(dir.join("iris").join("iris.toml"));
        }

        paths
    }

    /// The per-user configuration directory for this platform.
    ///
    /// **Windows had none at all until 2026-08-31.** This looked only at
    /// `XDG_CONFIG_HOME` and `HOME`, and Windows sets neither — it uses
    /// `APPDATA` and `USERPROFILE` — so `config_search_paths` returned the
    /// executable's directory and nothing else, leaving an installed Windows
    /// build unconfigurable. Found by the Windows agent on round 2, as a test
    /// failure that turned out to be a real product gap rather than a test
    /// artifact.
    ///
    /// `is_absolute()` is checked only on unix. On Windows a path like
    /// `/run/user/1000` is *not* absolute (no drive letter), and applying the
    /// same filter there would silently reject values the platform considers
    /// usable.
    fn per_user_config_dir() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            // %APPDATA% is the Windows convention (roaming per-user data), with
            // %USERPROFILE%\AppData\Roaming as the fallback when it is unset.
            if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
                return Some(appdata);
            }
            return std::env::var_os("USERPROFILE")
                .map(|h| PathBuf::from(h).join("AppData").join("Roaming"));
        }
        #[cfg(not(windows))]
        {
            // XDG basedir: an absolute XDG_CONFIG_HOME, else ~/.config.
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        }
    }

    /// Load the first `iris.toml` found along [`config_search_paths`].
    ///
    /// A missing file everywhere is not an error — the built-in defaults are
    /// returned. A file that is present but unreadable or malformed **is** an
    /// error, so it is reported rather than silently skipped in favour of a
    /// lower-priority file: silently falling through would make a typo in the
    /// user's own config look like the config being ignored.
    pub fn load() -> IrisResult<Self> {
        for path in Self::config_search_paths() {
            if !path.exists() {
                continue;
            }
            let s = std::fs::read_to_string(&path).map_err(IrisError::Io)?;
            let cfg: IrisConfig = toml::from_str(&s).map_err(|e| {
                IrisError::Config(format!("toml parse ({}): {}", path.display(), e))
            })?;
            return Ok(cfg);
        }
        Ok(IrisConfig::default())
    }

    pub fn save(&self) -> IrisResult<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(IrisError::Io)?;
        }
        let s = toml::to_string_pretty(self)
            .map_err(|e| IrisError::Config(format!("toml ser: {}", e)))?;
        std::fs::write(&path, s).map_err(IrisError::Io)?;
        Ok(())
    }

    pub fn validate(&self) -> IrisResult<()> {
        if !(1..=7680).contains(&self.capture.width) {
            return Err(IrisError::Config("width out of range".to_string()));
        }
        if !(1..=4320).contains(&self.capture.height) {
            return Err(IrisError::Config("height out of range".to_string()));
        }
        if !(1..=240).contains(&self.capture.target_fps) {
            return Err(IrisError::Config("target_fps out of range".to_string()));
        }
        if !ALLOWED_PIXEL_FORMATS.contains(&self.capture.pixel_format.as_str()) {
            return Err(IrisError::Config("pixel_format invalid".to_string()));
        }
        if self.capture.max_queue_depth < 1 {
            return Err(IrisError::Config(
                "max_queue_depth must be >= 1".to_string(),
            ));
        }
        if !(self.capture.drop_policy == "oldest" || self.capture.drop_policy == "newest") {
            return Err(IrisError::Config(
                "drop_policy must be 'oldest' or 'newest'".to_string(),
            ));
        }
        if self.stream.ring_buffer_capacity < 2 {
            return Err(IrisError::Config(
                "ring_buffer_capacity must be >= 2".to_string(),
            ));
        }
        if self.stream.max_subscribers < 1 {
            return Err(IrisError::Config(
                "max_subscribers must be >= 1".to_string(),
            ));
        }
        if !(self.ui.preview_scale > 0.0 && self.ui.preview_scale <= 2.0) {
            return Err(IrisError::Config("preview_scale out of range".to_string()));
        }
        let levels = ["trace", "debug", "info", "warn", "error"];
        if !levels.contains(&self.logging.level.as_str()) {
            return Err(IrisError::Config("logging.level invalid".to_string()));
        }
        Ok(())
    }
}
