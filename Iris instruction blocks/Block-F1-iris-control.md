Block F-1 — iris-control
========================

Objective
---------
Implement the iris-control crate: camera control abstraction, capability queries,
named profiles, and a ControlService. This is a NEW crate with no BSR equivalent.
It manages exposure, gain, focus, zoom, white balance, and other UVC camera
controls through iris-hal.

Prerequisites
-------------
Blocks A-1, A-2, B-1, and D-1 must be complete.

File: crates/iris-control/lib.rs
---------------------------------
Public modules: control, profile, service.

```rust
pub mod control;
pub mod profile;
pub mod service;
```

File: crates/iris-control/control.rs
--------------------------------------

```rust
use serde::{Deserialize, Serialize};

/// Enumeration of standard UVC camera controls.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CameraControl {
    Brightness,
    Contrast,
    Saturation,
    Sharpness,
    Gamma,
    Hue,
    WhiteBalance,
    BacklightCompensation,
    Gain,
    Exposure,
    Focus,
    Zoom,
    Pan,
    Tilt,
    /// For controls not in the standard list.
    Custom(String),
}

impl CameraControl {
    /// Convert from string name to CameraControl.
    pub fn from_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "brightness" => Self::Brightness,
            "contrast" => Self::Contrast,
            "saturation" => Self::Saturation,
            "sharpness" => Self::Sharpness,
            "gamma" => Self::Gamma,
            "hue" => Self::Hue,
            "white_balance" | "whitebalance" => Self::WhiteBalance,
            "backlight_compensation" | "backlightcompensation" => Self::BacklightCompensation,
            "gain" => Self::Gain,
            "exposure" => Self::Exposure,
            "focus" => Self::Focus,
            "zoom" => Self::Zoom,
            "pan" => Self::Pan,
            "tilt" => Self::Tilt,
            other => Self::Custom(other.to_string()),
        }
    }

    /// Get the string name.
    pub fn name(&self) -> String {
        match self {
            Self::Brightness => "brightness".into(),
            Self::Contrast => "contrast".into(),
            Self::Saturation => "saturation".into(),
            Self::Sharpness => "sharpness".into(),
            Self::Gamma => "gamma".into(),
            Self::Hue => "hue".into(),
            Self::WhiteBalance => "white_balance".into(),
            Self::BacklightCompensation => "backlight_compensation".into(),
            Self::Gain => "gain".into(),
            Self::Exposure => "exposure".into(),
            Self::Focus => "focus".into(),
            Self::Zoom => "zoom".into(),
            Self::Pan => "pan".into(),
            Self::Tilt => "tilt".into(),
            Self::Custom(s) => s.clone(),
        }
    }
}

/// Capability descriptor for a single camera control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlCapability {
    pub control: CameraControl,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub default: i64,
    pub current: i64,
    pub auto_supported: bool,
    pub auto_enabled: bool,
}

impl ControlCapability {
    /// Validate that a proposed value is within the control's range and step.
    pub fn validate_value(&self, value: i64) -> bool {
        value >= self.min
            && value <= self.max
            && (value - self.min) % self.step == 0
    }
}
```

File: crates/iris-control/profile.rs
--------------------------------------
Named control profiles — save/load sets of control values.

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use super::control::CameraControl;

/// A named profile storing a set of camera control values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraProfile {
    pub name: String,
    /// Map of control → value.
    pub values: HashMap<String, i64>,
    /// Map of control → auto_enabled.
    pub auto_settings: HashMap<String, bool>,
    /// Optional description.
    pub description: String,
}

impl CameraProfile {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            values: HashMap::new(),
            auto_settings: HashMap::new(),
            description: String::new(),
        }
    }

    /// Set a control value in the profile.
    pub fn set(&mut self, control: &CameraControl, value: i64) {
        self.values.insert(control.name(), value);
    }

    /// Set auto mode for a control.
    pub fn set_auto(&mut self, control: &CameraControl, enabled: bool) {
        self.auto_settings.insert(control.name(), enabled);
    }

    /// Get a control value from the profile.
    pub fn get(&self, control: &CameraControl) -> Option<i64> {
        self.values.get(&control.name()).copied()
    }

    /// Get auto mode setting.
    pub fn get_auto(&self, control: &CameraControl) -> Option<bool> {
        self.auto_settings.get(&control.name()).copied()
    }
}

/// Profile storage — manages saving/loading profiles to/from disk.
pub struct ProfileStore {
    /// Directory where profiles are stored (JSON files).
    profiles_dir: std::path::PathBuf,
}

impl ProfileStore {
    pub fn new(profiles_dir: std::path::PathBuf) -> Self { ... }

    /// List all saved profile names.
    pub fn list_profiles(&self) -> iris_core::error::IrisResult<Vec<String>> { ... }

    /// Load a profile by name.
    pub fn load_profile(&self, name: &str) -> iris_core::error::IrisResult<CameraProfile> { ... }

    /// Save a profile.
    pub fn save_profile(&self, profile: &CameraProfile) -> iris_core::error::IrisResult<()> { ... }

    /// Delete a profile by name.
    pub fn delete_profile(&self, name: &str) -> iris_core::error::IrisResult<()> { ... }
}
```

File: crates/iris-control/service.rs
--------------------------------------

```rust
use tokio::sync::{mpsc, broadcast};
use iris_hal::backend::UvcBackend;

