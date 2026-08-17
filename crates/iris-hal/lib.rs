// SPDX-License-Identifier: MIT
// Iris — iris-hal

pub mod backend;
pub mod device;
pub mod error;
pub mod hotplug;
pub mod v4l2_backend;
pub mod wmf_backend;

#[cfg(test)]
mod tests;
