Reproducing Phase-6 acceptance locally
-------------------------------------
1. Ensure camera device is connected and Windows camera permission for this app is allowed.
2. Start the GUI smoke (verifies `/metrics` and rebase):

```powershell
3. Run the real-device validation harness (starts `iris-ui` with DXGI backend, attempts to bring the window forward, collects logs and `/metrics`):

```powershell
Artifacts produced locally (after running the harness):

- Packaging artifact: `ci-artifacts/phase-6/iris-ui-windows.zip`
Notes
-----
- The harness calls `.github/scripts/bring-iris-front.ps1` to attempt to restore the UI so permission prompts are visible; if the app name differs or the window title is changed, adjust the script accordingly.
- I created a local annotated tag `phase-6` and updated `CHANGELOG.md` and `docs/PHASE-6-TASKS.md` with seal notes.
**Camera Integration Handoff**

- **Status:** In-progress. UI updated to prefer physical cameras and exclude mock/virtual devices. Auto-select and auto-start behavior added when a physical device is detected. Manual `Select` button added to device list (only shown for physical cameras).

- **What I changed (key files):**
- `crates/iris-ui/ui_app.rs`: added device filtering, auto-select logic, Start/Stop behavior, single combined preview (320x180 max), removed low-framerate thumbnail generation, added auto-select-on-enumeration and attempted auto-start capture.
- `crates/iris-ui/bootstrap.rs`: clarified that the runtime defaults to mock backend unless `IRIS_BACKEND=dxgi` is set. The mock backend is still used by default for safety.
- `crates/iris-ui/win32.rs`: (already present) helpers to bring-to-front and set window size on Windows.

- **How to reproduce current behavior locally:**
1. Open a PowerShell terminal inside the workspace root: `C:\Users\Baxter\Desktop\Iris`.
2. To run with the Windows capture backend (DXGI), set the env and run the UI in foreground so you can interact:

```powershell
$env:IRIS_BACKEND='dxgi'
cargo run -p iris-ui --release
```

3. If no physical cameras appear, unplug & replug the USB camera, then press the `Refresh` button in the UI. The device list intentionally filters out mock/virtual devices and will show only physical cameras.

- **Where logs and useful outputs are:**
- UI headless log: `iris-ui-headless.log` at the workspace root (contains `ListDevices` responses and telemetry events).
- Build/run output shown in the active PowerShell terminal used to `cargo run`.

- **Known issues and difficulties encountered (what blocked progress):**
1. Default mock backend: The app runtime intentionally uses a mock capture backend by default to keep the UI safe for development and CI. This meant the UI would list only `Mock Camera` unless `IRIS_BACKEND=dxgi` was set. I added guidance and auto-select code, but the default remains mock unless environment or runtime wiring is changed.
2. Backend availability on Windows: The DXGI/WMF capture backends are behind a runtime flag (`IRIS_BACKEND`). On some developer systems the DXGI backend is not fully wired/compiled or requires additional system capabilities and drivers. I could not guarantee enumeration until the backend is available and functioning on this machine.
3. Hotplug detection and timing: USB camera hotplug detection can miss quick re-plugs; the UI currently queries devices on start and when the `Refresh` button is pressed. In some runs the device list still showed `No physical cameras found` immediately after a plug; unplug/replug + Refresh worked intermittently.
4. Permissions/overlay interference: NVIDIA overlay or other system overlays can lock the camera or change driver behavior; this can prevent DXGI/WMF from enumerating the device while overlays are active. You observed the NVIDIA overlay triggering — closing overlays or restarting the camera driver can help.
5. In-process mock IPC dispatcher: For quicker iteration I used the in-process mock dispatcher in `bootstrap.rs` which returns mock device lists unless the DXGI backend is explicitly selected. This makes reproducing physical devices easy to miss if the env variable isn't set or if tests spawn the mock runtime.

- **Immediate next actions I'd recommend:**
- Confirm DXGI backend is available and working on the target machine. If it is, run the UI with `IRIS_BACKEND=dxgi` in foreground and test `Refresh` after unplug/replug.
- Add a visible `Detect Cameras` button that triggers an explicit `ListDevices` IPC call (I can add this for you). This will help with hotplug timing issues.
- Add repeated polling (short, controlled retries) after a plug event to catch transient USB arrival timings.
- Add explicit logging in the HAL backend enumeration path to capture errors returned by the real backend (WMF/DXGI) and present them in the UI log for easier diagnostics.
- If dual-cam tests are needed later, plan to add a camera-selection UI with checkboxes or a device-selection dropdown that supports multiple selections and independent capture toggles.

- **Commands I ran and outcomes:**
- `cargo build -p iris-ui` — build succeeded.
- `cargo run -p iris-ui --release` — launches UI. If launched without `IRIS_BACKEND=dxgi` the mock backend will be enumerated (Mock Camera). When `IRIS_BACKEND=dxgi` the UI will attempt to enumerate real cameras (may still require unplug/replug and `Refresh`).

- **Where to pick up:**
1. Add `Detect Cameras` button to `crates/iris-ui/ui_app.rs` that calls `IpcCommand::ListDevices` and logs the full response in the UI log. (I can implement this quickly.)
2. Improve hotplug handling: wire `iris-hal::hotplug` events into the UI (or poll) and update the device list automatically.
3. If DXGI cannot enumerate on this machine, instrument the DXGI backend (`crates/iris-hal`) to capture and log detailed errors and HRESULTs so we can see why enumeration failed.
4. Add a small system diagnostic mode that prints the HAL backends available and their probe results (useful for remote debugging).

- **Contact/Notes:**
- If you bring another USB webcam back, plug it in and run the UI with `IRIS_BACKEND=dxgi` in foreground, press `Refresh`, and tell me the device name shown (or paste the `iris-ui-headless.log` content). I'll then add a `Detect Cameras` button and polling to make the UX more resilient.

---

If you'd like, I'll implement the `Detect Cameras` button now and add retry polling + logged backend errors next — say the word and I'll apply that change and run the UI foreground for verification.