/// Commands for the control service.
#[derive(Debug)]
pub enum ControlCommand {
    /// Get the value of a control.
    GetControl {
        control: CameraControl,
        reply: tokio::sync::oneshot::Sender<iris_core::error::IrisResult<i64>>,
    },
    /// Set a control value.
    SetControl {
        control: CameraControl,
        value: i64,
        reply: tokio::sync::oneshot::Sender<iris_core::error::IrisResult<()>>,
    },
    /// Reset a control to default.
    ResetControl {
        control: CameraControl,
        reply: tokio::sync::oneshot::Sender<iris_core::error::IrisResult<()>>,
    },
    /// Toggle auto mode for a control.
    SetAuto {
        control: CameraControl,
        enabled: bool,
        reply: tokio::sync::oneshot::Sender<iris_core::error::IrisResult<()>>,
    },
    /// List all available controls with capabilities.
    ListControls {
        reply: tokio::sync::oneshot::Sender<Vec<ControlCapability>>,
    },
    /// Load a named profile.
    LoadProfile {
        name: String,
        reply: tokio::sync::oneshot::Sender<iris_core::error::IrisResult<usize>>,
    },
    /// Save current controls as a profile.
    SaveProfile {
        name: String,
        reply: tokio::sync::oneshot::Sender<iris_core::error::IrisResult<()>>,
    },
    /// Shutdown.
    Shutdown,
}

/// The control service manages camera controls through the HAL backend.
pub struct ControlService {
    cmd_rx: mpsc::Receiver<ControlCommand>,
    telemetry_tx: broadcast::Sender<iris_ipc::telemetry::TelemetryEnvelope>,
    profile_store: ProfileStore,
    /// Cached capabilities (populated on first ListControls or startup).
    capabilities: Vec<ControlCapability>,
}

impl ControlService {
    pub fn new(
        telemetry_tx: broadcast::Sender<iris_ipc::telemetry::TelemetryEnvelope>,
        profiles_dir: std::path::PathBuf,
    ) -> (Self, ControlHandle) { ... }

    /// Run the control service loop.
    /// For each command:
    /// - GetControl: call backend.get_control(), return value
    /// - SetControl: validate value against capability, call backend.set_control(),
    ///   emit ControlChanged telemetry with old and new values
    /// - ResetControl: get default from capability, set to default
    /// - SetAuto: toggle auto mode, emit ControlAutoToggled telemetry
    /// - ListControls: return cached capabilities
    /// - LoadProfile: load from ProfileStore, apply all values, emit ProfileLoaded
    /// - SaveProfile: read current values, save to ProfileStore, emit ProfileSaved
    /// - Shutdown: exit loop
    ///
    /// NOTE: The backend reference will be provided during integration (I-1).
    /// For now, the service operates on the cached capabilities and emits telemetry.
    /// Use a trait object or generic parameter for the backend.
    pub async fn run(mut self) { ... }
}

/// Handle for interacting with the control service.
pub struct ControlHandle {
    cmd_tx: mpsc::Sender<ControlCommand>,
}

impl ControlHandle {
    pub async fn get_control(&self, control: CameraControl) -> IrisResult<i64> { ... }
    pub async fn set_control(&self, control: CameraControl, value: i64) -> IrisResult<()> { ... }
    pub async fn reset_control(&self, control: CameraControl) -> IrisResult<()> { ... }
    pub async fn set_auto(&self, control: CameraControl, enabled: bool) -> IrisResult<()> { ... }
    pub async fn list_controls(&self) -> Vec<ControlCapability> { ... }
    pub async fn load_profile(&self, name: &str) -> IrisResult<usize> { ... }
    pub async fn save_profile(&self, name: &str) -> IrisResult<()> { ... }
    pub async fn shutdown(&self) -> IrisResult<()> { ... }
}
```

Unit Tests
----------
File: crates/iris-control/tests.rs

### Required Tests

1. `test_camera_control_from_name` — "brightness" → Brightness, "unknown" → Custom("unknown")
2. `test_camera_control_roundtrip` — name() → from_name() → same variant
3. `test_control_capability_validate` — min=0, max=100, step=5: validate 50=true, 53=false, -1=false, 101=false
4. `test_camera_profile_set_get` — set brightness=50, get brightness=Some(50)
5. `test_camera_profile_auto` — set auto exposure=true, get auto exposure=Some(true)
6. `test_camera_profile_json_roundtrip` — serialize profile to JSON, deserialize, assert equal
7. `test_profile_store_save_load` — save a profile to temp dir, load it back, verify contents match
8. `test_profile_store_list` — save 3 profiles, list, verify 3 names returned
9. `test_profile_store_delete` — save a profile, delete it, verify it's gone
10. `test_control_handle_get_set` — create service with mock capabilities, get/set via handle

Acceptance Criteria
-------------------
1. `cargo check -p iris-control` passes
2. `cargo test -p iris-control` — all 10 tests pass
3. CameraControl covers all standard UVC controls plus Custom fallback
4. ControlCapability validates values against min/max/step
5. CameraProfile stores and retrieves control values and auto settings
6. ProfileStore can save/load/delete profiles as JSON files
7. ControlService handles all command types
8. Telemetry events emitted for control changes, auto toggles, profile load/save
