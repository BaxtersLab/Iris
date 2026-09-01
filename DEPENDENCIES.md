Dependencies and native build notes

## Corrected 2026-09-01 — read this before the Windows section below

**Iris does not link FFmpeg.** The opening line of this file said it did, via
`ffmpeg-sys-next`, and named `bsr-ui` — a crate from a *different* project that
has never been part of this workspace. Neither `ffmpeg-sys-next` nor any FFmpeg
binding appears in `Cargo.lock`.

FFmpeg is a **test-time** dependency on Linux only, and only as the `ffmpeg`
**CLI**, which `iris-harness` shells out to. That is documented correctly at the
bottom of this file and in `README.md`. The Windows vcpkg instructions below are
retained because they may still be useful for a `LIBCLANG_PATH`/MSVC setup, but
**they are not required to build Iris**.

## Rust dependencies worth knowing about

All pure Rust — no system libraries, no bindgen — which is deliberate:

| crate | used by | why |
|---|---|---|
| `zune-jpeg` | `iris-capture` | decodes MJPEG frames from UVC cameras. **Reports success on a truncated JPEG**, so `decode_to_rgb24` checks for an EOI marker itself |
| `png` | `iris-ui` | decodes the embedded window icon at startup. The alternative was committing raw RGBA, which nobody can open or review |
| `libc` | `iris-hal`, `iris-ui` | V4L2 ioctls, and `flock` for the single-instance guard |
| `eframe` / `egui` | `iris-ui` | the window. Renders through EGL/OpenGL, **not Vulkan** |

Runtime shared libraries for the `.deb` are derived from a running process
rather than from `ldd` — see `packaging/DEBIAN/control`.

---

Below: the original Windows notes, retained as written.


Option A — vcpkg (recommended on Windows)

1) Clone and bootstrap vcpkg (run as normal user):

```powershell
# one-time
git clone https://github.com/microsoft/vcpkg.git C:\tools\vcpkg
C:\tools\vcpkg\bootstrap-vcpkg.bat
```

2) Install FFmpeg and required libs (x64):

```powershell
C:\tools\vcpkg\vcpkg.exe install ffmpeg:x64-windows
```

3) Make vcpkg available to builds (set for future shells):

```powershell
setx VCPKG_ROOT C:\tools\vcpkg
# then restart your shell or set for current session:
$env:VCPKG_ROOT = 'C:\tools\vcpkg'
```

Notes:
- vcpkg will build some dependencies (CMake, build tools) if not present. On Windows you should have the MSVC toolchain (Visual Studio Build Tools) installed.
- If vcpkg fails because of missing tools (CMake, etc.), install them using Visual Studio Installer or package managers like Chocolatey.

Option B — pkg-config + system FFmpeg (MSYS2 / pkg-config)

If you have FFmpeg installed via MSYS2 or another distribution that provides pkg-config `.pc` files, point `PKG_CONFIG_PATH` at the directory with `libavutil.pc`/`libavcodec.pc`.

```powershell
# Example: if FFmpeg pkgconfig files are in C:\msys64\mingw64\lib\pkgconfig
$env:PKG_CONFIG_PATH = 'C:\msys64\mingw64\lib\pkgconfig'
cargo build
```

Also ensure `pkg-config` is installed and on your PATH (MSYS2 or Chocolatey packages provide it).

What I ran (automated attempt)

I started a background install that clones vcpkg and attempts to bootstrap and install `ffmpeg:x64-windows`. The vcpkg install reported a missing/insufficient `cmake` version (vcpkg requires a suitable CMake for building ffmpeg). To continue you can:

- Install CMake 3.31.10 or newer (recommended: install Visual Studio Build Tools + CMake), or
- Install Visual Studio (Community) with the Desktop development workload, which includes needed tools, or
- Install CMake via Chocolatey / MSYS2:

```powershell
choco install cmake --installargs 'ADD_CMAKE_TO_PATH=System'
# or via MSYS2 pacman inside msys2 shell:
# pacman -S mingw-w64-x86_64-cmake
```

After ensuring CMake and MSVC build tools are available, re-run the vcpkg bootstrap and install commands (or rerun the script I started).

Alternative: If you prefer, I can proceed to install CMake and continue the vcpkg + ffmpeg install for you (requires admin or developer tools). Ask me to proceed and I'll continue.

Why this matters

- The `ffmpeg-sys-next` crate links to native FFmpeg development libraries (headers and .lib/.dll). Having `vcpkg` or proper pkg-config/system libraries is required so `cargo build` can succeed for UI crates that depend on FFmpeg.

Contact me with your preference (I can install CMake and continue, or you can install it and I'll finish the vcpkg step).
---

## Linux — test-time dependency: the `ffmpeg` CLI

**Distinct from everything above.** The sections above cover the FFmpeg
*development libraries* that `ffmpeg-sys-next` links against at build time. This
one is about the **`ffmpeg` command-line binary**, which `iris-harness` shells
out to at **test** time. Neither implies the other: you can build the workspace
without the CLI, and having the CLI does not satisfy the link-time requirement.

```bash
sudo apt-get install -y ffmpeg     # Ubuntu 26.04: 3 packages, 0 removed
```

Pulls `ffmpeg`, `libsdl2-2.0-0`, `libsdl2-classic`. On a box that already has a
desktop stack this adds **no new X11 or Wayland libraries** — SDL2's client-side
deps are already present — and installs **no X server component**. SDL2 is only
reachable via `ffplay`, which Iris never invokes; the harness runs ffmpeg
headless to a file with stdin/stdout/stderr all null, so it touches no display
server. Safe on a Wayland-only, single-seat box.

**Why it is required rather than optional.** `iris-harness`'s
`ffmpeg_produces_a_real_mpegts_stream` asserts `Ok` outright. Without ffmpeg on
`PATH` it **fails** — verified 2026-08-16 by running the test binary under
`env -i PATH=/nonexistent`:

```
test tests::ffmpeg_produces_a_real_mpegts_stream ... FAILED
  panicked: ffmpeg must be installed to run this suite — see DEPENDENCIES.md:
            "ffmpeg spawn failed: No such file or directory (os error 2)"
```

That failure is deliberate. The predecessor test tolerated both branches and so
passed whether or not ffmpeg existed — a green result proving nothing. If you
are deploying somewhere ffmpeg genuinely cannot be installed, mark the test
`#[ignore]` so the count stops flattering; do not restore the tolerant version.
