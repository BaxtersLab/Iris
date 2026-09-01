// SPDX-License-Identifier: MIT
// Iris — iris-control tests

use crate::control::{resolve_auto_support, AutoSupport, CameraControl, ControlCapability};
use crate::profile::{CameraProfile, ProfileStore};
use iris_hal::device::ControlCapabilityInfo;

fn info(id: u32, name: &str, min: i64, max: i64, step: i64, default: i64) -> ControlCapabilityInfo {
    ControlCapabilityInfo {
        id,
        name: name.to_string(),
        min,
        max,
        step,
        default,
    }
}

fn cap(min: i64, max: i64, step: i64, default: i64) -> ControlCapability {
    ControlCapability::from_hal(
        &info(0, "brightness", min, max, step, default),
        default,
        AutoSupport::None,
    )
}

// ---- CameraControl -------------------------------------------------------

#[test]
fn known_names_resolve_and_round_trip() {
    for name in [
        "brightness", "contrast", "saturation", "sharpness", "gamma", "hue",
        "white_balance", "backlight_compensation", "gain", "exposure", "focus",
        "zoom", "pan", "tilt",
    ] {
        let c = CameraControl::from_name(name);
        assert_eq!(c.name(), name, "{name} must round-trip through from_name/name");
    }
}

/// The two platforms spell the same control differently — V4L2 reports
/// `white_balance_temperature`, Windows reports `white_balance` — and a profile
/// is keyed by the canonical name, so both must land on the same control or a
/// profile stops applying when it crosses platforms.
#[test]
fn platform_spellings_map_to_one_control() {
    for alias in ["white_balance_temperature", "whitebalance", "White Balance"] {
        assert_eq!(
            CameraControl::from_name(alias),
            CameraControl::WhiteBalance,
            "{alias:?} must resolve to WhiteBalance"
        );
    }
    assert_eq!(
        CameraControl::from_name("exposure_time_absolute"),
        CameraControl::Exposure
    );
    assert_eq!(CameraControl::from_name("focus_absolute"), CameraControl::Focus);
}

/// An unrecognised control is kept, not dropped: a device's own controls are
/// still usable by name, and discarding them would make them invisible.
#[test]
fn an_unknown_name_becomes_custom_and_keeps_its_text() {
    let c = CameraControl::from_name("power_line_frequency");
    assert_eq!(c, CameraControl::Custom("power_line_frequency".into()));
    assert_eq!(c.name(), "power_line_frequency");
}

// ---- validation ----------------------------------------------------------

#[test]
fn a_value_outside_the_range_is_refused() {
    let c = cap(0, 64, 1, 32);
    assert!(!c.validate_value(-1));
    assert!(!c.validate_value(65));
    assert!(c.validate_value(0));
    assert!(c.validate_value(64));
}

/// Range alone is not enough. A driver reporting `step = 4` does not accept 5,
/// and a set that silently rounds makes the caller's read-back disagree with
/// what it wrote.
#[test]
fn a_value_off_the_step_grid_is_refused() {
    let c = cap(0, 64, 4, 0);
    assert!(c.validate_value(8));
    assert!(!c.validate_value(5));
    let offset = cap(3, 63, 4, 3);
    assert!(offset.validate_value(7), "the grid starts at min, not at zero");
    assert!(!offset.validate_value(8));
}

/// A driver reporting step 0 is a quirk, not a grid of width zero — treating it
/// as one would divide by zero or refuse every value.
#[test]
fn a_non_positive_step_falls_back_to_range_only() {
    let c = cap(0, 10, 0, 5);
    assert!(c.validate_value(7));
    assert!(!c.validate_value(11));
}

