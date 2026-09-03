use crate::device::{
    ControlCapabilityInfo, DeviceCapabilities, DeviceId, DeviceInfo, FormatDescriptor,
};
use crate::error::{HalError, HalResult};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[async_trait]
pub trait UvcBackend: Send + Sync + 'static {
    async fn enumerate_devices(&self) -> HalResult<Vec<DeviceInfo>>;
    async fn probe_capabilities(&self, id: &DeviceId) -> HalResult<DeviceCapabilities>;
    /// Open the device, asking for `requested` if the backend can.
    ///
    /// The request is **advisory**: drivers negotiate, and a backend must read
    /// back what it actually got — `current_format` stays authoritative. The
    /// parameter is on the trait rather than defaulted away on purpose. A
    /// defaulted `open_device_with` that quietly ignored the request is exactly
    /// how `capture.pixel_format` came to be honoured by the mock backend alone
    /// for months; making it part of the signature means a new backend cannot
    /// compile without deciding what to do with it.
    async fn open_device(
        &self,
        id: &DeviceId,
        requested: Option<&crate::device::FormatRequest>,
    ) -> HalResult<()>;
    async fn close_device(&self, id: &DeviceId) -> HalResult<()>;
    async fn read_frame(&self, id: &DeviceId) -> HalResult<Vec<u8>>;
    async fn list_controls(&self, id: &DeviceId) -> HalResult<Vec<ControlCapabilityInfo>>;
    async fn get_control(&self, id: &DeviceId, control_id: u32) -> HalResult<i64>;
    async fn set_control(&self, id: &DeviceId, control_id: u32, value: i64) -> HalResult<()>;
    /// The format the opened device is actually delivering (authoritative for
    /// telemetry). Default: unknown.
    async fn current_format(&self, _id: &DeviceId) -> HalResult<Option<crate::device::FormatDescriptor>> {
        Ok(None)
    }
}

#[derive(Clone)]
pub struct MockUvcBackend {
    devices: Arc<Vec<DeviceInfo>>,
    caps: Arc<HashMap<String, DeviceCapabilities>>,
    open_set: Arc<Mutex<HashSet<String>>>,
    control_map: Arc<Mutex<HashMap<(String, u32), i64>>>,
}

