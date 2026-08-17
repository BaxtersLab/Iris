/// Linux V4L2 UVC backend — the Linux mirror of `wmf_backend.rs`.
///
/// Enumerates every video-capture device under `/dev/video*` using raw V4L2
/// ioctls (`VIDIOC_QUERYCAP`, `VIDIOC_ENUM_FMT`, `VIDIOC_ENUM_FRAMESIZES`,
/// `VIDIOC_ENUM_FRAMEINTERVALS`) via libc — no bindgen, no extra system libs,
/// deterministic and dependency-light per the Iris design principles.
///
/// Like the Phase-5 WMF backend this currently supports `enumerate_devices`
/// and `probe_capabilities`; frame-read methods return
/// `HalError::NotImplemented` until the full capture pipeline is wired up.
/// Only available on Linux; guarded by `cfg(target_os = "linux")`.
#[cfg(target_os = "linux")]
pub mod v4l2 {
    use crate::backend::UvcBackend;
    use crate::device::{
        ControlCapabilityInfo, DeviceCapabilities, DeviceId, DeviceInfo, FormatDescriptor,
        PixelFormat,
    };
    use crate::error::{HalError, HalResult};
    use async_trait::async_trait;
    use std::fs;
    use std::os::fd::{AsRawFd, OwnedFd};

    // ---- V4L2 ABI (from linux/videodev2.h) --------------------------------

    pub const VIDIOC_QUERYCAP: libc::c_ulong = 0x8068_5600; // _IOR('V', 0, v4l2_capability[104])
    pub const VIDIOC_ENUM_FMT: libc::c_ulong = 0xc040_5602; // _IOWR('V', 2, v4l2_fmtdesc[64])
    pub const VIDIOC_ENUM_FRAMESIZES: libc::c_ulong = 0xc02c_564a; // _IOWR('V', 74, [44])
    pub const VIDIOC_ENUM_FRAMEINTERVALS: libc::c_ulong = 0xc034_564b; // _IOWR('V', 75, [52])

    const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
    const V4L2_FRMSIZE_TYPE_DISCRETE: u32 = 1;
    const V4L2_FRMIVAL_TYPE_DISCRETE: u32 = 1;
    const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
    const V4L2_CAP_DEVICE_CAPS: u32 = 0x8000_0000;

    #[repr(C)]
    struct V4l2Capability {
        driver: [u8; 16],
        card: [u8; 32],
        bus_info: [u8; 32],
        version: u32,
        capabilities: u32,
        device_caps: u32,
        reserved: [u32; 3],
    }

    #[repr(C)]
    struct V4l2FmtDesc {
        index: u32,
        typ: u32,
        flags: u32,
        description: [u8; 32],
        pixelformat: u32,
        mbus_code: u32,
        reserved: [u32; 3],
    }

    #[repr(C)]
    struct V4l2FrmSizeEnum {
        index: u32,
        pixel_format: u32,
        typ: u32,
        // union { discrete: {width, height}, stepwise: {6 x u32} } — 24 bytes
        union_data: [u32; 6],
        reserved: [u32; 2],
    }

    #[repr(C)]
    struct V4l2FrmIvalEnum {
        index: u32,
        pixel_format: u32,
        width: u32,
        height: u32,
        typ: u32,
        // union { discrete: v4l2_fract{num, den}, stepwise: {3 x fract} } — 24 bytes
        union_data: [u32; 6],
        reserved: [u32; 2],
    }

    // ---- helpers ----------------------------------------------------------