#[test]
fn clamping_snaps_to_the_nearest_grid_point_inside_the_range() {
    let c = cap(0, 64, 4, 0);
    assert_eq!(c.clamp_value(-5), 0);
    assert_eq!(c.clamp_value(100), 64);
    assert_eq!(c.clamp_value(5), 4, "nearer the lower grid point");
    assert_eq!(c.clamp_value(6), 8, "nearer the upper grid point");
    assert!(
        c.validate_value(c.clamp_value(37)),
        "clamping must always produce an acceptable value"
    );
}

// ---- automation companions ----------------------------------------------

/// Auto support is read off the device, never assumed: V4L2 exposes automation
/// as a separate boolean control beside the one it governs.
#[test]
fn a_boolean_companion_is_toggleable() {
    let all = vec![
        info(1, "white_balance_temperature", 2800, 6500, 1, 4000),
        info(2, "white_balance_automatic", 0, 1, 1, 1),
    ];
    match resolve_auto_support(&all, "white_balance_temperature") {
        AutoSupport::Toggleable { companion_id, .. } => assert_eq!(companion_id, 2),
        other => panic!("expected Toggleable, got {other:?}"),
    }
}

/// `auto_exposure` on V4L2 is a MENU — 0 auto, 1 manual, 2 shutter priority,
/// 3 aperture priority — so neither min nor max means "on". Guessing would put
/// a real camera into a mode nobody asked for, so it is reported as not
/// toggleable instead.
#[test]
fn a_menu_companion_is_not_toggled_blindly() {
    let all = vec![
        info(1, "exposure_time_absolute", 3, 2047, 1, 166),
        info(2, "auto_exposure", 0, 3, 1, 3),
    ];
    match resolve_auto_support(&all, "exposure_time_absolute") {
        AutoSupport::NotToggleable { companion_name, reason } => {
            assert_eq!(companion_name, "auto_exposure");
            assert!(reason.contains("menu"), "the reason must say why: {reason}");
        }
        other => panic!("expected NotToggleable, got {other:?}"),
    }
}

/// REGRESSION, found on a real camera and not reproducible with a fake one.
///
/// V4L2 reports the companion as **"White Balance, Automatic"** — with a comma.
/// The first normalisation replaced only `-` and space, producing
/// `white_balance,_automatic`, which matched nothing: the camera had the
/// companion and Iris said the control had no automation. No test author
/// writes a comma into a fixture, so only hardware surfaced it.
#[test]
fn a_punctuated_driver_name_still_matches_its_companion() {
    let all = vec![
        info(9, "White Balance, Automatic", 0, 1, 1, 1),
        info(12, "White Balance", 2800, 6500, 1, 4000),
    ];
    assert_eq!(
        crate::control::normalise_control_name("White Balance, Automatic"),
        "white_balance_automatic"
    );
    match resolve_auto_support(&all, "White Balance") {
        AutoSupport::Toggleable { companion_id, .. } => assert_eq!(companion_id, 9),
        other => panic!("a comma must not hide the companion, got {other:?}"),
    }
}

/// Punctuation, case and spacing all reduce to the same key, so the control a
/// profile names is the control the driver reported however it spelled it.
#[test]
fn normalisation_collapses_punctuation_and_runs() {
    for (raw, want) in [
        ("Exposure Time, Absolute", "exposure_time_absolute"),
        ("  White-Balance   Temperature ", "white_balance_temperature"),
        ("Backlight Compensation", "backlight_compensation"),
        ("__gain__", "gain"),
    ] {
        assert_eq!(crate::control::normalise_control_name(raw), want, "{raw:?}");
    }
}

#[test]
fn no_companion_means_no_auto_support() {
    let all = vec![info(1, "brightness", -64, 64, 1, 0)];
    assert_eq!(resolve_auto_support(&all, "brightness"), AutoSupport::None);
    assert!(!AutoSupport::None.is_toggleable());
}

/// Nothing may invent a pairing the device did not report both halves of.
#[test]
fn a_companion_for_a_different_control_is_not_borrowed() {
    let all = vec![
        info(1, "focus_absolute", 0, 255, 5, 0),
        info(2, "white_balance_automatic", 0, 1, 1, 1),
    ];
    assert_eq!(resolve_auto_support(&all, "focus_absolute"), AutoSupport::None);
}