impl MockUvcBackend {
    pub fn new() -> Self {
        let dev = DeviceInfo {
            id: DeviceId("mock-0".into()),
            name: "Mock Camera".into(),
        };
        let caps = {
            let mut map = HashMap::new();
            map.insert(
                dev.id.0.clone(),
                DeviceCapabilities {
                    formats: vec![FormatDescriptor {
                        width: 640,
                        height: 480,
                        fps: 30,
                        pixel_format: crate::device::PixelFormat::Rgb24,
                    }],
                },
            );
            map
        };

        MockUvcBackend {
            devices: Arc::new(vec![dev]),
            caps: Arc::new(caps),
            open_set: Arc::new(Mutex::new(HashSet::new())),
            control_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for MockUvcBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl UvcBackend for MockUvcBackend {
    async fn enumerate_devices(&self) -> HalResult<Vec<DeviceInfo>> {
        Ok((*self.devices).clone())
    }

    async fn probe_capabilities(&self, id: &DeviceId) -> HalResult<DeviceCapabilities> {
        self.caps
            .get(&id.0)
            .cloned()
            .ok_or(HalError::DeviceNotFound)
    }

    async fn open_device(
        &self,
        id: &DeviceId,
        _requested: Option<&crate::device::FormatRequest>,
    ) -> HalResult<()> {
        let mut open = self.open_set.lock().unwrap();
        if open.contains(&id.0) {
            return Err(HalError::DeviceAlreadyOpen);
        }
        open.insert(id.0.clone());
        Ok(())
    }

    async fn close_device(&self, id: &DeviceId) -> HalResult<()> {
        let mut open = self.open_set.lock().unwrap();
        if !open.remove(&id.0) {
            return Err(HalError::DeviceNotOpen);
        }
        Ok(())
    }

    async fn read_frame(&self, id: &DeviceId) -> HalResult<Vec<u8>> {
        let open = self.open_set.lock().unwrap();
        if !open.contains(&id.0) {
            return Err(HalError::DeviceNotOpen);
        }
        // return a synthetic RGB frame (all zeroes)
        let caps = self.caps.get(&id.0).ok_or(HalError::DeviceNotFound)?;
        let fmt = caps
            .formats
            .first()
            .ok_or(HalError::InvalidParameter("no format".into()))?;
        let size = (fmt.width * fmt.height * 3) as usize;
        Ok(vec![0u8; size])
    }

    async fn list_controls(&self, id: &DeviceId) -> HalResult<Vec<ControlCapabilityInfo>> {
        let _caps = self.caps.get(&id.0).ok_or(HalError::DeviceNotFound)?;
        Ok(vec![ControlCapabilityInfo {
            id: 1,
            name: "Brightness".into(),
            min: 0,
            max: 255,
            step: 1,
            default: 128,
        }])
    }

    async fn get_control(&self, id: &DeviceId, control_id: u32) -> HalResult<i64> {
        let key = (id.0.clone(), control_id);
        let map = self.control_map.lock().unwrap();
        Ok(*map.get(&key).unwrap_or(&128))
    }

    async fn set_control(&self, id: &DeviceId, control_id: u32, value: i64) -> HalResult<()> {
        let _ = self.caps.get(&id.0).ok_or(HalError::DeviceNotFound)?;
        let mut map = self.control_map.lock().unwrap();
        map.insert((id.0.clone(), control_id), value);
        Ok(())
    }
}

#[cfg(windows)]
mod wmf {
    use super::*;
    use crate::device::{
        ControlCapabilityInfo, DeviceCapabilities, DeviceId, DeviceInfo, FormatDescriptor,
        PixelFormat,
    };
    use crate::error::{HalError, HalResult};
    use std::sync::Mutex as StdMutex;
    use windows::core::GUID;
    use windows::core::Interface;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::Media::DirectShow::*;
    use windows::Win32::System::Com::*;

    // MF GUID constants
    const MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE: GUID =
        GUID::from_u128(0xc60ac5fe_252a_478f_a0ef_bc8fa5f7cad3);
    const MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID: GUID =
        GUID::from_u128(0x8ac3587a_4ae7_42d8_99e0_0a6013eef90f);
    const MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME: GUID =
        GUID::from_u128(0x60d0e559_52f8_4fa2_bbce_acdb34a8ec01);
    const MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK: GUID =
        GUID::from_u128(0x58f0aad8_22bf_4f8a_bb3d_d2c4978c6e2f);

    // Media type attributes
    const MF_MT_SUBTYPE: GUID = GUID::from_u128(0xf7e34c9a_42e8_4714_b74b_cb29d72c35e5);
    const MF_MT_FRAME_SIZE: GUID = GUID::from_u128(0x1652c33d_d6b2_4012_b834_72030849a37d);
    const MF_MT_FRAME_RATE: GUID = GUID::from_u128(0xc459a2e8_3d2c_4e44_b132_fee5156c7bb0);

    // Well-known subtype GUIDs
    const MF_VIDEO_FORMAT_NV12: GUID = GUID::from_u128(0x3231564e_0000_0010_8000_00aa00389b71);
    const MF_VIDEO_FORMAT_YUY2: GUID = GUID::from_u128(0x32595559_0000_0010_8000_00aa00389b71);
    const MF_VIDEO_FORMAT_MJPG: GUID = GUID::from_u128(0x47504a4d_0000_0010_8000_00aa00389b71);
    const MF_VIDEO_FORMAT_RGB24: GUID = GUID::from_u128(0x00000014_0000_0010_8000_00aa00389b71);
    const MF_VIDEO_FORMAT_RGB32: GUID = GUID::from_u128(0x00000016_0000_0010_8000_00aa00389b71);

    const MF_SOURCE_READER_FIRST_VIDEO_STREAM: u32 = 0xFFFFFFFC;

    /// Frames per second from `MF_MT_FRAME_RATE`, **rounded**.
    ///
    /// The attribute is a packed ratio: numerator in the high 32 bits,
    /// denominator in the low 32. Unlike V4L2, whose `timeperframe` is an
    /// *interval* (seconds per frame), this one is already a rate — so the
    /// arithmetic is `num/den` rather than `den/num`, and only the rounding is
    /// shared with `fps_from_interval`.
    ///
    /// **Rounded, not truncated**, and that is the whole point of the helper.
    /// 30000/1001 is 29.97fps, which every broadcast-derived device reports;
    /// truncation gives 29, so a correctly granted 30fps request would warn on
    /// every launch that it got 29. This read used to truncate.
    pub(crate) fn fps_from_ratio(numerator: u32, denominator: u32) -> Option<u32> {
        if numerator == 0 || denominator == 0 {
            return None;
        }
        Some((numerator + denominator / 2) / denominator)
    }

    /// Pick which enumerated native mode to use, and say whether the requested
    /// **rate** was among the reasons.
    ///
    /// Pure, and separated from the COM walk on purpose: this is the whole of
    /// round 8's new decision, and **it cannot be proven on hardware that
    /// offers one frame rate.** Every native mode on the reference camera
    /// advertises 30/1, so a `choose_mode` that ignored the rate entirely would
    /// behave identically there. The tests below are the only place the
    /// preference is actually exercised.
    ///
    /// Returns `(index, rate_matched)`.
    ///
    /// * a mode-and-rate match wins outright and stops the search;
    /// * otherwise the **first** mode match is kept, but the walk continues —
    ///   stopping at the first mode match is exactly how the rate would be
    ///   silently ignored on a camera that does offer a choice;
    /// * `req.fps == 0` means "do not ask", which reduces this to round 7's
    ///   mode-only search.
    ///
    /// Rate is deliberately **not** part of `is_satisfied_by`: mode selection
    /// and rate selection are different questions, and a predicate demanding an
    /// exact rate would find nothing on a camera whose default rate differs.
    pub(crate) fn choose_mode(
        req: &crate::device::FormatRequest,
        candidates: &[(u32, FormatDescriptor)],
    ) -> Option<(u32, bool)> {
        let mut mode_only: Option<u32> = None;
        for (idx, fd) in candidates {
            if !req.is_satisfied_by(fd.width, fd.height, &fd.pixel_format) {
                continue;
            }
            if req.fps != 0 && fd.fps == req.fps {
                return Some((*idx, true));
            }
            if mode_only.is_none() {
                mode_only = Some(*idx);
            }
        }
        mode_only.map(|idx| (idx, false))
    }

    fn subtype_to_pixel_format(guid: &GUID) -> Option<PixelFormat> {
        if *guid == MF_VIDEO_FORMAT_NV12 {
            Some(PixelFormat::Nv12)
        } else if *guid == MF_VIDEO_FORMAT_YUY2 {
            Some(PixelFormat::Yuyv)
        } else if *guid == MF_VIDEO_FORMAT_RGB24 {
            Some(PixelFormat::Rgb24)
        } else if *guid == MF_VIDEO_FORMAT_RGB32 {
            Some(PixelFormat::Bgr24)
        } else if *guid == MF_VIDEO_FORMAT_MJPG {
            // Mapped as of round 7. Before that, this camera's 8 MJPG native
            // modes returned `None` here, `media_type_to_format` dropped them,
            // and 17 native modes enumerated as 9. That was cosmetic while
            // nothing could select a mode; with negotiation it is not —
            // **negotiation can only request a mode Iris can see**, so an
            // unmapped format makes half this device's modes unrequestable.
            //
            // Safe to hand onward: `snapshot.rs` decodes `Mjpeg`, the preview's
            // `frame_to_rgba` has an `Mjpeg` arm, and `expected_size` already
            // returns 0 for it meaning "not predictable", which is the right
            // contract for a compressed frame of varying length.
            Some(PixelFormat::Mjpeg)
        } else {
            None
        }
    }

    struct WmfState {
        reader: Option<IMFSourceReader>,
        /// Kept solely so camera controls can reach it: `IAMVideoProcAmp` and
        /// `IAMCameraControl` are obtained by QueryInterface on the media
        /// SOURCE, not on the reader. Before controls existed, `open_device`
        /// created the source and then dropped it.
        source: Option<IMFMediaSource>,
        device_id: Option<String>,
        current_width: u32,
        current_height: u32,
        current_format: PixelFormat,
        /// The frame rate the device actually granted, read back from
        /// `MF_MT_FRAME_RATE`. Was hardcoded to 30 in `current_format`, which
        /// reported a rate nobody had checked.
        current_fps: u32,
    }

    // SAFETY: IMFSourceReader is a COM pointer (NonNull<c_void>). We protect all
    // access through a StdMutex so only one thread touches the reader at a time.
    // MF itself is initialized with COINIT_MULTITHREADED, and SourceReader
    // created in MTA is safe to call from any MTA thread.
    unsafe impl Send for WmfState {}
    unsafe impl Sync for WmfState {}

    /// Wrapper to carry an IMFSourceReader across thread boundaries (into/out of
    /// spawn_blocking). SAFETY: MF is initialised in MTA mode, and we serialise
    /// all reader access through the StdMutex in WmfBackend.
    struct SendReader(IMFSourceReader);
    unsafe impl Send for SendReader {}

    /// Same contract as [`SendReader`], for the media source that camera
    /// controls are queried from.
    struct SendSource(IMFMediaSource);
    unsafe impl Send for SendSource {}

    pub struct WmfBackend {
        state: StdMutex<WmfState>,
    }

    impl WmfBackend {
        pub fn new() -> HalResult<Self> {
            unsafe {
                // CoInitializeEx returns HRESULT directly in windows 0.54
                CoInitializeEx(None, COINIT_MULTITHREADED)
                    .ok()
                    .map_err(|e| HalError::Io(format!("COM init failed: {e}")))?;
                MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)
                    .map_err(|e| HalError::Io(format!("MFStartup failed: {e}")))?;
            }
            Ok(WmfBackend {
                state: StdMutex::new(WmfState {
                    reader: None,
                    source: None,
                    device_id: None,
                    current_width: 0,
                    current_height: 0,
                    current_format: PixelFormat::Bgr24,
                    current_fps: 0,
                }),
            })
        }
    }

    impl Drop for WmfBackend {
        fn drop(&mut self) {
            {
                let mut st = self.state.lock().unwrap();
                st.reader = None;
                st.source = None;
            }
            unsafe {
                let _ = MFShutdown();
                CoUninitialize();
            }
        }
    }

    /// Enumerate video capture device MF activate objects.
    unsafe fn enumerate_video_devices() -> HalResult<Vec<IMFActivate>> {
        // MFCreateAttributes: out-pointer pattern in windows 0.54
        let mut attr: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attr as *mut _, 1)
            .map_err(|e| HalError::Io(format!("MFCreateAttributes: {e}")))?;
        let attr = attr.ok_or(HalError::Io("MFCreateAttributes returned None".into()))?;

        attr.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )
        .map_err(|e| HalError::Io(format!("SetGUID: {e}")))?;

        let mut count = 0u32;
        let mut devices_ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
        MFEnumDeviceSources(&attr, &mut devices_ptr, &mut count)
            .map_err(|e| HalError::Io(format!("MFEnumDeviceSources: {e}")))?;

        if devices_ptr.is_null() || count == 0 {
            return Ok(vec![]);
        }

        let mut result = Vec::new();
        for i in 0..count {
            let slot = &*devices_ptr.add(i as usize);
            if let Some(activate) = slot {
                result.push(activate.clone());
            }
        }
        CoTaskMemFree(Some(devices_ptr as *const _));
        Ok(result)
    }

