// SPDX-License-Identifier: MIT
// Iris — iris-control

//! Camera controls: a named vocabulary, and capabilities read from hardware.

use iris_hal::device::ControlCapabilityInfo;
use serde::{Deserialize, Serialize};

/// A camera control, named rather than numbered.
///
/// **Control *ids* are platform-defined and must not be hardcoded.** Linux V4L2
/// uses `V4L2_CID_*`; Windows uses `(namespace << 16) | property` across
/// `IAMVideoProcAmp` and `IAMCameraControl`. That difference is deliberate —
/// the two sets are not in bijection, so a shared numbering would need invented
/// ids that look real and are not. This enum is the portable handle: a name,
/// resolved against whatever the device reports at runtime.
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
    /// Anything the device exposes that is not in the standard list. Kept
    /// rather than discarded: a device's own controls are still usable by name.
    Custom(String),
}

/// Reduce a driver-reported control name to a comparable key.
///
/// Drivers punctuate. V4L2 on the reference camera reports **"White Balance,
/// Automatic"** — with a comma — which an earlier version normalised to
/// `white_balance,_automatic` and therefore failed to recognise as the
/// automation companion of `white_balance`. The camera had the companion, and
/// Iris reported the control as having no automation.
///
/// Only a real camera surfaced that: a fake backend uses whatever names the
/// test author writes, and no author writes a comma.
///
/// So: lowercase, map every non-alphanumeric character to `_`, collapse runs,
/// and trim the ends.
pub fn normalise_control_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_underscore = true; // trims leading separators
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

impl CameraControl {
    /// Resolve a driver-reported name to a control.
    ///
    /// Case-insensitive, and tolerant of the spellings the two platforms use:
    /// V4L2 reports `white_balance_temperature`, Windows reports
    /// `white_balance`. Unknown names become `Custom` rather than an error —
    /// a control this list has not heard of is still a control.
    pub fn from_name(name: &str) -> Self {
        let n = normalise_control_name(name);
        match n.as_str() {
            "brightness" => Self::Brightness,
            "contrast" => Self::Contrast,
            "saturation" => Self::Saturation,
            "sharpness" => Self::Sharpness,
            "gamma" => Self::Gamma,
            "hue" => Self::Hue,
            "white_balance" | "whitebalance" | "white_balance_temperature" => Self::WhiteBalance,
            "power_line_frequency" => Self::Custom("power_line_frequency".into()),
            "backlight_compensation" | "backlightcompensation" => Self::BacklightCompensation,
            "gain" => Self::Gain,
            "exposure" | "exposure_time_absolute" => Self::Exposure,
            "focus" | "focus_absolute" => Self::Focus,
            "zoom" | "zoom_absolute" => Self::Zoom,
            "pan" | "pan_absolute" => Self::Pan,
            "tilt" | "tilt_absolute" => Self::Tilt,
            other => Self::Custom(other.to_string()),
        }
    }

    /// The canonical name, which is also the key used in saved profiles.
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

/// Whether a control can be handed back to the camera's own automation.
///
/// **Derived from the device, never assumed.** UVC drivers expose automation as
/// a *separate* control — V4L2 reports `white_balance_automatic` alongside
/// `white_balance_temperature` — so auto support exists exactly when such a
/// companion is present in what the hardware reported.
///
/// Only **boolean** companions (`min == 0 && max == 1`) are treated as
/// toggleable. `auto_exposure` on V4L2 is a *menu* (0 = auto, 1 = manual,
/// 2 = shutter priority, 3 = aperture priority), where neither "min" nor "max"
/// means "on" — writing a guess there would set a real camera to a mode nobody
/// asked for. Menu companions are reported as **not** toggleable, with the
/// reason recorded, rather than guessed at. See `ROADMAP.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoSupport {
    /// No companion control was reported for this control.
    None,
    /// A boolean companion exists and can be toggled.
    Toggleable {
        /// Platform control id of the companion — resolved at runtime.
        companion_id: u32,
        /// The companion's driver-reported name, for diagnostics.
        companion_name: String,
    },
    /// A companion exists but its semantics are not a simple on/off.
    NotToggleable {
        companion_name: String,
        reason: String,
    },
}

