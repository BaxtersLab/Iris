/// Windows Media Foundation UVC backend.
///
/// Uses `MFEnumDeviceSources` with the video-capture attribute to enumerate
/// every camera visible to Windows (USB UVC, built-in, IP cameras exposed via
/// a driver, etc.).  Only available on Windows; guarded by `cfg(windows)`.
///
/// This backend currently supports `enumerate_devices` only; all frame-read
/// methods return `HalError::NotImplemented` until a full MF source pipeline
/// is wired up.
#[cfg(windows)]
pub mod wmf {
    use crate::backend::UvcBackend;
    use crate::device::{DeviceCapabilities, DeviceId, DeviceInfo};
    use crate::error::{HalError, HalResult};
    use async_trait::async_trait;

    use windows::{
        core::PWSTR,
        Win32::Media::MediaFoundation::{
            MFCreateAttributes, MFEnumDeviceSources, MFShutdown, MFStartup,
            IMFAttributes, IMFActivate,
            MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
            MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
            MFSTARTUP_NOSOCKET,
            MF_VERSION,
        },
        Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
    };

    pub struct WmfUvcBackend;

    impl WmfUvcBackend {
        pub fn new() -> Self {
            Self
        }

        /// Enumerate all video-capture devices visible to Windows MF.
        /// Returns an empty list (not an error) if MF initializes but finds nothing.
        pub fn enumerate_sync() -> HalResult<Vec<DeviceInfo>> {
            unsafe {
                // CoInitialize for this thread (MF needs COM)
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

                let hr = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
                if hr.is_err() {
                    CoUninitialize();
                    return Err(HalError::Io(format!(
                        "MFStartup failed: {:?}",
                        hr
                    )));
                }

                let result = (|| -> HalResult<Vec<DeviceInfo>> {
                    // Create attribute store — output-pointer pattern in windows 0.54
                    let mut attrs_opt: Option<IMFAttributes> = None;
                    MFCreateAttributes(&mut attrs_opt, 1)
                        .map_err(|e| HalError::Io(format!("MFCreateAttributes failed: {:?}", e)))?;
                    let attrs = attrs_opt.ok_or_else(|| HalError::Io("MFCreateAttributes returned null".into()))?;

                    // Set the source type to video capture
                    attrs
                        .SetGUID(
                            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                        )
                        .map_err(|e| HalError::Io(format!("SetGUID failed: {:?}", e)))?;

                    // Enumerate devices
                    let mut ppdevices: *mut Option<IMFActivate> = std::ptr::null_mut();
                    let mut count: u32 = 0;

                    MFEnumDeviceSources(&attrs, &mut ppdevices, &mut count)
                        .map_err(|e| HalError::Io(format!("MFEnumDeviceSources failed: {:?}", e)))?;

                    let mut devices: Vec<DeviceInfo> = Vec::new();

                    // Walk the array returned by MF (caller must CoTaskMemFree the array)
                    if !ppdevices.is_null() && count > 0 {
                        let slice = std::slice::from_raw_parts(ppdevices, count as usize);
                        for (i, activate_opt) in slice.iter().enumerate() {
                            if let Some(activate) = activate_opt {
                                // Get friendly name
                                let mut name_ptr = PWSTR::null();
                                let mut name_len: u32 = 0;
                                let name = if activate
                                    .GetAllocatedString(
                                        &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
                                        &mut name_ptr,
                                        &mut name_len,
                                    )
                                    .is_ok()
                                    && !name_ptr.is_null()
                                {
                                    let s = name_ptr.to_string().unwrap_or_default();
                                    windows::Win32::System::Com::CoTaskMemFree(Some(
                                        name_ptr.as_ptr() as *const _,
                                    ));
                                    s
                                } else {
                                    format!("Camera {}", i)
                                };

                                // Get symbolic link as stable ID
                                let mut link_ptr = PWSTR::null();
                                let mut link_len: u32 = 0;
                                let id = if activate
                                    .GetAllocatedString(
                                        &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                                        &mut link_ptr,
                                        &mut link_len,
                                    )
                                    .is_ok()
                                    && !link_ptr.is_null()
                                {
                                    let s = link_ptr.to_string().unwrap_or_default();
                                    windows::Win32::System::Com::CoTaskMemFree(Some(
                                        link_ptr.as_ptr() as *const _,
                                    ));
                                    s
                                } else {
                                    format!("vidcap-{}", i)
                                };

                                devices.push(DeviceInfo {
                                    id: DeviceId(id),
                                    name,
                                });
                            }
                        }

                        // Free the array itself
                        windows::Win32::System::Com::CoTaskMemFree(Some(
                            ppdevices as *const _,
                        ));
                    }

                    Ok(devices)
                })();

                let _ = MFShutdown();
                CoUninitialize();
                result
            }
        }
    }

    #[async_trait]
    impl UvcBackend for WmfUvcBackend {
        async fn enumerate_devices(&self) -> HalResult<Vec<DeviceInfo>> {
            // Run blocking COM/MF work on a dedicated thread
            tokio::task::spawn_blocking(WmfUvcBackend::enumerate_sync)
                .await
                .map_err(|e| HalError::Io(format!("spawn_blocking join error: {:?}", e)))?
        }

        async fn probe_capabilities(&self, _id: &DeviceId) -> HalResult<DeviceCapabilities> {
            Ok(DeviceCapabilities::default())
        }

        async fn open_device(&self, _id: &DeviceId) -> HalResult<()> {
            Err(HalError::NotImplemented)
        }

        async fn close_device(&self, _id: &DeviceId) -> HalResult<()> {
            Err(HalError::NotImplemented)
        }

        async fn read_frame(&self, _id: &DeviceId) -> HalResult<Vec<u8>> {
            Err(HalError::NotImplemented)
        }

        async fn list_controls(
            &self,
            _id: &DeviceId,
        ) -> HalResult<Vec<crate::device::ControlCapabilityInfo>> {
            Ok(vec![])
        }

        async fn get_control(&self, _id: &DeviceId, _control_id: u32) -> HalResult<i64> {
            Err(HalError::NotImplemented)
        }

        async fn set_control(
            &self,
            _id: &DeviceId,
            _control_id: u32,
            _value: i64,
        ) -> HalResult<()> {
            Err(HalError::NotImplemented)
        }
    }
}
