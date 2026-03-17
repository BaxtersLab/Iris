Dependencies and native build notes

This project requires a few native development dependencies for some crates (for example, `bsr-ui` links to FFmpeg via `ffmpeg-sys-next`). Below are recommended installation options for Windows (PowerShell commands).

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