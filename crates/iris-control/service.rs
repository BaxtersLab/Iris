// SPDX-License-Identifier: MIT
// Iris — iris-control

//! The control service: one owner of the camera's control surface.

use crate::control::{resolve_auto_support, AutoSupport, CameraControl, ControlCapability};
use crate::profile::{CameraProfile, ProfileStore};
use iris_core::error::{IrisError, IrisResult};
use iris_hal::backend::UvcBackend;
use iris_hal::device::DeviceId;
use iris_ipc::telemetry::{TelemetryEnvelope, TelemetryEvent};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};

/// Requests to the control service.
///
/// Every variant that can fail carries its own reply channel, so a caller
/// learns the outcome rather than firing into the dark.
#[derive(Debug)]
pub enum ControlCommand {
    GetControl {
        control: CameraControl,
        reply: oneshot::Sender<IrisResult<i64>>,
    },
    SetControl {
        control: CameraControl,
        value: i64,
        reply: oneshot::Sender<IrisResult<()>>,
    },
    ResetControl {
        control: CameraControl,
        reply: oneshot::Sender<IrisResult<()>>,
    },
    SetAuto {
        control: CameraControl,
        enabled: bool,
        reply: oneshot::Sender<IrisResult<()>>,
    },
    ListControls {
        reply: oneshot::Sender<IrisResult<Vec<ControlCapability>>>,
    },
    LoadProfile {
        name: String,
        reply: oneshot::Sender<IrisResult<usize>>,
    },
    SaveProfile {
        name: String,
        reply: oneshot::Sender<IrisResult<()>>,
    },
    Shutdown,
}

/// Owns the camera's control surface and serialises access to it.
///
/// One task, one command channel. Camera controls are device-global state: two
/// callers writing exposure concurrently leave the camera in whichever order
/// the driver happened to see, and neither knows which won. Funnelling through
/// a service makes the order explicit and gives every change one place to be
/// reported from.
pub struct ControlService<B: UvcBackend> {
    backend: Arc<B>,
    device: DeviceId,
    cmd_rx: mpsc::Receiver<ControlCommand>,
    telemetry_tx: broadcast::Sender<TelemetryEnvelope>,
    profile_store: ProfileStore,
    sequence: Arc<AtomicU64>,
}

/// The caller's side of the service.
#[derive(Clone, Debug)]
pub struct ControlHandle {
    cmd_tx: mpsc::Sender<ControlCommand>,
}

impl<B: UvcBackend> ControlService<B> {
    pub fn new(
        backend: Arc<B>,
        device: DeviceId,
        telemetry_tx: broadcast::Sender<TelemetryEnvelope>,
        profiles_dir: std::path::PathBuf,
    ) -> (Self, ControlHandle) {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let svc = Self {
            backend,
            device,
            cmd_rx,
            telemetry_tx,
            profile_store: ProfileStore::new(profiles_dir),
            sequence: Arc::new(AtomicU64::new(0)),
        };
        (svc, ControlHandle { cmd_tx })
    }

