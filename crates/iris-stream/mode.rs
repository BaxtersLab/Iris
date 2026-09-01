// SPDX-License-Identifier: MIT
// Iris — iris-stream

//! How frames leave the stream service.

use serde::{Deserialize, Serialize};

/// Frame delivery mode.
///
/// **Two of these are implemented and two are declared.** `Pull` and `Push` are
/// real and tested. `SharedMemory` and `Ipc` are transports that do not exist
/// yet, and the service **refuses** them rather than quietly behaving like
/// `Pull` — a mode that silently does something other than what it is named is
/// exactly the undeclared stub Article VII forbids, and it is the failure this
/// crate was rewritten to remove. See `ROADMAP.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamMode {
    /// Frames land in a ring buffer; consumers read the latest at their own
    /// pace. A slow consumer misses frames rather than delaying the producer.
    Pull,
    /// Frames are pushed to each subscriber's channel. A subscriber that
    /// cannot keep up drops frames, and only its own.
    Push,
    /// **Not implemented.** A cross-process shared-memory ring.
    SharedMemory,
    /// **Not implemented.** Serialised frames over the IPC transport.
    Ipc,
}

impl StreamMode {
    /// Whether the service can actually run in this mode.
    pub fn is_implemented(&self) -> bool {
        matches!(self, Self::Pull | Self::Push)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pull => "pull",
            Self::Push => "push",
            Self::SharedMemory => "shared_memory",
            Self::Ipc => "ipc",
        }
    }
}

impl std::str::FromStr for StreamMode {
    type Err = String;

    /// Parses every named mode, **including the unimplemented ones**.
    ///
    /// Deliberate: `stream.default_mode` in `iris.toml` should fail at the
    /// service with "that mode is not implemented" rather than here with "no
    /// such mode", because those are different problems and only one of them
    /// is the user's typo.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "pull" => Ok(Self::Pull),
            "push" => Ok(Self::Push),
            "shared_memory" | "sharedmemory" | "shm" => Ok(Self::SharedMemory),
            "ipc" => Ok(Self::Ipc),
            other => Err(format!(
                "unknown stream mode {other:?} (expected pull, push, shared_memory or ipc)"
            )),
        }
    }
}

impl std::fmt::Display for StreamMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