    /// Read a string attribute from an IMFActivate.
    unsafe fn get_activate_string(activate: &IMFActivate, key: &GUID) -> Option<String> {
        // GetStringLength in 0.54: takes key, returns Result<u32>
        let len = activate.GetStringLength(key).ok()?;
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let mut actual = 0u32;
        if activate.GetString(key, &mut buf, Some(&mut actual)).is_ok() {
            Some(String::from_utf16_lossy(&buf[..actual as usize]))
        } else {
            None
        }
    }

    /// Extract a FormatDescriptor from an IMFMediaType.
    unsafe fn media_type_to_format(mt: &IMFMediaType) -> Option<FormatDescriptor> {
        let subtype = mt.GetGUID(&MF_MT_SUBTYPE).ok()?;
        let pixel_format = subtype_to_pixel_format(&subtype)?;

        let frame_size = mt.GetUINT64(&MF_MT_FRAME_SIZE).ok()?;
        let width = (frame_size >> 32) as u32;
        let height = (frame_size & 0xFFFFFFFF) as u32;

        let fps = mt
            .GetUINT64(&MF_MT_FRAME_RATE)
            .ok()
            .and_then(|r| fps_from_ratio((r >> 32) as u32, (r & 0xFFFFFFFF) as u32))
            .unwrap_or(30);

        Some(FormatDescriptor {
            width,
            height,
            fps,
            pixel_format,
        })
    }

