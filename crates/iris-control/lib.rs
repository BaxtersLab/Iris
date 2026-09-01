// SPDX-License-Identifier: MIT
// Iris — iris-control

//! Camera control: a portable vocabulary, saved profiles, and one owner of the
//! camera's control surface.
//!
//! Built on `iris-hal`, which implements controls on **both** platforms —
//! V4L2's `G_CTRL`/`S_CTRL`/`QUERYCTRL` on Linux, `IAMVideoProcAmp` and
//! `IAMCameraControl` on Windows. This crate adds what sits above that: names
//! instead of platform-defined ids, validation against the device's own
//! reported range and step, profiles that survive moving between platforms,
//! and a single serialising owner so two callers cannot race each other's
//! writes.
//!
//! Until 2026-08-31 this crate was six lines — `apply_profile(_) -> true`, a
//! function that ignored its argument and reported success for work it had not
//! done, while the README advertised the whole feature. That is why the
//! validation here is explicit and why nothing guesses: the failure mode this
//! code replaced was something that always claimed to work.

pub mod control;
pub mod profile;
pub mod service;

#[cfg(test)]
mod tests;

pub use control::{normalise_control_name, AutoSupport, CameraControl, ControlCapability};
pub use profile::{CameraProfile, ProfileStore};
pub use service::{ControlCommand, ControlHandle, ControlService};
