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
    async fn open_device(&self, id: &DeviceId) -> HalResult<()>;
    async fn close_device(&self, id: &DeviceId) -> HalResult<()>;
    async fn read_frame(&self, id: &DeviceId) -> HalResult<Vec<u8>>;
    async fn list_controls(&self, id: &DeviceId) -> HalResult<Vec<ControlCapabilityInfo>>;
    async fn get_control(&self, id: &DeviceId, control_id: u32) -> HalResult<i64>;
    async fn set_control(&self, id: &DeviceId, control_id: u32, value: i64) -> HalResult<()>;
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

    async fn open_device(&self, id: &DeviceId) -> HalResult<()> {
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
    use windows::Win32::Media::MediaFoundation::*;
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
    const MF_VIDEO_FORMAT_RGB24: GUID = GUID::from_u128(0x00000014_0000_0010_8000_00aa00389b71);
    const MF_VIDEO_FORMAT_RGB32: GUID = GUID::from_u128(0x00000016_0000_0010_8000_00aa00389b71);

    const MF_SOURCE_READER_FIRST_VIDEO_STREAM: u32 = 0xFFFFFFFC;

    fn subtype_to_pixel_format(guid: &GUID) -> Option<PixelFormat> {
        if *guid == MF_VIDEO_FORMAT_NV12 {
            Some(PixelFormat::Nv12)
        } else if *guid == MF_VIDEO_FORMAT_YUY2 {
            Some(PixelFormat::Yuyv)
        } else if *guid == MF_VIDEO_FORMAT_RGB24 {
            Some(PixelFormat::Rgb24)
        } else if *guid == MF_VIDEO_FORMAT_RGB32 {
            Some(PixelFormat::Bgr24)
        } else {
            None
        }
    }

    struct WmfState {
        reader: Option<IMFSourceReader>,
        device_id: Option<String>,
        current_width: u32,
        current_height: u32,
        current_format: PixelFormat,
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
                    device_id: None,
                    current_width: 0,
                    current_height: 0,
                    current_format: PixelFormat::Bgr24,
                }),
            })
        }
    }

    impl Drop for WmfBackend {
        fn drop(&mut self) {
            {
                let mut st = self.state.lock().unwrap();
                st.reader = None;
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
            .map(|r| {
                let num = (r >> 32) as u32;
                let den = (r & 0xFFFFFFFF) as u32;
                if den > 0 {
                    num / den
                } else {
                    30
                }
            })
            .unwrap_or(30);

        Some(FormatDescriptor {
            width,
            height,
            fps,
            pixel_format,
        })
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

        async fn open_device(&self, id: &DeviceId) -> HalResult<()> {
            let dev_id = id.0.clone();

            // Create reader + detect current format on blocking thread, then
            // move the resulting objects back into our Mutex-protected state.
            // This is safe because WmfState has unsafe Send/Sync and the reader
            // will only be used under the state mutex or in spawn_blocking.
            let (reader, fd) = tokio::task::spawn_blocking(move || unsafe {
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

                let current_type = reader
                    .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM)
                    .map_err(|e| HalError::Io(format!("GetCurrentMediaType: {e}")))?;

                let fd = media_type_to_format(&current_type).unwrap_or(FormatDescriptor {
                    width: 640,
                    height: 480,
                    fps: 30,
                    pixel_format: PixelFormat::Bgr24,
                });

                Ok::<_, HalError>((SendReader(reader), fd))
            })
            .await
            .map_err(|e| HalError::Io(format!("spawn_blocking: {e}")))??;

            let mut st = self.state.lock().unwrap();
            st.reader = Some(reader.0);
            st.device_id = Some(id.0.clone());
            st.current_width = fd.width;
            st.current_height = fd.height;
            st.current_format = fd.pixel_format;
            Ok(())
        }

        async fn close_device(&self, id: &DeviceId) -> HalResult<()> {
            let mut st = self.state.lock().unwrap();
            if st.device_id.as_deref() != Some(&id.0) {
                return Err(HalError::DeviceNotOpen);
            }
            st.reader = None;
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

                let sample = sample.ok_or(HalError::Io("ReadSample returned no sample".into()))?;

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

        async fn list_controls(&self, _id: &DeviceId) -> HalResult<Vec<ControlCapabilityInfo>> {
            Ok(vec![])
        }

        async fn get_control(&self, _id: &DeviceId, _control_id: u32) -> HalResult<i64> {
            Err(HalError::Io(
                "camera controls not yet implemented for WMF".into(),
            ))
        }

        async fn set_control(
            &self,
            _id: &DeviceId,
            _control_id: u32,
            _value: i64,
        ) -> HalResult<()> {
            Err(HalError::Io(
                "camera controls not yet implemented for WMF".into(),
            ))
        }
    }

    pub fn new_wmf_backend() -> HalResult<WmfBackend> {
        WmfBackend::new()
    }
}

#[cfg(windows)]
pub use wmf::WmfBackend;

#[cfg(windows)]
pub fn new_wmf_backend() -> HalResult<WmfBackend> {
    wmf::new_wmf_backend()
}
