Block A-1 — Workspace Scaffolding
====================================

Objective
---------
Create the Iris Rust workspace at %USERPROFILE%\Desktop\Iris with all 8 crate
stubs, workspace Cargo.toml, README, LICENSE, and .gitignore. After this block,
`cargo check` must pass with zero errors in the workspace root.

Workspace Root
--------------
Path: %USERPROFILE%\Desktop\Iris\Cargo.toml

```toml
[workspace]
resolver = "2"
members = [
    "crates/iris-core",
    "crates/iris-ipc",
    "crates/iris-hrt",
    "crates/iris-hal",
    "crates/iris-capture",
    "crates/iris-control",
    "crates/iris-stream",
    "crates/iris-ui",
]

[profile.release]
opt-level = 3
lto = true
codegen-units = 1
panic = "abort"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
tokio = { version = "1", features = ["full"] }
toml = "0.8"
tracing = "0.1"
chrono = { version = "0.4", features = ["serde"] }
async-trait = "0.1"
```

Crate Cargo.toml Files
-----------------------
Create one Cargo.toml per crate. Each crate uses `[lib] path = "lib.rs"` (source
files live directly in the crate folder, not under src/).

### crates/iris-core/Cargo.toml
```toml
[package]
name = "iris-core"
version = "0.1.0"
edition = "2021"

[lib]
path = "lib.rs"

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
toml = { workspace = true }
tracing = { workspace = true }
```

### crates/iris-ipc/Cargo.toml
```toml
[package]
name = "iris-ipc"
version = "0.1.0"
edition = "2021"

[lib]
path = "lib.rs"

[dependencies]
iris-core = { path = "../iris-core" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
chrono = { workspace = true }
```

### crates/iris-hrt/Cargo.toml
```toml
[package]
name = "iris-hrt"
version = "0.1.0"
edition = "2021"

[lib]
path = "lib.rs"

[dependencies]
iris-core = { path = "../iris-core" }
iris-ipc = { path = "../iris-ipc" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
```

### crates/iris-hal/Cargo.toml
```toml
[package]
name = "iris-hal"
version = "0.1.0"
edition = "2021"

[lib]
path = "lib.rs"

[dependencies]
iris-core = { path = "../iris-core" }
iris-ipc = { path = "../iris-ipc" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
async-trait = { workspace = true }
thiserror = { workspace = true }

[target.'cfg(windows)'.dependencies]
windows = { version = "0.54", features = [
    "Win32_Media_MediaFoundation",
    "Win32_System_Com",
    "Win32_Devices_Usb",
] }
```

### crates/iris-capture/Cargo.toml
```toml
[package]
name = "iris-capture"
version = "0.1.0"
edition = "2021"

[lib]
path = "lib.rs"

[dependencies]
iris-core = { path = "../iris-core" }
iris-ipc = { path = "../iris-ipc" }
iris-hrt = { path = "../iris-hrt" }
iris-hal = { path = "../iris-hal" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
async-trait = { workspace = true }
thiserror = { workspace = true }
```

### crates/iris-control/Cargo.toml
```toml
[package]
name = "iris-control"
version = "0.1.0"
edition = "2021"

[lib]
path = "lib.rs"

[dependencies]
iris-core = { path = "../iris-core" }
iris-ipc = { path = "../iris-ipc" }
iris-hal = { path = "../iris-hal" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
async-trait = { workspace = true }
thiserror = { workspace = true }
```

### crates/iris-stream/Cargo.toml
```toml
[package]
name = "iris-stream"
version = "0.1.0"
edition = "2021"

[lib]
path = "lib.rs"

[dependencies]
iris-core = { path = "../iris-core" }
iris-ipc = { path = "../iris-ipc" }
iris-capture = { path = "../iris-capture" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
async-trait = { workspace = true }
thiserror = { workspace = true }
```

### crates/iris-ui/Cargo.toml
```toml
[package]
name = "iris-ui"
version = "0.1.0"
edition = "2021"

[lib]
path = "lib.rs"

[[bin]]
name = "iris-ui"
path = "main.rs"

[dependencies]
iris-core = { path = "../iris-core" }
iris-ipc = { path = "../iris-ipc" }
iris-hrt = { path = "../iris-hrt" }
iris-hal = { path = "../iris-hal" }
iris-capture = { path = "../iris-capture" }
iris-control = { path = "../iris-control" }
iris-stream = { path = "../iris-stream" }
tokio = { workspace = true, features = ["rt-multi-thread", "macros"] }
tracing = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
chrono = { workspace = true }
eframe = "0.27"
egui = "0.27"
tray-icon = "0.13"
global-hotkey = "0.4"
tracing-subscriber = "0.3"
```

Stub lib.rs Files
-----------------
Every crate gets a minimal lib.rs that compiles:

```rust
// SPDX-License-Identifier: MIT
// Iris — <crate description>
```

For iris-ui, also create a minimal main.rs:

```rust
fn main() {
    println!("Iris UI — not yet implemented");
}
```

Supporting Files
----------------

### README.md (workspace root)
```markdown
# Iris — 4K Vision Interface

A stand-alone, deterministic, telemetry-rich Rust subsystem providing high-fidelity
visual input to agentic systems. USB 4K webcam capture, zero-inference, full
outbound telemetry.

Iris is a tool, not a brain.

## Crates
- iris-core: config, app state, logging
- iris-ipc: commands, responses, telemetry, JSON envelope
- iris-hrt: health, runtime, thermal monitoring
- iris-hal: USB UVC hardware abstraction, device enumeration
- iris-capture: async frame pipeline, FPS pacing, ROI
- iris-control: camera controls, capability queries, profiles
- iris-stream: multi-subscriber output, ring buffer, IPC delivery
- iris-ui: egui viewer, charcoal theme, diagnostics

## Build
```
cargo build --release
```

## License
MIT
```

### LICENSE
Standard MIT license text with "Baxter" as copyright holder, year 2026.

### .gitignore
```
/target
Cargo.lock
*.log
*.tmp
.vscode/
backup/
dist/
installer/
```

Acceptance Criteria
-------------------
1. `cargo check` passes with zero errors from workspace root
2. All 8 crates resolve their dependencies
3. No circular dependencies
4. `cargo test` runs (even if there are zero tests yet)
5. Directory structure matches:

```
%USERPROFILE%\Desktop\Iris\
    Cargo.toml
    README.md
    LICENSE
    .gitignore
    crates/
        iris-core/
            Cargo.toml
            lib.rs
        iris-ipc/
            Cargo.toml
            lib.rs
        iris-hrt/
            Cargo.toml
            lib.rs
        iris-hal/
            Cargo.toml
            lib.rs
        iris-capture/
            Cargo.toml
            lib.rs
        iris-control/
            Cargo.toml
            lib.rs
        iris-stream/
            Cargo.toml
            lib.rs
        iris-ui/
            Cargo.toml
            lib.rs
            main.rs
```