impl AutoSupport {
    pub fn is_toggleable(&self) -> bool {
        matches!(self, Self::Toggleable { .. })
    }
}

/// What a single control can do on the attached device, plus its current value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlCapability {
    pub control: CameraControl,
    /// Platform-defined id, as reported by the HAL. Do not persist this — it is
    /// not portable, and profiles are keyed by name for that reason.
    pub id: u32,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub default: i64,
    pub current: i64,
    pub auto: AutoSupport,
}

impl ControlCapability {
    /// Is `value` one this control will actually accept?
    ///
    /// Range **and** step: a driver reporting `min=0 max=64 step=4` does not
    /// accept 5, and a set that silently rounds is worse than a rejection
    /// because the caller's read-back then disagrees with what it wrote.
    pub fn validate_value(&self, value: i64) -> bool {
        if value < self.min || value > self.max {
            return false;
        }
        if self.step <= 0 {
            // A non-positive step is a driver quirk, not a grid. Range alone.
            return true;
        }
        (value - self.min) % self.step == 0
    }

    /// Clamp to the nearest acceptable value on the control's own grid.
    ///
    /// For callers that want "as close as possible" rather than a rejection —
    /// a slider, say. Kept separate from `validate_value` so that rounding is
    /// always the caller's explicit choice, never a silent one.
    pub fn clamp_value(&self, value: i64) -> i64 {
        let v = value.clamp(self.min, self.max);
        if self.step <= 1 {
            return v;
        }
        let offset = v - self.min;
        let snapped = self.min + (offset / self.step) * self.step;
        // Round to whichever grid point is nearer, without leaving the range.
        let up = (snapped + self.step).min(self.max);
        if (v - snapped) * 2 >= self.step && up <= self.max {
            up
        } else {
            snapped
        }
    }

    /// Build from a HAL report plus the value read back for it.
    pub fn from_hal(info: &ControlCapabilityInfo, current: i64, auto: AutoSupport) -> Self {
        Self {
            control: CameraControl::from_name(&info.name),
            id: info.id,
            min: info.min,
            max: info.max,
            step: info.step,
            default: info.default,
            current,
            auto,
        }
    }
}

/// Pair each control with its automation companion, from one HAL listing.
///
/// A companion is recognised by name: V4L2's convention is the controlled
/// quantity plus `_automatic` (`white_balance_automatic`), or `auto_` prefixed
/// (`auto_exposure`). Nothing here invents a pairing that the device did not
/// report both halves of.
pub fn resolve_auto_support(all: &[ControlCapabilityInfo], target: &str) -> AutoSupport {
    let base = CameraControl::from_name(target).name();
    let candidates: Vec<String> = match base.as_str() {
        "white_balance" => vec![
            "white_balance_automatic".into(),
            "auto_white_balance".into(),
        ],
        "exposure" => vec!["auto_exposure".into(), "exposure_auto".into()],
        "focus" => vec![
            "focus_automatic_continuous".into(),
            "focus_auto".into(),
            "auto_focus".into(),
        ],
        "gain" => vec!["gain_automatic".into(), "auto_gain".into()],
        "hue" => vec!["hue_auto".into(), "auto_hue".into()],
        _ => vec![],
    };

    for cand in candidates {
        if let Some(info) = all
            .iter()
            .find(|i| normalise_control_name(&i.name) == cand)
        {
            return if info.min == 0 && info.max == 1 {
                AutoSupport::Toggleable {
                    companion_id: info.id,
                    companion_name: info.name.clone(),
                }
            } else {
                AutoSupport::NotToggleable {
                    companion_name: info.name.clone(),
                    reason: format!(
                        "companion is a menu (min {} max {}), not a boolean; \
                         its values are mode-specific and cannot be inferred",
                        info.min, info.max
                    ),
                }
            };
        }
    }
    AutoSupport::None
}
