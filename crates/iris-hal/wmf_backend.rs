/// Windows Media Foundation camera **enumeration**.
///
/// Uses `MFEnumDeviceSources` with the video-capture attribute to list every
/// camera visible to Windows (USB UVC, built-in, IP cameras exposed via a
/// driver, etc.). Only available on Windows; guarded by `cfg(windows)`.
///
/// **This is deliberately not a `UvcBackend`.** Real capture on Windows is
/// `backend::WmfBackend`, which owns a Media Foundation source reader and,
/// crucially, thread-scoped COM state: its `new()` calls `CoInitializeEx` +
/// `MFStartup` and its `Drop` calls `MFShutdown` + `CoUninitialize`, which is
/// why `bootstrap.rs` builds one on a dedicated long-lived thread. This type
/// carries none of that — `enumerate_sync` initialises COM, enumerates, and
/// tears down again within the one call, so it is safe to call from anywhere,
/// including an async task on an arbitrary worker thread.
///
/// It previously also implemented `UvcBackend` with five `NotImplemented`
/// stubs (`open_device`, `close_device`, `read_frame`, `get_control`,
/// `set_control`). Nothing ever called them — `bootstrap.rs` only ever used
/// the associated `enumerate_sync` — but being a `UvcBackend` made this type
/// indistinguishable from the real one at a call site, so picking the wrong
/// one silently got you `NotImplemented` from something that looked like the
/// camera backend. The impl is gone; the trap went with it.
#[cfg(windows)]
pub mod wmf {
    use crate::device::{DeviceId, DeviceInfo};
    use crate::error::{HalError, HalResult};

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

}