    /// The Windows camera-control surface, and the `control_id` decision.
    ///
    /// **`control_id` is platform-defined, and deliberately so.** V4L2 uses
    /// kernel `V4L2_CID_*` integers; Media Foundation has no equivalent
    /// numbering, exposing controls through two unrelated DirectShow
    /// interfaces — `IAMVideoProcAmp` (image properties) and
    /// `IAMCameraControl` (lens/mechanism) — each with its own small enum
    /// starting at 0. So the two namespaces overlap numerically while meaning
    /// entirely different things.
    ///
    /// The alternative was to map Windows properties onto `V4L2_CID_*` values
    /// so one number worked on both platforms. That was rejected because:
    ///
    /// * no caller in this workspace hardcodes a control id — every consumer
    ///   reads `list_controls` first and uses the ids it returns, so a shared
    ///   numbering buys nothing today; and
    /// * the sets are not in bijection. V4L2 exposes controls Media Foundation
    ///   has no property for and vice versa, so a shared numbering would need
    ///   invented ids for the unmatched ones — numbers that look like V4L2 CIDs
    ///   but are not, which is a worse trap than two honestly separate spaces.
    ///
    /// The id is therefore `(namespace << 16) | property`, with namespace 0 =
    /// `IAMVideoProcAmp` and 1 = `IAMCameraControl`. The shift keeps the two
    /// enums from colliding, and `list_controls` is self-describing: it returns
    /// the id together with a name and real min/max/step/default read from the
    /// driver, so a caller never needs to know the encoding.
    const NS_PROC_AMP: u32 = 0;
    const NS_CAMERA_CONTROL: u32 = 1;

    /// Every property this backend can address, with the name reported to
    /// callers. Order is the order `list_controls` reports.
    const WMF_CONTROLS: &[(u32, i32, &str)] = &[
        // IAMVideoProcAmp — image processing
        (NS_PROC_AMP, VideoProcAmp_Brightness.0, "brightness"),
        (NS_PROC_AMP, VideoProcAmp_Contrast.0, "contrast"),
        (NS_PROC_AMP, VideoProcAmp_Hue.0, "hue"),
        (NS_PROC_AMP, VideoProcAmp_Saturation.0, "saturation"),
        (NS_PROC_AMP, VideoProcAmp_Sharpness.0, "sharpness"),
        (NS_PROC_AMP, VideoProcAmp_Gamma.0, "gamma"),
        (NS_PROC_AMP, VideoProcAmp_ColorEnable.0, "color_enable"),
        (NS_PROC_AMP, VideoProcAmp_WhiteBalance.0, "white_balance"),
        (NS_PROC_AMP, VideoProcAmp_BacklightCompensation.0, "backlight_compensation"),
        (NS_PROC_AMP, VideoProcAmp_Gain.0, "gain"),
        // IAMCameraControl — lens and mechanism
        (NS_CAMERA_CONTROL, CameraControl_Pan.0, "pan"),
        (NS_CAMERA_CONTROL, CameraControl_Tilt.0, "tilt"),
        (NS_CAMERA_CONTROL, CameraControl_Roll.0, "roll"),
        (NS_CAMERA_CONTROL, CameraControl_Zoom.0, "zoom"),
        (NS_CAMERA_CONTROL, CameraControl_Exposure.0, "exposure"),
        (NS_CAMERA_CONTROL, CameraControl_Iris.0, "iris"),
        (NS_CAMERA_CONTROL, CameraControl_Focus.0, "focus"),
    ];

    fn encode_control_id(ns: u32, prop: i32) -> u32 {
        (ns << 16) | (prop as u32 & 0xFFFF)
    }

