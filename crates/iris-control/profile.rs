// SPDX-License-Identifier: MIT
// Iris — iris-control

//! Named control profiles, stored as JSON on disk.

use crate::control::CameraControl;
use iris_core::error::{IrisError, IrisResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A saved set of control values.
///
/// Keyed by **name**, not by platform control id, so a profile written on Linux
/// applies on Windows. The ids differ between platforms by design; the names do
/// not.
///
/// `BTreeMap` rather than `HashMap` so the serialised form has a stable key
/// order — a profile that reorders itself on every save produces noise in any
/// diff and makes two identical profiles look different.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraProfile {
    pub name: String,
    /// control name → value
    pub values: BTreeMap<String, i64>,
    /// control name → auto enabled
    pub auto_settings: BTreeMap<String, bool>,
    #[serde(default)]
    pub description: String,
}

impl CameraProfile {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Default::default()
        }
    }

    pub fn set(&mut self, control: &CameraControl, value: i64) {
        self.values.insert(control.name(), value);
    }

    pub fn set_auto(&mut self, control: &CameraControl, enabled: bool) {
        self.auto_settings.insert(control.name(), enabled);
    }

    pub fn get(&self, control: &CameraControl) -> Option<i64> {
        self.values.get(&control.name()).copied()
    }

    pub fn get_auto(&self, control: &CameraControl) -> Option<bool> {
        self.auto_settings.get(&control.name()).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty() && self.auto_settings.is_empty()
    }
}

/// A directory of profiles, one JSON file each.
pub struct ProfileStore {
    profiles_dir: PathBuf,
}

impl ProfileStore {
    pub fn new(profiles_dir: PathBuf) -> Self {
        Self { profiles_dir }
    }

    pub fn dir(&self) -> &Path {
        &self.profiles_dir
    }

    /// Reject anything that is not a plain profile name.
    ///
    /// A profile name becomes a filename, so `../../etc/whatever` would escape
    /// the store and `save_profile` would write wherever it pointed. Checked
    /// once, here, rather than at each call site — the call sites are exactly
    /// where it would be forgotten.
    fn safe_filename(&self, name: &str) -> IrisResult<PathBuf> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(IrisError::Control("profile name is empty".into()));
        }
        let bad = trimmed.contains('/')
            || trimmed.contains('\\')
            || trimmed.contains("..")
            || trimmed.contains('\0')
            || Path::new(trimmed).components().count() != 1;
        if bad {
            return Err(IrisError::Control(format!(
                "profile name {trimmed:?} is not a plain name — \
                 path separators and traversal are refused"
            )));
        }
        Ok(self.profiles_dir.join(format!("{trimmed}.json")))
    }

    /// Every profile in the store, sorted.
    ///
    /// A missing directory is an **empty store**, not an error: nothing has
    /// been saved yet is a normal state, and erroring here would make every
    /// caller special-case first use.
    pub fn list_profiles(&self) -> IrisResult<Vec<String>> {
        if !self.profiles_dir.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&self.profiles_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    names.push(stem.to_string());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn load_profile(&self, name: &str) -> IrisResult<CameraProfile> {
        let path = self.safe_filename(name)?;
        if !path.exists() {
            return Err(IrisError::Control(format!("no profile named {name:?}")));
        }
        let text = std::fs::read_to_string(&path)?;
        let profile: CameraProfile = serde_json::from_str(&text)?;
        Ok(profile)
    }

    pub fn save_profile(&self, profile: &CameraProfile) -> IrisResult<()> {
        let path = self.safe_filename(&profile.name)?;
        std::fs::create_dir_all(&self.profiles_dir)?;
        let text = serde_json::to_string_pretty(profile)?;

        // Write to a temporary file and rename over the target. A profile
        // half-written by an interrupted save is unparseable, and it would be
        // discovered at the next load rather than at the failure.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn delete_profile(&self, name: &str) -> IrisResult<()> {
        let path = self.safe_filename(name)?;
        if !path.exists() {
            return Err(IrisError::Control(format!("no profile named {name:?}")));
        }
        std::fs::remove_file(&path)?;
        Ok(())
    }
}