    fn emit(&self, event: TelemetryEvent) {
        let envelope = TelemetryEnvelope {
            timestamp: chrono::Utc::now(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            event,
        };
        // A send failure means nobody is subscribed. That is normal — telemetry
        // is observation, and losing an observer must never fail the operation
        // being observed.
        let _ = self.telemetry_tx.send(envelope);
    }

    /// Read the device's controls and pair each with its automation companion.
    async fn capabilities(&self) -> IrisResult<Vec<ControlCapability>> {
        let infos = self
            .backend
            .list_controls(&self.device)
            .await
            .map_err(|e| IrisError::Control(format!("list_controls failed: {e}")))?;

        let mut out = Vec::with_capacity(infos.len());
        for info in &infos {
            // A control that lists but will not read is reported at its
            // default rather than dropped: the range is still true, and
            // hiding it would make the control vanish from the UI entirely.
            let current = self
                .backend
                .get_control(&self.device, info.id)
                .await
                .unwrap_or(info.default);
            let auto = resolve_auto_support(&infos, &info.name);
            out.push(ControlCapability::from_hal(info, current, auto));
        }
        Ok(out)
    }

    async fn capability_for(&self, control: &CameraControl) -> IrisResult<ControlCapability> {
        let want = control.name();
        self.capabilities()
            .await?
            .into_iter()
            .find(|c| c.control.name() == want)
            .ok_or_else(|| {
                IrisError::Control(format!("this device does not expose a {want:?} control"))
            })
    }

    async fn set_checked(&self, control: &CameraControl, value: i64) -> IrisResult<()> {
        let cap = self.capability_for(control).await?;
        if !cap.validate_value(value) {
            return Err(IrisError::Control(format!(
                "{} = {value} is out of range (min {} max {} step {}); \
                 nearest acceptable is {}",
                control.name(),
                cap.min,
                cap.max,
                cap.step,
                cap.clamp_value(value),
            )));
        }
        let old = cap.current;
        self.backend
            .set_control(&self.device, cap.id, value)
            .await
            .map_err(|e| IrisError::Control(format!("set {} failed: {e}", control.name())))?;
        self.emit(TelemetryEvent::ControlChanged {
            control: control.name(),
            old_value: old,
            new_value: value,
        });
        Ok(())
    }

    /// Run until told to stop, or until every handle is dropped.
    ///
    /// **Deliberately does not open the device.** An earlier version opened it
    /// at startup so `get_control`/`set_control` would have a handle — and on
    /// V4L2 `open_device` performs `VIDIOC_S_FMT`, which a second handle cannot
    /// do while the capture path is negotiating its own format. That produced
    /// `VIDIOC_S_FMT: Device or resource busy`, a capture backend that never
    /// started, and a permanently blank preview — intermittently, depending on
    /// which of the two opened first.
    ///
    /// The HAL now opens the node transiently for control ioctls, which need no
    /// streaming setup. So this service can share a camera with the capture
    /// path instead of competing with it for one.
    pub async fn run(mut self) {
        while let Some(cmd) = self.cmd_rx.recv().await {
            match cmd {
                ControlCommand::GetControl { control, reply } => {
                    let r = match self.capability_for(&control).await {
                        Ok(cap) => self
                            .backend
                            .get_control(&self.device, cap.id)
                            .await
                            .map_err(|e| {
                                IrisError::Control(format!("get {} failed: {e}", control.name()))
                            }),
                        Err(e) => Err(e),
                    };
                    let _ = reply.send(r);
                }
                ControlCommand::SetControl {
                    control,
                    value,
                    reply,
                } => {
                    let r = self.set_checked(&control, value).await;
                    let _ = reply.send(r);
                }
                ControlCommand::ResetControl { control, reply } => {
                    let r = match self.capability_for(&control).await {
                        Ok(cap) => self.set_checked(&control, cap.default).await,
                        Err(e) => Err(e),
                    };
                    let _ = reply.send(r);
                }
                ControlCommand::SetAuto {
                    control,
                    enabled,
                    reply,
                } => {
                    let r = self.set_auto(&control, enabled).await;
                    let _ = reply.send(r);
                }
                ControlCommand::ListControls { reply } => {
                    let _ = reply.send(self.capabilities().await);
                }
                ControlCommand::LoadProfile { name, reply } => {
                    let r = self.load_profile(&name).await;
                    let _ = reply.send(r);
                }
                ControlCommand::SaveProfile { name, reply } => {
                    let r = self.save_profile(&name).await;
                    let _ = reply.send(r);
                }
                ControlCommand::Shutdown => break,
            }
        }

    }

    async fn set_auto(&self, control: &CameraControl, enabled: bool) -> IrisResult<()> {
        let cap = self.capability_for(control).await?;
        match &cap.auto {
            AutoSupport::Toggleable { companion_id, .. } => {
                let value = i64::from(enabled);
                self.backend
                    .set_control(&self.device, *companion_id, value)
                    .await
                    .map_err(|e| {
                        IrisError::Control(format!("set auto for {} failed: {e}", control.name()))
                    })?;
                self.emit(TelemetryEvent::ControlAutoToggled {
                    control: control.name(),
                    auto_enabled: enabled,
                });
                Ok(())
            }
            AutoSupport::NotToggleable {
                companion_name,
                reason,
            } => Err(IrisError::Control(format!(
                "{} has an automation control ({companion_name}) but it cannot be \
                 toggled blindly: {reason}. Set it directly by name instead.",
                control.name()
            ))),
            AutoSupport::None => Err(IrisError::Control(format!(
                "this device exposes no automation control for {}",
                control.name()
            ))),
        }
    }

    /// Apply a saved profile, returning how many controls were actually set.
    ///
    /// A profile saved from another camera will name controls this one does not
    /// have, and values its ranges do not accept. Those are **skipped and
    /// counted out**, not treated as failure: refusing the whole profile
    /// because one control is missing makes profiles useless across devices,
    /// which is most of the point of keying them by name.
    async fn load_profile(&self, name: &str) -> IrisResult<usize> {
        let profile = self.profile_store.load_profile(name)?;
        let mut applied = 0usize;

        for (key, value) in &profile.values {
            let control = CameraControl::from_name(key);
            match self.set_checked(&control, *value).await {
                Ok(()) => applied += 1,
                Err(e) => tracing::warn!("profile {name:?}: skipping {key}: {e}"),
            }
        }
        for (key, enabled) in &profile.auto_settings {
            let control = CameraControl::from_name(key);
            match self.set_auto(&control, *enabled).await {
                Ok(()) => applied += 1,
                Err(e) => tracing::warn!("profile {name:?}: skipping auto for {key}: {e}"),
            }
        }

        self.emit(TelemetryEvent::ProfileLoaded {
            name: name.to_string(),
            controls_applied: applied,
        });
        Ok(applied)
    }

    async fn save_profile(&self, name: &str) -> IrisResult<()> {
        let caps = self.capabilities().await?;
        let mut profile = CameraProfile::new(name);
        for cap in &caps {
            profile.set(&cap.control, cap.current);
            if let AutoSupport::Toggleable { companion_id, .. } = cap.auto {
                if let Ok(v) = self.backend.get_control(&self.device, companion_id).await {
                    profile.set_auto(&cap.control, v != 0);
                }
            }
        }
        self.profile_store.save_profile(&profile)?;
        self.emit(TelemetryEvent::ProfileSaved {
            name: name.to_string(),
        });
        Ok(())
    }
}

impl ControlHandle {
    async fn request<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<IrisResult<T>>) -> ControlCommand,
    ) -> IrisResult<T> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(make(tx))
            .await
            .map_err(|_| IrisError::Control("control service is not running".into()))?;
        rx.await
            .map_err(|_| IrisError::Control("control service dropped the request".into()))?
    }

    pub async fn get_control(&self, control: CameraControl) -> IrisResult<i64> {
        self.request(|reply| ControlCommand::GetControl { control, reply })
            .await
    }

    pub async fn set_control(&self, control: CameraControl, value: i64) -> IrisResult<()> {
        self.request(|reply| ControlCommand::SetControl {
            control,
            value,
            reply,
        })
        .await
    }

    pub async fn reset_control(&self, control: CameraControl) -> IrisResult<()> {
        self.request(|reply| ControlCommand::ResetControl { control, reply })
            .await
    }

    pub async fn set_auto(&self, control: CameraControl, enabled: bool) -> IrisResult<()> {
        self.request(|reply| ControlCommand::SetAuto {
            control,
            enabled,
            reply,
        })
        .await
    }

    pub async fn list_controls(&self) -> IrisResult<Vec<ControlCapability>> {
        self.request(|reply| ControlCommand::ListControls { reply })
            .await
    }

    pub async fn load_profile(&self, name: &str) -> IrisResult<usize> {
        let name = name.to_string();
        self.request(|reply| ControlCommand::LoadProfile { name, reply })
            .await
    }

    pub async fn save_profile(&self, name: &str) -> IrisResult<()> {
        let name = name.to_string();
        self.request(|reply| ControlCommand::SaveProfile { name, reply })
            .await
    }

    pub async fn shutdown(&self) -> IrisResult<()> {
        self.cmd_tx
            .send(ControlCommand::Shutdown)
            .await
            .map_err(|_| IrisError::Control("control service is not running".into()))
    }
}