    fn decode_control_id(id: u32) -> (u32, i32) {
        ((id >> 16), (id & 0xFFFF) as i32)
    }

    /// The two control interfaces, if the device offers them.
    ///
    /// A camera commonly implements one and not the other — a fixed-lens
    /// webcam has `IAMVideoProcAmp` but no pan/tilt — so a missing interface
    /// is normal and must not be an error at enumeration time.
    unsafe fn control_interfaces(
        source: &IMFMediaSource,
    ) -> (Option<IAMVideoProcAmp>, Option<IAMCameraControl>) {
        (source.cast().ok(), source.cast().ok())
    }
    #[async_trait]
    impl UvcBackend for WmfBackend {
        async fn enumerate_devices(&self) -> HalResult<Vec<DeviceInfo>> {
            tokio::task::spawn_blocking(|| unsafe {
                let activates = enumerate_video_devices()?;
                let mut devices = Vec::new();
                for (i, act) in activates.iter().enumerate() {
                    let name = get_activate_string(act, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
                        .unwrap_or_else(|| format!("Unknown Device {i}"));
                    let symlink = get_activate_string(
                        act,
                        &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                    )
                    .unwrap_or_else(|| format!("wmf-{i}"));
                    devices.push(DeviceInfo {
                        id: DeviceId(symlink),
                        name,
                    });
                }
                Ok::<_, HalError>(devices)
            })
            .await
            .map_err(|e| HalError::Io(format!("spawn_blocking: {e}")))?
        }

        async fn probe_capabilities(&self, id: &DeviceId) -> HalResult<DeviceCapabilities> {
            let dev_id = id.0.clone();
            tokio::task::spawn_blocking(move || unsafe {
                let activates = enumerate_video_devices()?;
                let activate = activates
                    .into_iter()
                    .find(|a| {
                        get_activate_string(
                            a,
                            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                        )
                        .map(|s| s == dev_id)
                        .unwrap_or(false)
                    })
                    .ok_or(HalError::DeviceNotFound)?;

                let source: IMFMediaSource = activate
                    .ActivateObject()
                    .map_err(|e| HalError::Io(format!("ActivateObject: {e}")))?;
                let reader: IMFSourceReader = MFCreateSourceReaderFromMediaSource(&source, None)
                    .map_err(|e| HalError::Io(format!("CreateSourceReader: {e}")))?;

                let mut formats = Vec::new();
                let mut idx = 0u32;
                while let Ok(media_type) = reader.GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, idx) {
                    if let Some(fd) = media_type_to_format(&media_type) {
                        if !formats.contains(&fd) {
                            formats.push(fd);
                        }
                    }
                    idx += 1;
                }
                let _ = source.Shutdown();
                Ok::<_, HalError>(DeviceCapabilities { formats })
            })
            .await
            .map_err(|e| HalError::Io(format!("spawn_blocking: {e}")))?
        }

