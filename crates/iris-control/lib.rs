// SPDX-License-Identifier: MIT
// Iris — iris-control

//! Camera control abstraction — **not yet implemented**.
//!
//! This crate is a declared placeholder. Its specification (instruction block
//! F-1) calls for `control`, `profile` and `service` modules managing exposure,
//! gain, focus, zoom and white balance through `iris-hal`, plus named profiles
//! and a `ControlService`. None of that exists yet, and the gap is declared in
//! `ROADMAP.md`.
//!
//! **It deliberately exposes no API.** Until 2026-08-31 it exported
//!
//! ```ignore
//! pub fn apply_profile(_: &str) -> bool { true }
//! ```
//!
//! which ignored the profile it was given and reported success for work it had
//! not done — a mock in delivered product code, which Article VII forbids, and
//! the worst shape of one: a caller could not tell it had failed. Nothing ever
//! called it. An empty crate is honest; a function that always returns `true`
//! is not.
//!
//! The underlying capability does partly exist one layer down:
//! `iris_hal::backend`'s V4L2 implementation has working `get_control`,
//! `set_control` and `list_controls` (11 controls enumerated on the reference
//! camera). The WMF side does not — see `ROADMAP.md`. Whoever builds this crate
//! should route through the HAL rather than re-derive it.