    /// FourCC little-endian code -> Iris PixelFormat (None = unsupported).
    pub fn fourcc_to_pixel_format(fourcc: u32) -> Option<PixelFormat> {
        match fourcc {
            0x5659_5559 => Some(PixelFormat::Yuyv),  // 'YUYV'
            0x3231_564e => Some(PixelFormat::Nv12),  // 'NV12'
            0x3342_4752 => Some(PixelFormat::Rgb24), // 'RGB3'
            0x3352_4742 => Some(PixelFormat::Bgr24), // 'BGR3'
            // 'MJPG' — compressed. Enumerated because on USB 2.0 UVC cameras
            // every mode above ~640x480 is usually MJPEG-only; without this a
            // 1080p-capable camera reports as 640x480-only on Linux. (Windows
            // Media Foundation decodes MJPEG and reports NV12, so the WMF path
            // never sees it.)
            0x4750_4a4d => Some(PixelFormat::Mjpeg), // 'MJPG'
            _ => None,
        }
    }

    /// Is this /dev entry name a V4L2 video node ("video" + digits)?
    pub fn is_video_node(name: &str) -> bool {
        name.strip_prefix("video")
            .map(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false)
    }

    fn nul_trimmed(bytes: &[u8]) -> String {
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }

    /// ioctl with EINTR retry (classic V4L2 pattern).
    unsafe fn xioctl(fd: libc::c_int, req: libc::c_ulong, arg: *mut libc::c_void) -> libc::c_int {
        loop {
            let r = libc::ioctl(fd, req as _, arg);
            if r == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return r;
        }
    }