        /// Open the device, negotiating the requested format.
        ///
        /// **Media Foundation does not clamp**, which is the one way this
        /// differs from the V4L2 side. `VIDIOC_S_FMT` substitutes the nearest
        /// supported mode and returns success; `SetCurrentMediaType` takes a
        /// media type the caller picked. So this searches the native types for
        /// an exact match, sets it, and reads back what it got:
        ///
        /// 1. walk `GetNativeMediaType` for a mode satisfying the request;
        /// 2. `SetCurrentMediaType` with it;
        /// 3. `GetCurrentMediaType` — requesting is not receiving;
        /// 4. `info` when the request was granted, `warn` when it was not;
        /// 5. **no match, or a failed set → keep MF's default and warn**, naming
        ///    both the request and what is running instead.
        ///
        /// Never a silent "nearest" substitution. A caller that asked for
        /// 640x480 YUY2, got 1920x1080 NV12 and was not told cannot notice —
        /// which is how `capture.pixel_format` came to be honoured by the mock
        /// backend alone on both platforms until 2026-09-02.
        ///
        /// `current_width`/`current_height`/`current_format` are set from the
        /// read-back, so telemetry reports what is delivered rather than what
        /// was asked for.
        async fn open_device(
            &self,
            id: &DeviceId,
            requested: Option<&crate::device::FormatRequest>,
        ) -> HalResult<()> {
            let dev_id = id.0.clone();
            let requested = requested.cloned();

            // Create reader + detect current format on blocking thread, then
            // move the resulting objects back into our Mutex-protected state.
            // This is safe because WmfState has unsafe Send/Sync and the reader
            // will only be used under the state mutex or in spawn_blocking.
            let (reader, source, fd) = tokio::task::spawn_blocking(move || unsafe {
                let activates = enumerate_video_devices()?;
                let activate = activates
                    .into_iter()
                    .find(|a| {
                        get_activate_string(
                            a,
                            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                        )
                        .map(|s| s == dev_id)
                        .unwrap_or(false)
                    })
                    .ok_or(HalError::DeviceNotFound)?;

                let source: IMFMediaSource = activate
                    .ActivateObject()
                    .map_err(|e| HalError::Io(format!("ActivateObject: {e}")))?;
                let reader: IMFSourceReader = MFCreateSourceReaderFromMediaSource(&source, None)
                    .map_err(|e| HalError::Io(format!("CreateSourceReader: {e}")))?;

                // ASK for the configured mode.
                //
                // MF has no clamping negotiation, so this is a search: walk the
                // native types, take the first that satisfies the request
                // exactly, and set it. Anything not found is reported, never
                // approximated — see `mode_satisfies`.
                //
                // A failure anywhere here falls back to the device's default
                // rather than propagating, for the same reason the V4L2 side
                // does: asking for something unsupported must not leave a
                // working camera worse off than before negotiation existed.
                if let Some(req) = requested.as_ref() {
                    // Walk once, collecting descriptors, then decide with the
                    // pure `choose_mode`. Collecting first keeps the decision
                    // out of the COM loop, which is what makes it testable
                    // without a camera — and on hardware offering a single
                    // frame rate that is the only way it can be tested at all.
                    let mut candidates: Vec<(u32, FormatDescriptor)> = Vec::new();
                    let mut idx = 0u32;
                    while let Ok(mt) =
                        reader.GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, idx)
                    {
                        if let Some(fd) = media_type_to_format(&mt) {
                            candidates.push((idx, fd));
                        }
                        idx += 1;
                    }
                    let picked = choose_mode(req, &candidates);
                    if let Some((_, false)) = picked {
                        if req.fps != 0 {
                            tracing::warn!(
                                "format negotiation: {}x{} {} is available but not at {} fps; \
                                 taking the mode and leaving the rate to the device",
                                req.width,
                                req.height,
                                req.pixel_format,
                                req.fps
                            );
                        }
                    }
                    // Re-fetch by index rather than holding the type through the
                    // decision: `GetNativeMediaType` is cheap and this keeps
                    // `choose_mode` free of COM entirely.
                    let chosen = picked.and_then(|(at, _)| {
                        reader
                            .GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, at)
                            .ok()
                            .map(|mt| (at, mt))
                    });
                    match chosen {
                        Some((found_at, mt)) => {
                            if let Err(e) = reader.SetCurrentMediaType(
                                MF_SOURCE_READER_FIRST_VIDEO_STREAM,
                                None,
                                &mt,
                            ) {
                                tracing::warn!(
                                    "format negotiation: native type {found_at} matched \
                                     {}x{} {} but SetCurrentMediaType failed ({e}); \
                                     falling back to the device's default mode",
                                    req.width,
                                    req.height,
                                    req.pixel_format
                                );
                            }
                        }
                        // Naming what is being used instead matters as much as
                        // naming what was refused: "not available" alone leaves
                        // a caller unable to tell whether anything is running.
                        None => tracing::warn!(
                            "format negotiation: no native mode matches the requested \
                             {}x{} {}; using the device's default mode instead. \
                             Run with the enumeration this device advertises to \
                             pick a supported one.",
                            req.width,
                            req.height,
                            req.pixel_format
                        ),
                    }
                }

                // Read back what we actually got. Authoritative for telemetry
                // whether or not anything was requested — requesting is not
                // receiving.
                let current_type = reader
                    .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM)
                    .map_err(|e| HalError::Io(format!("GetCurrentMediaType: {e}")))?;

                let fd = media_type_to_format(&current_type).unwrap_or(FormatDescriptor {
                    width: 640,
                    height: 480,
                    fps: 30,
                    pixel_format: PixelFormat::Bgr24,
                });

                // Say so either way. Silence is indistinguishable from success,
                // and a mode that was not granted is exactly what a caller most
                // needs to hear about.
                if let Some(req) = requested.as_ref() {
                    // The granted rate, read back from `MF_MT_FRAME_RATE` on the
                    // type the device is actually on — round 8 replaced the
                    // honest `None` that stood here while Windows did not read it.
                    match req.describe_mismatch(
                        fd.width,
                        fd.height,
                        &fd.pixel_format,
                        Some(fd.fps),
                    ) {
                        Some(diff) => tracing::warn!("format negotiation: {diff}"),
                        None => tracing::info!(
                            "format negotiation: granted {}x{} {} as requested",
                            fd.width,
                            fd.height,
                            fd.pixel_format
                        ),
                    }
                }