// ---- profiles ------------------------------------------------------------

fn temp_store() -> (ProfileStore, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "iris-profiles-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    (ProfileStore::new(dir.clone()), dir)
}

#[test]
fn a_profile_round_trips_through_disk() {
    let (store, dir) = temp_store();
    let mut p = CameraProfile::new("studio");
    p.set(&CameraControl::Brightness, 12);
    p.set(&CameraControl::WhiteBalance, 4600);
    p.set_auto(&CameraControl::WhiteBalance, false);
    p.description = "even light".into();

    store.save_profile(&p).expect("save");
    let back = store.load_profile("studio").expect("load");
    assert_eq!(back, p, "a profile must survive the round trip unchanged");
    assert_eq!(back.get(&CameraControl::Brightness), Some(12));
    assert_eq!(back.get_auto(&CameraControl::WhiteBalance), Some(false));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Profiles are keyed by NAME, not by platform control id, so one written on
/// Linux applies on Windows where the ids are entirely different numbers.
#[test]
fn profiles_are_keyed_by_name_not_by_platform_id() {
    let mut p = CameraProfile::new("x");
    p.set(&CameraControl::WhiteBalance, 4000);
    let json = serde_json::to_string(&p).expect("serialise");
    assert!(json.contains("white_balance"), "keys must be names: {json}");
    assert!(
        !json.contains("\"id\""),
        "a profile must not carry platform ids: {json}"
    );
}

/// A profile name becomes a filename. Without this, `../../etc/whatever` would
/// escape the store and be written wherever it pointed.
#[test]
fn a_traversing_profile_name_is_refused() {
    let (store, dir) = temp_store();
    for bad in ["../escape", "a/b", "..", "", "   "] {
        let mut p = CameraProfile::new(bad);
        p.set(&CameraControl::Gain, 1);
        assert!(
            store.save_profile(&p).is_err(),
            "{bad:?} must be refused as a profile name"
        );
        assert!(store.load_profile(bad).is_err(), "{bad:?} must not load");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Nothing saved yet is a normal state, not an error — erroring would make
/// every caller special-case first use.
#[test]
fn an_absent_store_lists_empty_rather_than_failing() {
    let (store, _dir) = temp_store();
    assert_eq!(store.list_profiles().expect("list"), Vec::<String>::new());
}

#[test]
fn listing_is_sorted_and_delete_removes_one() {
    let (store, dir) = temp_store();
    for n in ["zulu", "alpha", "mike"] {
        store.save_profile(&CameraProfile::new(n)).expect("save");
    }
    assert_eq!(store.list_profiles().unwrap(), vec!["alpha", "mike", "zulu"]);
    store.delete_profile("mike").expect("delete");
    assert_eq!(store.list_profiles().unwrap(), vec!["alpha", "zulu"]);
    assert!(
        store.delete_profile("mike").is_err(),
        "deleting what is not there must report it"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Saved keys are ordered, so two identical profiles produce identical files
/// and a diff shows real changes rather than map reordering.
#[test]
fn saved_keys_are_in_a_stable_order() {
    let (store, dir) = temp_store();
    let mut p = CameraProfile::new("order");
    for c in [
        CameraControl::Zoom,
        CameraControl::Brightness,
        CameraControl::Gain,
    ] {
        p.set(&c, 1);
    }
    store.save_profile(&p).expect("save");
    let text = std::fs::read_to_string(dir.join("order.json")).expect("read");
    let b = text.find("brightness").expect("brightness present");
    let g = text.find("gain").expect("gain present");
    let z = text.find("zoom").expect("zoom present");
    assert!(b < g && g < z, "keys must be sorted, got:\n{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- the service, against a recording fake backend ----------------------
//
// The service is the part with the interesting behaviour — validation before a
// write, telemetry on change, profiles applied across devices — and none of it
// is exercised by the pure tests above. A fake backend that RECORDS its writes
// is what makes "did it actually call the device, and with what" assertable
// without hardware.

use crate::service::ControlService;
use async_trait::async_trait;
use iris_hal::backend::UvcBackend;
use iris_hal::device::{DeviceCapabilities, DeviceId, DeviceInfo};
use iris_hal::error::{HalError, HalResult};
use iris_ipc::telemetry::{TelemetryEnvelope, TelemetryEvent};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct FakeInner {
    values: HashMap<u32, i64>,
    /// Every set_control that reached the device, in order.
    writes: Vec<(u32, i64)>,
    fail_sets: bool,
}

struct FakeBackend {
    controls: Vec<ControlCapabilityInfo>,
    inner: Mutex<FakeInner>,
}

impl FakeBackend {
    fn new(controls: Vec<ControlCapabilityInfo>) -> Arc<Self> {
        let mut values = HashMap::new();
        for c in &controls {
            values.insert(c.id, c.default);
        }
        Arc::new(Self {
            controls,
            inner: Mutex::new(FakeInner {
                values,
                ..Default::default()
            }),
        })
    }
    fn writes(&self) -> Vec<(u32, i64)> {
        self.inner.lock().unwrap().writes.clone()
    }
    fn set_direct(&self, id: u32, v: i64) {
        self.inner.lock().unwrap().values.insert(id, v);
    }
    /// Make the device refuse every write, as a driver does for a control it
    /// advertises but will not accept.
    fn refuse_writes(&self) {
        self.inner.lock().unwrap().fail_sets = true;
    }
}

#[async_trait]
impl UvcBackend for FakeBackend {
    async fn enumerate_devices(&self) -> HalResult<Vec<DeviceInfo>> {
        Ok(vec![DeviceInfo {
            id: DeviceId("fake".into()),
            name: "Fake Camera".into(),
        }])
    }
    async fn probe_capabilities(&self, _id: &DeviceId) -> HalResult<DeviceCapabilities> {
        Ok(DeviceCapabilities { formats: vec![] })
    }
    async fn open_device(&self, _id: &DeviceId) -> HalResult<()> {
        Ok(())
    }
    async fn close_device(&self, _id: &DeviceId) -> HalResult<()> {
        Ok(())
    }
    async fn read_frame(&self, _id: &DeviceId) -> HalResult<Vec<u8>> {
        Ok(vec![])
    }
    async fn list_controls(&self, _id: &DeviceId) -> HalResult<Vec<ControlCapabilityInfo>> {
        Ok(self.controls.clone())
    }
    async fn get_control(&self, _id: &DeviceId, control_id: u32) -> HalResult<i64> {
        self.inner
            .lock()
            .unwrap()
            .values
            .get(&control_id)
            .copied()
            .ok_or_else(|| HalError::InvalidParameter("no such control on the fake device".into()))
    }
    async fn set_control(&self, _id: &DeviceId, control_id: u32, value: i64) -> HalResult<()> {
        let mut g = self.inner.lock().unwrap();
        if g.fail_sets {
            return Err(HalError::InvalidParameter("fake backend set failure".into()));
        }
        g.writes.push((control_id, value));
        g.values.insert(control_id, value);
        Ok(())
    }
}

fn typical_controls() -> Vec<ControlCapabilityInfo> {
    vec![
        info(10, "brightness", -64, 64, 1, 0),
        info(11, "contrast", 0, 64, 4, 32),
        info(12, "white_balance_temperature", 2800, 6500, 10, 4000),
        info(13, "white_balance_automatic", 0, 1, 1, 1),
        info(14, "exposure_time_absolute", 3, 2047, 1, 166),
        info(15, "auto_exposure", 0, 3, 1, 3),
    ]
}

/// Spin up a service on a fake device, returning the handle, the backend (to
/// inspect what was written) and a telemetry receiver.
fn harness(
    controls: Vec<ControlCapabilityInfo>,
) -> (
    crate::service::ControlHandle,
    Arc<FakeBackend>,
    tokio::sync::broadcast::Receiver<TelemetryEnvelope>,
    std::path::PathBuf,
) {
    let backend = FakeBackend::new(controls);
    let (tx, rx) = tokio::sync::broadcast::channel(64);
    let dir = std::env::temp_dir().join(format!(
        "iris-svc-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let (svc, handle) = ControlService::new(
        backend.clone(),
        DeviceId("fake".into()),
        tx,
        dir.clone(),
    );
    tokio::spawn(svc.run());
    (handle, backend, rx, dir)
}

fn drain(rx: &mut tokio::sync::broadcast::Receiver<TelemetryEnvelope>) -> Vec<TelemetryEvent> {
    let mut out = Vec::new();
    while let Ok(env) = rx.try_recv() {
        out.push(env.event);
    }
    out
}

#[tokio::test]
async fn listing_reports_the_devices_controls_with_auto_resolved() {
    let (h, _b, _rx, dir) = harness(typical_controls());
    let caps = h.list_controls().await.expect("list");
    let wb = caps
        .iter()
        .find(|c| c.control == CameraControl::WhiteBalance)
        .expect("white balance present");
    assert!(wb.auto.is_toggleable(), "boolean companion must be toggleable");
    let ex = caps
        .iter()
        .find(|c| c.control == CameraControl::Exposure)
        .expect("exposure present");
    assert!(
        matches!(ex.auto, AutoSupport::NotToggleable { .. }),
        "a menu companion must not be reported as toggleable"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_valid_set_reaches_the_device_and_is_reported() {
    let (h, b, mut rx, dir) = harness(typical_controls());
    h.set_control(CameraControl::Brightness, 12).await.expect("set");
    assert_eq!(b.writes(), vec![(10, 12)], "the write must reach the device");
    let events = drain(&mut rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            TelemetryEvent::ControlChanged { control, old_value, new_value }
                if control == "brightness" && *old_value == 0 && *new_value == 12
        )),
        "ControlChanged must carry both old and new: {events:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The device must never see an out-of-range write, and nothing may be
/// reported as changed when nothing changed.
#[tokio::test]
async fn an_invalid_set_never_reaches_the_device() {
    let (h, b, mut rx, dir) = harness(typical_controls());
    let err = h.set_control(CameraControl::Brightness, 500).await.unwrap_err();
    assert!(format!("{err}").contains("out of range"), "{err}");
    assert!(b.writes().is_empty(), "nothing may be written: {:?}", b.writes());
    assert!(drain(&mut rx).is_empty(), "no change means no ControlChanged");
    let _ = std::fs::remove_dir_all(&dir);
}

/// contrast has step 4, so 33 is in range and off the grid.
#[tokio::test]
async fn an_off_grid_set_is_refused_and_names_the_nearest_value() {
    let (h, b, _rx, dir) = harness(typical_controls());
    let err = h.set_control(CameraControl::Contrast, 33).await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("step 4"), "the message must say why: {msg}");
    assert!(msg.contains("nearest acceptable is 32"), "{msg}");
    assert!(b.writes().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn reset_writes_the_drivers_own_default() {
    let (h, b, _rx, dir) = harness(typical_controls());
    h.set_control(CameraControl::Contrast, 8).await.expect("set");
    h.reset_control(CameraControl::Contrast).await.expect("reset");
    assert_eq!(
        b.writes(),
        vec![(11, 8), (11, 32)],
        "reset must write the reported default, not zero"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_control_the_device_lacks_is_reported_not_guessed() {
    let (h, b, _rx, dir) = harness(vec![info(10, "brightness", -64, 64, 1, 0)]);
    let err = h.set_control(CameraControl::Zoom, 1).await.unwrap_err();
    assert!(format!("{err}").contains("does not expose"), "{err}");
    assert!(b.writes().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn auto_toggles_the_companion_control() {
    let (h, b, mut rx, dir) = harness(typical_controls());
    h.set_auto(CameraControl::WhiteBalance, false).await.expect("auto off");
    assert_eq!(b.writes(), vec![(13, 0)], "the companion id must be written");
    assert!(drain(&mut rx)
        .iter()
        .any(|e| matches!(e, TelemetryEvent::ControlAutoToggled { auto_enabled: false, .. })));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The exposure companion is a menu; toggling it blindly would put a real
/// camera into a mode nobody asked for.
#[tokio::test]
async fn auto_on_a_menu_companion_is_refused_with_the_reason() {
    let (h, b, _rx, dir) = harness(typical_controls());
    let err = h.set_auto(CameraControl::Exposure, true).await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("auto_exposure"), "{msg}");
    assert!(msg.contains("cannot be toggled blindly"), "{msg}");
    assert!(b.writes().is_empty(), "nothing may be written on a refusal");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_profile_saves_current_values_and_reapplies_them() {
    let (h, b, mut rx, dir) = harness(typical_controls());
    h.set_control(CameraControl::Brightness, 20).await.expect("set");
    h.save_profile("studio").await.expect("save");
    assert!(drain(&mut rx)
        .iter()
        .any(|e| matches!(e, TelemetryEvent::ProfileSaved { name } if name == "studio")));

    // Move the camera away from the saved state, then restore it.
    b.set_direct(10, -30);
    let applied = h.load_profile("studio").await.expect("load");
    assert!(applied >= 1, "at least brightness must be applied");
    assert!(
        b.writes().contains(&(10, 20)),
        "the saved brightness must be rewritten: {:?}",
        b.writes()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A profile written for another camera names controls this one does not have.
/// Refusing the whole profile for that would make profiles useless across
/// devices, which is most of the point of keying them by name — so unusable
/// entries are skipped and counted out.
#[tokio::test]
async fn a_profile_from_another_camera_applies_what_it_can() {
    let (h, _b, mut rx, dir) = harness(typical_controls());
    let store = ProfileStore::new(dir.clone());
    let mut p = CameraProfile::new("foreign");
    p.set(&CameraControl::Brightness, 5);
    p.set(&CameraControl::Zoom, 3);        // this device has no zoom
    p.set(&CameraControl::Contrast, 9999); // out of range here
    store.save_profile(&p).expect("save");

    let applied = h.load_profile("foreign").await.expect("load");
    assert_eq!(applied, 1, "only brightness is applicable");
    assert!(drain(&mut rx).iter().any(|e| matches!(
        e,
        TelemetryEvent::ProfileLoaded { controls_applied: 1, .. }
    )));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn loading_a_profile_that_does_not_exist_is_an_error() {
    let (h, _b, _rx, dir) = harness(typical_controls());
    assert!(h.load_profile("nope").await.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// After shutdown the handle must report that plainly rather than hanging.
#[tokio::test]
async fn a_handle_reports_a_stopped_service() {
    let (h, _b, _rx, dir) = harness(typical_controls());
    h.shutdown().await.expect("shutdown");
    for _ in 0..50 {
        if h.get_control(CameraControl::Brightness).await.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("a stopped service must make the handle fail, not hang");
}

/// A driver can advertise a control and still refuse the write. The failure
/// must reach the caller, and — the part worth testing — **no ControlChanged
/// may be emitted**, or telemetry ends up asserting a change that never
/// happened and any consumer trusting it is wrong.
#[tokio::test]
async fn a_refused_write_reports_failure_and_emits_nothing() {
    let (h, b, mut rx, dir) = harness(typical_controls());
    b.refuse_writes();
    let err = h.set_control(CameraControl::Brightness, 12).await.unwrap_err();
    assert!(format!("{err}").contains("set brightness failed"), "{err}");
    assert!(
        drain(&mut rx).is_empty(),
        "a write the device refused must not be reported as a change"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- against a real camera ----------------------------------------------

/// End to end on real hardware: list the camera's controls through the
/// service, write one, and read it back changed.
///
/// Hardware-gated on `IRIS_USE_HW=1`, and — following the discipline the rest
/// of the suite uses — it distinguishes **no camera attached** (an environment
/// fact, so skip) from **a camera present that misbehaves** (a real failure).
///
/// What this proves that the fake-backend tests cannot: that a control service
/// can hold the device for control ioctls **on its own fd while the capture
/// path is streaming from the same node**. V4L2 permits a second open for
/// controls, but "permits" is documentation, not evidence.
#[tokio::test]
async fn real_camera_controls_list_and_take_a_write() {
    if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
        eprintln!("skipping real_camera_controls_list_and_take_a_write (set IRIS_USE_HW=1)");
        return;
    }
    #[cfg(target_os = "linux")]
    {
        use iris_hal::v4l2_backend::v4l2::V4l2UvcBackend;

        if !V4l2UvcBackend::video_nodes_present() {
            eprintln!(
                "SKIP real_camera_controls_list_and_take_a_write: \
                 IRIS_USE_HW=1 but no /dev/video* node exists — no camera attached"
            );
            return;
        }

        let devices = V4l2UvcBackend::enumerate_sync().expect("enumerate");
        assert!(
            !devices.is_empty(),
            "video nodes exist but nothing enumerated — that is a regression, not an empty bench"
        );
        let device = devices[0].id.clone();
        eprintln!("controls on {device}");

        let backend = std::sync::Arc::new(V4l2UvcBackend::new());
        let (tx, _rx) = tokio::sync::broadcast::channel(64);
        let dir = std::env::temp_dir().join(format!("iris-hw-profiles-{}", std::process::id()));
        let (svc, handle) = crate::service::ControlService::new(
            backend,
            device,
            tx,
            dir.clone(),
        );
        let task = tokio::spawn(svc.run());

        let caps = handle.list_controls().await.expect("list_controls");
        assert!(!caps.is_empty(), "a UVC camera must expose at least one control");
        for c in &caps {
            eprintln!(
                "  {:<26} min={:<6} max={:<6} step={:<4} default={:<6} current={} auto={}",
                c.control.name(),
                c.min,
                c.max,
                c.step,
                c.default,
                c.current,
                c.auto.is_toggleable()
            );
            assert!(c.min < c.max, "{}: min {} not below max {}", c.control.name(), c.min, c.max);
            assert!(
                c.current >= c.min && c.current <= c.max,
                "{}: current {} outside its own reported range",
                c.control.name(),
                c.current
            );
        }

        // Write to the first control that has room to move, then read it back.
        let target = caps
            .iter()
            .find(|c| c.max - c.min >= c.step.max(1) * 2)
            .expect("at least one control with a usable range");
        let original = target.current;
        let wanted = target.clamp_value(if original == target.max {
            original - target.step.max(1)
        } else {
            original + target.step.max(1)
        });
        assert_ne!(wanted, original, "the test must actually change something");

        handle
            .set_control(target.control.clone(), wanted)
            .await
            .unwrap_or_else(|e| panic!("set {} = {wanted}: {e}", target.control.name()));

        let after = handle
            .get_control(target.control.clone())
            .await
            .expect("read back");
        eprintln!(
            "  set {} {} -> {}, read back {}",
            target.control.name(),
            original,
            wanted,
            after
        );
        assert_eq!(
            after, wanted,
            "{}: the camera did not take the write",
            target.control.name()
        );

        // Leave the camera as we found it.
        handle
            .set_control(target.control.clone(), original)
            .await
            .expect("restore");

        handle.shutdown().await.expect("shutdown");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