    fn open_node(path: &str) -> Option<OwnedFd> {
        use std::os::fd::FromRawFd;
        let c_path = std::ffi::CString::new(path).ok()?;
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return None;
        }
        Some(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn query_cap(fd: libc::c_int) -> Option<V4l2Capability> {
        let mut cap: V4l2Capability = unsafe { std::mem::zeroed() };
        let r = unsafe { xioctl(fd, VIDIOC_QUERYCAP, &mut cap as *mut _ as *mut _) };
        if r == -1 {
            return None;
        }
        Some(cap)
    }

    fn effective_caps(cap: &V4l2Capability) -> u32 {
        if cap.capabilities & V4L2_CAP_DEVICE_CAPS != 0 {
            cap.device_caps
        } else {
            cap.capabilities
        }
    }

    /// First discrete fps for (fourcc, w, h); defaults to 30.
    fn first_fps(fd: libc::c_int, fourcc: u32, width: u32, height: u32) -> u32 {
        let mut ival: V4l2FrmIvalEnum = unsafe { std::mem::zeroed() };
        ival.pixel_format = fourcc;
        ival.width = width;
        ival.height = height;
        let r = unsafe { xioctl(fd, VIDIOC_ENUM_FRAMEINTERVALS, &mut ival as *mut _ as *mut _) };
        if r == 0 && ival.typ == V4L2_FRMIVAL_TYPE_DISCRETE {
            let num = ival.union_data[0];
            let den = ival.union_data[1];
            if num > 0 && den > 0 {
                return den / num;
            }
        }
        30
    }

    pub struct V4l2UvcBackend;

    impl V4l2UvcBackend {
        pub fn new() -> Self {
            Self
        }

        /// Enumerate all V4L2 capture devices. Returns an empty list (not an
        /// error) when no camera is present — mirrors WMF behaviour.
        pub fn enumerate_sync() -> HalResult<Vec<DeviceInfo>> {
            let entries = match fs::read_dir("/dev") {
                Ok(e) => e,
                Err(e) => return Err(HalError::Io(format!("read /dev: {e}"))),
            };
            let mut nodes: Vec<String> = entries
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| is_video_node(n))
                .collect();
            // numeric sort: video2 < video10
            nodes.sort_by_key(|n| n[5..].parse::<u32>().unwrap_or(u32::MAX));

            let mut devices = Vec::new();
            for node in nodes {
                let path = format!("/dev/{node}");
                let Some(fd) = open_node(&path) else { continue };
                let Some(cap) = query_cap(fd.as_raw_fd()) else { continue };
                // skip metadata/output nodes — capture nodes only
                if effective_caps(&cap) & V4L2_CAP_VIDEO_CAPTURE == 0 {
                    continue;
                }
                let name = {
                    let card = nul_trimmed(&cap.card);
                    if card.is_empty() {
                        node.clone()
                    } else {
                        card
                    }
                };
                devices.push(DeviceInfo {
                    id: DeviceId(path),
                    name,
                });
            }
            Ok(devices)
        }

        /// Probe supported formats/sizes/fps for a device id (a /dev/videoN path).
        pub fn probe_capabilities_sync(id: &DeviceId) -> HalResult<DeviceCapabilities> {
            let fd = open_node(&id.0).ok_or(HalError::DeviceNotFound)?;
            let raw = fd.as_raw_fd();
            let cap = query_cap(raw).ok_or(HalError::Io("VIDIOC_QUERYCAP failed".into()))?;
            if effective_caps(&cap) & V4L2_CAP_VIDEO_CAPTURE == 0 {
                return Err(HalError::InvalidParameter("not a capture device".into()));
            }

            let mut formats = Vec::new();
            let mut fmt_idx = 0u32;
            loop {
                let mut desc: V4l2FmtDesc = unsafe { std::mem::zeroed() };
                desc.index = fmt_idx;
                desc.typ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                let r = unsafe { xioctl(raw, VIDIOC_ENUM_FMT, &mut desc as *mut _ as *mut _) };
                if r == -1 {
                    break; // EINVAL = end of format list
                }
                if let Some(pixel_format) = fourcc_to_pixel_format(desc.pixelformat) {
                    let mut size_idx = 0u32;
                    loop {
                        let mut fs: V4l2FrmSizeEnum = unsafe { std::mem::zeroed() };
                        fs.index = size_idx;
                        fs.pixel_format = desc.pixelformat;
                        let r = unsafe {
                            xioctl(raw, VIDIOC_ENUM_FRAMESIZES, &mut fs as *mut _ as *mut _)
                        };
                        if r == -1 {
                            break;
                        }
                        if fs.typ == V4L2_FRMSIZE_TYPE_DISCRETE {
                            let (width, height) = (fs.union_data[0], fs.union_data[1]);
                            let fd_desc = FormatDescriptor {
                                width,
                                height,
                                fps: first_fps(raw, desc.pixelformat, width, height),
                                pixel_format: pixel_format.clone(),
                            };
                            if !formats.contains(&fd_desc) {
                                formats.push(fd_desc);
                            }
                            size_idx += 1;
                        } else {
                            // stepwise/continuous: record the max as one descriptor
                            let fd_desc = FormatDescriptor {
                                width: fs.union_data[1],
                                height: fs.union_data[4],
                                fps: 30,
                                pixel_format: pixel_format.clone(),
                            };
                            if !formats.contains(&fd_desc) {
                                formats.push(fd_desc);
                            }
                            break;
                        }
                    }
                }
                fmt_idx += 1;
            }
            Ok(DeviceCapabilities { formats })
        }
    }

    impl Default for V4l2UvcBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait]
    impl UvcBackend for V4l2UvcBackend {
        async fn enumerate_devices(&self) -> HalResult<Vec<DeviceInfo>> {
            tokio::task::spawn_blocking(V4l2UvcBackend::enumerate_sync)
                .await
                .map_err(|e| HalError::Io(format!("spawn_blocking join error: {e:?}")))?
        }

        async fn probe_capabilities(&self, id: &DeviceId) -> HalResult<DeviceCapabilities> {
            let id = id.clone();
            tokio::task::spawn_blocking(move || V4l2UvcBackend::probe_capabilities_sync(&id))
                .await
                .map_err(|e| HalError::Io(format!("spawn_blocking join error: {e:?}")))?
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

        async fn list_controls(&self, _id: &DeviceId) -> HalResult<Vec<ControlCapabilityInfo>> {
            Ok(vec![])
        }

        async fn get_control(&self, _id: &DeviceId, _control_id: u32) -> HalResult<i64> {
            Err(HalError::NotImplemented)
        }

        async fn set_control(&self, _id: &DeviceId, _control_id: u32, _value: i64) -> HalResult<()> {
            Err(HalError::NotImplemented)
        }
    }
}