                Ok::<_, HalError>((SendReader(reader), SendSource(source), fd))
            })
            .await
            .map_err(|e| HalError::Io(format!("spawn_blocking: {e}")))??;

            let mut st = self.state.lock().unwrap();
            st.reader = Some(reader.0);
            st.source = Some(source.0);
            st.device_id = Some(id.0.clone());
            st.current_width = fd.width;
            st.current_height = fd.height;
            st.current_format = fd.pixel_format;
            st.current_fps = fd.fps;
            Ok(())
        }

        async fn close_device(&self, id: &DeviceId) -> HalResult<()> {
            let mut st = self.state.lock().unwrap();
            if st.device_id.as_deref() != Some(&id.0) {
                return Err(HalError::DeviceNotOpen);
            }
            st.reader = None;
            st.source = None;
            st.device_id = None;
            Ok(())
        }

        async fn read_frame(&self, id: &DeviceId) -> HalResult<Vec<u8>> {
            let reader = {
                let st = self.state.lock().unwrap();
                if st.device_id.as_deref() != Some(&id.0) {
                    return Err(HalError::DeviceNotOpen);
                }
                SendReader(st.reader.clone().ok_or(HalError::DeviceNotOpen)?)
            };

            // SAFETY: The closure only touches MF COM objects (IMFSourceReader,
            // IMFSample, IMFMediaBuffer) that live entirely within the blocking
            // thread.  MF is in MTA mode so calls from any thread are valid.
            // We wrap in an AssertSend helper because the intermediate COM
            // temporaries are technically !Send even though they never escape.
            struct AssertSend<F>(F);
            unsafe impl<F> Send for AssertSend<F> {}
            impl<F: FnOnce() -> T, T> AssertSend<F> {
                fn call(self) -> T {
                    (self.0)()
                }
            }

            let closure = AssertSend(move || unsafe {
                // MF_SOURCE_READER_FLAG bits
                const READERF_ERROR: u32 = 0x1;
                const READERF_ENDOFSTREAM: u32 = 0x2;

                // ReadSample may legitimately return no sample (stream ticks,
                // format-change notifications, device warm-up before the first
                // frame). Retry until a real sample arrives.
                let mut got: Option<IMFSample> = None;
                for _attempt in 0..50 {
                    let mut flags = 0u32;
                    let mut timestamp = 0i64;
                    let mut stream_idx = 0u32;
                    let mut sample: Option<IMFSample> = None;
                    reader
                        .0
                        .ReadSample(
                            MF_SOURCE_READER_FIRST_VIDEO_STREAM,
                            0,
                            Some(&mut stream_idx),
                            Some(&mut flags),
                            Some(&mut timestamp),
                            Some(&mut sample),
                        )
                        .map_err(|e| HalError::Io(format!("ReadSample: {e}")))?;

                    if flags & READERF_ERROR != 0 {
                        return Err(HalError::Io("ReadSample: stream error".into()));
                    }
                    if flags & READERF_ENDOFSTREAM != 0 {
                        return Err(HalError::Io("ReadSample: end of stream".into()));
                    }
                    if let Some(s) = sample {
                        got = Some(s);
                        break;
                    }
                }
                let sample =
                    got.ok_or(HalError::Io("ReadSample: no sample after 50 reads".into()))?;

                let buffer: IMFMediaBuffer = sample
                    .ConvertToContiguousBuffer()
                    .map_err(|e| HalError::Io(format!("ConvertToContiguousBuffer: {e}")))?;

                let mut buf_ptr = std::ptr::null_mut();
                let mut max_len = 0u32;
                let mut current_len = 0u32;
                buffer
                    .Lock(&mut buf_ptr, Some(&mut max_len), Some(&mut current_len))
                    .map_err(|e| HalError::Io(format!("Lock: {e}")))?;

                let data = std::slice::from_raw_parts(buf_ptr, current_len as usize).to_vec();

                buffer
                    .Unlock()
                    .map_err(|e| HalError::Io(format!("Unlock: {e}")))?;

                Ok::<_, HalError>(data)
            });

            tokio::task::spawn_blocking(move || closure.call())
                .await
                .map_err(|e| HalError::Io(format!("spawn_blocking: {e}")))?
        }

        /// Enumerate the controls this camera actually exposes.
        ///
        /// `GetRange` is asked for every property in [`WMF_CONTROLS`]; the ones
        /// the device does not implement fail and are skipped, so the returned
        /// list reflects the hardware rather than the table. min/max/step/
        /// default all come from the driver — nothing is invented.
        async fn list_controls(&self, id: &DeviceId) -> HalResult<Vec<ControlCapabilityInfo>> {
            let want = id.0.clone();
            let source = {
                let st = self.state.lock().unwrap();
                if st.device_id.as_deref() != Some(&want) {
                    return Err(HalError::DeviceNotOpen);
                }
                SendSource(st.source.clone().ok_or(HalError::DeviceNotOpen)?)
            };
            tokio::task::spawn_blocking(move || unsafe {
                // Bind the wrapper whole: Rust 2021 disjoint capture would otherwise
                // capture only `source.0`, an IMFMediaSource, and bypass the
                // `unsafe impl Send for SendSource`.
                let source = source;
                let (amp, cam) = control_interfaces(&source.0);
                let mut out = Vec::new();
                for (ns, prop, name) in WMF_CONTROLS {
                    let (mut mn, mut mx, mut step, mut def, mut caps) = (0i32, 0i32, 0i32, 0i32, 0i32);
                    let ok = match *ns {
                        NS_PROC_AMP => amp.as_ref().map(|a| {
                            a.GetRange(*prop, &mut mn, &mut mx, &mut step, &mut def, &mut caps)
                        }),
                        _ => cam.as_ref().map(|c| {
                            c.GetRange(*prop, &mut mn, &mut mx, &mut step, &mut def, &mut caps)
                        }),
                    };
                    // A property the device does not implement returns an error
                    // here. That is not a failure of enumeration: skip it.
                    if !matches!(ok, Some(Ok(()))) {
                        continue;
                    }
                    out.push(ControlCapabilityInfo {
                        id: encode_control_id(*ns, *prop),
                        name: (*name).to_string(),
                        min: mn as i64,
                        max: mx as i64,
                        step: step as i64,
                        default: def as i64,
                    });
                }
                Ok::<_, HalError>(out)
            })
            .await
            .map_err(|e| HalError::Io(format!("spawn_blocking join error: {e:?}")))?
        }

        async fn current_format(
            &self,
            id: &DeviceId,
        ) -> HalResult<Option<crate::device::FormatDescriptor>> {
            let st = self.state.lock().unwrap();
            if st.device_id.as_deref() != Some(&id.0) {
                return Ok(None);
            }
            Ok(Some(FormatDescriptor {
                width: st.current_width,
                height: st.current_height,
                fps: st.current_fps,
                pixel_format: st.current_format.clone(),
            }))
        }

        async fn get_control(&self, id: &DeviceId, control_id: u32) -> HalResult<i64> {
            let want = id.0.clone();
            let source = {
                let st = self.state.lock().unwrap();
                if st.device_id.as_deref() != Some(&want) {
                    return Err(HalError::DeviceNotOpen);
                }
                SendSource(st.source.clone().ok_or(HalError::DeviceNotOpen)?)
            };
            tokio::task::spawn_blocking(move || unsafe {
                // See the note in `list_controls`: bind the wrapper whole so the
                // closure captures `SendSource`, not the COM pointer inside it.
                let source = source;
                let (ns, prop) = decode_control_id(control_id);
                let (amp, cam) = control_interfaces(&source.0);
                let (mut value, mut flags) = (0i32, 0i32);
                let r = match ns {
                    NS_PROC_AMP => amp
                        .as_ref()
                        .ok_or_else(|| HalError::InvalidParameter(
                            "this camera exposes no IAMVideoProcAmp interface".into()))?
                        .Get(prop, &mut value, &mut flags),
                    NS_CAMERA_CONTROL => cam
                        .as_ref()
                        .ok_or_else(|| HalError::InvalidParameter(
                            "this camera exposes no IAMCameraControl interface".into()))?
                        .Get(prop, &mut value, &mut flags),
                    other => return Err(HalError::InvalidParameter(format!(
                        "control id {control_id:#x} names namespace {other}, which does not exist"))),
                };
                r.map_err(|e| HalError::Io(format!("control get ({control_id:#x}): {e}")))?;
                Ok::<_, HalError>(value as i64)
            })
            .await
            .map_err(|e| HalError::Io(format!("spawn_blocking join error: {e:?}")))?
        }

        /// Set a control to `value`.
        ///
        /// Always sets the **manual** flag: a caller asking for a specific
        /// value wants that value, and leaving auto engaged means the driver
        /// overwrites it on the next frame — the set would appear to succeed
        /// and then silently not hold.
        async fn set_control(
            &self,
            id: &DeviceId,
            control_id: u32,
            value: i64,
        ) -> HalResult<()> {
            let want = id.0.clone();
            let source = {
                let st = self.state.lock().unwrap();
                if st.device_id.as_deref() != Some(&want) {
                    return Err(HalError::DeviceNotOpen);
                }
                SendSource(st.source.clone().ok_or(HalError::DeviceNotOpen)?)
            };
            let v = i32::try_from(value)
                .map_err(|_| HalError::Io(format!("control value {value} out of range for WMF")))?;
            tokio::task::spawn_blocking(move || unsafe {
                // See the note in `list_controls`: bind the wrapper whole so the
                // closure captures `SendSource`, not the COM pointer inside it.
                let source = source;
                let (ns, prop) = decode_control_id(control_id);
                let (amp, cam) = control_interfaces(&source.0);
                let r = match ns {
                    NS_PROC_AMP => amp
                        .as_ref()
                        .ok_or_else(|| HalError::InvalidParameter(
                            "this camera exposes no IAMVideoProcAmp interface".into()))?
                        .Set(prop, v, VideoProcAmp_Flags_Manual.0),
                    NS_CAMERA_CONTROL => cam
                        .as_ref()
                        .ok_or_else(|| HalError::InvalidParameter(
                            "this camera exposes no IAMCameraControl interface".into()))?
                        .Set(prop, v, CameraControl_Flags_Manual.0),
                    other => return Err(HalError::InvalidParameter(format!(
                        "control id {control_id:#x} names namespace {other}, which does not exist"))),
                };
                r.map_err(|e| HalError::Io(format!("control set ({control_id:#x}={v}): {e}")))?;
                Ok::<_, HalError>(())
            })
            .await
            .map_err(|e| HalError::Io(format!("spawn_blocking join error: {e:?}")))?
        }
    }

    pub fn new_wmf_backend() -> HalResult<WmfBackend> {
        WmfBackend::new()
    }
}

#[cfg(windows)]
pub use wmf::WmfBackend;

/// The pure half of round 8's rate negotiation, exposed for tests. `mod wmf`
/// is private and the rest of it needs COM; these two are arithmetic and a
/// selection over a slice. `cfg(test)` as well as `cfg(windows)`, since the
/// production callers live inside `mod wmf` itself and an ungated re-export is
/// an unused import in a normal build.
#[cfg(all(windows, test))]
pub(crate) use wmf::{choose_mode, fps_from_ratio};

#[cfg(windows)]
pub fn new_wmf_backend() -> HalResult<WmfBackend> {
    wmf::new_wmf_backend()
}
