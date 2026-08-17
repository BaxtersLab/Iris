/// Linux V4L2 UVC backend — the Linux mirror of `wmf_backend.rs`.
///
/// Enumerates every video-capture device under `/dev/video*` using raw V4L2
/// ioctls (`VIDIOC_QUERYCAP`, `VIDIOC_ENUM_FMT`, `VIDIOC_ENUM_FRAMESIZES`,
/// `VIDIOC_ENUM_FRAMEINTERVALS`) via libc — no bindgen, no extra system libs,
/// deterministic and dependency-light per the Iris design principles.
///
/// Full `UvcBackend` parity with the Phase-5 WMF backend: enumeration,
/// capability probing, and streaming capture via `VIDIOC_REQBUFS` /
/// `VIDIOC_QUERYBUF` / mmap / `VIDIOC_QBUF` / `VIDIOC_DQBUF`, plus user
/// controls through `VIDIOC_QUERYCTRL` / `G_CTRL` / `S_CTRL`.
///
/// The format is negotiated by adopting whatever mode the driver is already in
/// and reporting back what `VIDIOC_S_FMT` actually granted, so telemetry
/// describes the stream being delivered rather than the one requested — the
/// same rule the WMF backend follows.
///
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
    use std::sync::{Arc, Mutex};

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

    // ---- streaming I/O ABI ------------------------------------------------
    //
    // Encoded by hand as `_IOWR('V', nr, T)` = 0xC000_0000 | (size << 16) |
    // ('V' << 8) | nr, with `size` the x86_64 `sizeof(T)`. The sizes are
    // asserted in `tests::v4l2_abi_struct_sizes_match_kernel`, because a wrong
    // size makes the kernel read or write the wrong number of bytes through
    // our pointer rather than returning an error.

    pub const VIDIOC_G_FMT: libc::c_ulong = 0xc0d0_5604; // _IOWR('V',  4, v4l2_format[208])
    pub const VIDIOC_S_FMT: libc::c_ulong = 0xc0d0_5605; // _IOWR('V',  5, v4l2_format[208])
    pub const VIDIOC_REQBUFS: libc::c_ulong = 0xc014_5608; // _IOWR('V', 8, v4l2_requestbuffers[20])
    pub const VIDIOC_QUERYBUF: libc::c_ulong = 0xc058_5609; // _IOWR('V', 9, v4l2_buffer[88])
    pub const VIDIOC_QBUF: libc::c_ulong = 0xc058_560f; // _IOWR('V', 15, v4l2_buffer[88])
    pub const VIDIOC_DQBUF: libc::c_ulong = 0xc058_5611; // _IOWR('V', 17, v4l2_buffer[88])
    pub const VIDIOC_STREAMON: libc::c_ulong = 0x4004_5612; // _IOW('V', 18, int)
    pub const VIDIOC_STREAMOFF: libc::c_ulong = 0x4004_5613; // _IOW('V', 19, int)
    pub const VIDIOC_G_CTRL: libc::c_ulong = 0xc008_561b; // _IOWR('V', 27, v4l2_control[8])
    pub const VIDIOC_S_CTRL: libc::c_ulong = 0xc008_561c; // _IOWR('V', 28, v4l2_control[8])
    pub const VIDIOC_QUERYCTRL: libc::c_ulong = 0xc044_5624; // _IOWR('V', 36, v4l2_queryctrl[68])

    const V4L2_MEMORY_MMAP: u32 = 1;
    /// Set by the driver on a buffer whose frame is incomplete or corrupt.
    /// `uvcvideo` raises it for torn frames — commonly the first buffer after
    /// STREAMON, which can be a partial JPEG. The payload must be discarded.
    const V4L2_BUF_FLAG_ERROR: u32 = 0x0040;
    /// How many flagged/empty buffers `read_frame` will discard before giving
    /// up. Mirrors the WMF backend's ReadSample retry, which exists for the
    /// same reason: real cameras emit junk before they settle.
    const MAX_FRAME_RETRIES: u32 = 10;
    const V4L2_FIELD_NONE: u32 = 1;
    const V4L2_CTRL_FLAG_DISABLED: u32 = 0x0001;
    const V4L2_CID_BASE: u32 = 0x0098_0900;
    const V4L2_CID_LASTP1: u32 = V4L2_CID_BASE + 44;
    /// Buffers in the mmap queue. Four is the usual UVC choice: enough to
    /// absorb scheduling jitter without adding latency.
    const BUFFER_COUNT: u32 = 4;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct V4l2PixFormat {
        width: u32,
        height: u32,
        pixelformat: u32,
        field: u32,
        bytesperline: u32,
        sizeimage: u32,
        colorspace: u32,
        priv_: u32,
        flags: u32,
        enc: u32,
        quantization: u32,
        xfer_func: u32,
    }

    /// `type` + 4 bytes of padding (the union is 8-aligned because some of its
    /// members contain pointers) + a 200-byte union = 208 bytes.
    #[repr(C)]
    struct V4l2Format {
        typ: u32,
        _pad: u32,
        pix: V4l2PixFormat,
        _union_tail: [u8; 200 - std::mem::size_of::<V4l2PixFormat>()],
    }

    #[repr(C)]
    struct V4l2RequestBuffers {
        count: u32,
        typ: u32,
        memory: u32,
        capabilities: u32,
        flags: u8,
        reserved: [u8; 3],
    }

    #[repr(C)]
    struct V4l2Timecode {
        typ: u32,
        flags: u32,
        frames: u8,
        seconds: u8,
        minutes: u8,
        hours: u8,
        userbits: [u8; 4],
    }

    /// 88 bytes on x86_64. Note the explicit `_pad` after `field`: `timestamp`
    /// is a `struct timeval`, which is 8-aligned on 64-bit, so the compiler
    /// inserts 4 bytes there. Writing this struct without the pad silently
    /// shifts every subsequent member and the kernel returns the wrong buffer.
    #[repr(C)]
    struct V4l2Buffer {
        index: u32,
        typ: u32,
        bytesused: u32,
        flags: u32,
        field: u32,
        _pad: u32,
        timestamp: libc::timeval,
        timecode: V4l2Timecode,
        sequence: u32,
        memory: u32,
        m_offset: u64, // union m { __u32 offset; unsigned long userptr; ... }
        length: u32,
        reserved2: u32,
        request_fd: i32,
        _pad2: u32,
    }

    #[repr(C)]
    struct V4l2Control {
        id: u32,
        value: i32,
    }

    #[repr(C)]
    struct V4l2QueryCtrl {
        id: u32,
        typ: u32,
        name: [u8; 32],
        minimum: i32,
        maximum: i32,
        step: i32,
        default_value: i32,
        flags: u32,
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

    /// How long `read_frame` waits for the driver to fill a buffer. Generous
    /// enough for a UVC camera's first frame after STREAMON, which can lag.
    const FRAME_TIMEOUT_MS: libc::c_int = 2000;

    /// One mmap'd capture buffer. The address is kept as a `usize` rather than
    /// a raw pointer so the enclosing state stays `Send` without an
    /// `unsafe impl`; it is turned back into a pointer only inside the blocking
    /// section that owns the fd.
    struct MappedBuffer {
        ptr: usize,
        len: usize,
    }

    struct OpenDevice {
        fd: OwnedFd,
        path: String,
        buffers: Vec<MappedBuffer>,
        format: FormatDescriptor,
        streaming: bool,
    }

    impl OpenDevice {
        /// STREAMOFF then munmap every buffer. Idempotent.
        fn teardown(&mut self) {
            let raw = self.fd.as_raw_fd();
            if self.streaming {
                let mut typ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                unsafe { xioctl(raw, VIDIOC_STREAMOFF, &mut typ as *mut _ as *mut _) };
                self.streaming = false;
            }
            for b in self.buffers.drain(..) {
                unsafe { libc::munmap(b.ptr as *mut libc::c_void, b.len) };
            }
        }
    }

    impl Drop for OpenDevice {
        /// Runs teardown even when `close_device` was never called, so a
        /// dropped backend cannot leave the camera streaming or leak mappings.
        fn drop(&mut self) {
            self.teardown();
        }
    }

    #[derive(Clone)]
    pub struct V4l2UvcBackend {
        state: Arc<Mutex<Option<OpenDevice>>>,
    }

    impl V4l2UvcBackend {
        pub fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(None)),
            }
        }

        /// Open the node, negotiate a format, map buffers, start streaming.
        fn open_sync(id: &DeviceId) -> HalResult<OpenDevice> {
            let fd = open_node(&id.0).ok_or(HalError::DeviceNotFound)?;
            let raw = fd.as_raw_fd();
            let cap = query_cap(raw).ok_or(HalError::Io("VIDIOC_QUERYCAP failed".into()))?;
            if effective_caps(&cap) & V4L2_CAP_VIDEO_CAPTURE == 0 {
                return Err(HalError::InvalidParameter(format!(
                    "{} is not a video-capture device",
                    id.0
                )));
            }

            // Adopt the mode the driver is already in, then write it back so the
            // driver tells us what it actually granted. This mirrors the WMF
            // backend, which treats GetCurrentMediaType as authoritative rather
            // than asserting a requested resolution — telemetry must report the
            // format being delivered, not the one we asked for.
            let mut fmt: V4l2Format = unsafe { std::mem::zeroed() };
            fmt.typ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            if unsafe { xioctl(raw, VIDIOC_G_FMT, &mut fmt as *mut _ as *mut _) } == -1 {
                return Err(HalError::Io(format!(
                    "VIDIOC_G_FMT: {}",
                    std::io::Error::last_os_error()
                )));
            }
            fmt.pix.field = V4L2_FIELD_NONE;
            if unsafe { xioctl(raw, VIDIOC_S_FMT, &mut fmt as *mut _ as *mut _) } == -1 {
                return Err(HalError::Io(format!(
                    "VIDIOC_S_FMT: {}",
                    std::io::Error::last_os_error()
                )));
            }
            // S_FMT rewrites the struct in place with the granted format.
            let granted = fmt.pix;
            let pixel_format = fourcc_to_pixel_format(granted.pixelformat).ok_or_else(|| {
                HalError::InvalidParameter(format!(
                    "driver delivered unsupported fourcc {:#010x}",
                    granted.pixelformat
                ))
            })?;
            let format = FormatDescriptor {
                width: granted.width,
                height: granted.height,
                fps: first_fps(raw, granted.pixelformat, granted.width, granted.height),
                pixel_format,
            };

            let mut req: V4l2RequestBuffers = unsafe { std::mem::zeroed() };
            req.count = BUFFER_COUNT;
            req.typ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            req.memory = V4L2_MEMORY_MMAP;
            if unsafe { xioctl(raw, VIDIOC_REQBUFS, &mut req as *mut _ as *mut _) } == -1 {
                return Err(HalError::Io(format!(
                    "VIDIOC_REQBUFS: {}",
                    std::io::Error::last_os_error()
                )));
            }
            if req.count == 0 {
                return Err(HalError::Io("VIDIOC_REQBUFS granted 0 buffers".into()));
            }

            // From here on `dev` owns the fd, so any early return runs Drop and
            // unmaps whatever was mapped so far.
            let mut dev = OpenDevice {
                fd,
                path: id.0.clone(),
                buffers: Vec::new(),
                format,
                streaming: false,
            };

            for i in 0..req.count {
                let mut buf: V4l2Buffer = unsafe { std::mem::zeroed() };
                buf.typ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                buf.memory = V4L2_MEMORY_MMAP;
                buf.index = i;
                if unsafe { xioctl(raw, VIDIOC_QUERYBUF, &mut buf as *mut _ as *mut _) } == -1 {
                    return Err(HalError::Io(format!(
                        "VIDIOC_QUERYBUF({i}): {}",
                        std::io::Error::last_os_error()
                    )));
                }
                let ptr = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        buf.length as usize,
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_SHARED,
                        raw,
                        buf.m_offset as libc::off_t,
                    )
                };
                if ptr == libc::MAP_FAILED {
                    return Err(HalError::Io(format!(
                        "mmap buffer {i}: {}",
                        std::io::Error::last_os_error()
                    )));
                }
                dev.buffers.push(MappedBuffer {
                    ptr: ptr as usize,
                    len: buf.length as usize,
                });
                if unsafe { xioctl(raw, VIDIOC_QBUF, &mut buf as *mut _ as *mut _) } == -1 {
                    return Err(HalError::Io(format!(
                        "VIDIOC_QBUF({i}): {}",
                        std::io::Error::last_os_error()
                    )));
                }
            }

            let mut typ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            if unsafe { xioctl(raw, VIDIOC_STREAMON, &mut typ as *mut _ as *mut _) } == -1 {
                return Err(HalError::Io(format!(
                    "VIDIOC_STREAMON: {}",
                    std::io::Error::last_os_error()
                )));
            }
            dev.streaming = true;
            Ok(dev)
        }

        /// Wait for a filled buffer, copy it out, re-queue it.
        ///
        /// Buffers the driver flags as erroneous — and buffers with no payload —
        /// are re-queued and skipped rather than returned. Without this the
        /// first frame after STREAMON is often a torn JPEG: it carries a valid
        /// SOI marker but is truncated before EOI, so it looks like a real
        /// frame to anything that only checks the length.
        fn read_frame_sync(dev: &OpenDevice) -> HalResult<Vec<u8>> {
            let raw = dev.fd.as_raw_fd();
            let mut discarded = 0u32;

            for _ in 0..MAX_FRAME_RETRIES {
                // The node is opened O_NONBLOCK, so wait for readability rather
                // than spinning on EAGAIN.
                let mut pfd = libc::pollfd {
                    fd: raw,
                    events: libc::POLLIN,
                    revents: 0,
                };
                loop {
                    let r = unsafe { libc::poll(&mut pfd, 1, FRAME_TIMEOUT_MS) };
                    if r == -1 {
                        let e = std::io::Error::last_os_error();
                        if e.raw_os_error() == Some(libc::EINTR) {
                            continue;
                        }
                        return Err(HalError::Io(format!("poll: {e}")));
                    }
                    if r == 0 {
                        return Err(HalError::Io(format!(
                            "no frame within {FRAME_TIMEOUT_MS} ms"
                        )));
                    }
                    break;
                }

                let mut buf: V4l2Buffer = unsafe { std::mem::zeroed() };
                buf.typ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                buf.memory = V4L2_MEMORY_MMAP;
                if unsafe { xioctl(raw, VIDIOC_DQBUF, &mut buf as *mut _ as *mut _) } == -1 {
                    return Err(HalError::Io(format!(
                        "VIDIOC_DQBUF: {}",
                        std::io::Error::last_os_error()
                    )));
                }

                let idx = buf.index as usize;
                let mapped = dev.buffers.get(idx).ok_or_else(|| {
                    HalError::Io(format!("VIDIOC_DQBUF returned out-of-range index {idx}"))
                })?;

                let bad = buf.flags & V4L2_BUF_FLAG_ERROR != 0 || buf.bytesused == 0;

                // `bytesused` is authoritative, not `length`: an MJPEG frame is
                // far shorter than its buffer, and copying `length` would append
                // stale bytes from the previous frame.
                let used = (buf.bytesused as usize).min(mapped.len);
                let data = if bad {
                    Vec::new()
                } else {
                    unsafe { std::slice::from_raw_parts(mapped.ptr as *const u8, used).to_vec() }
                };

                // Re-queue before returning or the driver runs out of buffers.
                if unsafe { xioctl(raw, VIDIOC_QBUF, &mut buf as *mut _ as *mut _) } == -1 {
                    return Err(HalError::Io(format!(
                        "VIDIOC_QBUF (requeue): {}",
                        std::io::Error::last_os_error()
                    )));
                }

                if bad {
                    discarded += 1;
                    continue;
                }
                return Ok(data);
            }

            Err(HalError::Io(format!(
                "no usable frame after discarding {discarded} flagged buffers"
            )))
        }

        fn get_control_sync(dev: &OpenDevice, control_id: u32) -> HalResult<i64> {
            let mut ctrl = V4l2Control {
                id: control_id,
                value: 0,
            };
            if unsafe {
                xioctl(
                    dev.fd.as_raw_fd(),
                    VIDIOC_G_CTRL,
                    &mut ctrl as *mut _ as *mut _,
                )
            } == -1
            {
                return Err(HalError::Io(format!(
                    "VIDIOC_G_CTRL({control_id}): {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(ctrl.value as i64)
        }

        fn set_control_sync(dev: &OpenDevice, control_id: u32, value: i64) -> HalResult<()> {
            let v = i32::try_from(value).map_err(|_| {
                HalError::InvalidParameter(format!("control value {value} does not fit in i32"))
            })?;
            let mut ctrl = V4l2Control {
                id: control_id,
                value: v,
            };
            if unsafe {
                xioctl(
                    dev.fd.as_raw_fd(),
                    VIDIOC_S_CTRL,
                    &mut ctrl as *mut _ as *mut _,
                )
            } == -1
            {
                return Err(HalError::Io(format!(
                    "VIDIOC_S_CTRL({control_id}): {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(())
        }

        /// Enumerate the standard user controls a device exposes. Controls the
        /// driver does not implement return EINVAL and are skipped.
        fn list_controls_sync(id: &DeviceId) -> HalResult<Vec<ControlCapabilityInfo>> {
            let fd = open_node(&id.0).ok_or(HalError::DeviceNotFound)?;
            let raw = fd.as_raw_fd();
            let mut out = Vec::new();
            for cid in V4L2_CID_BASE..V4L2_CID_LASTP1 {
                let mut q: V4l2QueryCtrl = unsafe { std::mem::zeroed() };
                q.id = cid;
                if unsafe { xioctl(raw, VIDIOC_QUERYCTRL, &mut q as *mut _ as *mut _) } == -1 {
                    continue;
                }
                if q.flags & V4L2_CTRL_FLAG_DISABLED != 0 {
                    continue;
                }
                out.push(ControlCapabilityInfo {
                    id: q.id,
                    name: nul_trimmed(&q.name),
                    min: q.minimum as _,
                    max: q.maximum as _,
                    step: q.step as _,
                    default: q.default_value as _,
                });
            }
            Ok(out)
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

        async fn open_device(&self, id: &DeviceId) -> HalResult<()> {
            let state = Arc::clone(&self.state);
            let id = id.clone();
            tokio::task::spawn_blocking(move || {
                let mut guard = state.lock().unwrap();
                if guard.is_some() {
                    return Err(HalError::DeviceAlreadyOpen);
                }
                *guard = Some(V4l2UvcBackend::open_sync(&id)?);
                Ok(())
            })
            .await
            .map_err(|e| HalError::Io(format!("spawn_blocking join error: {e:?}")))?
        }

        async fn close_device(&self, id: &DeviceId) -> HalResult<()> {
            let state = Arc::clone(&self.state);
            let id = id.clone();
            tokio::task::spawn_blocking(move || {
                let mut guard = state.lock().unwrap();
                match guard.as_ref() {
                    Some(d) if d.path == id.0 => {}
                    _ => return Err(HalError::DeviceNotOpen),
                }
                // Dropping the OpenDevice runs teardown: STREAMOFF + munmap.
                *guard = None;
                Ok(())
            })
            .await
            .map_err(|e| HalError::Io(format!("spawn_blocking join error: {e:?}")))?
        }

        async fn read_frame(&self, id: &DeviceId) -> HalResult<Vec<u8>> {
            let state = Arc::clone(&self.state);
            let id = id.clone();
            // The lock is held across the poll+DQBUF, which serialises readers
            // against close_device. That matches the WMF backend, which also
            // serialises all reader access through its state mutex.
            tokio::task::spawn_blocking(move || {
                let guard = state.lock().unwrap();
                let dev = guard.as_ref().ok_or(HalError::DeviceNotOpen)?;
                if dev.path != id.0 {
                    return Err(HalError::DeviceNotOpen);
                }
                V4l2UvcBackend::read_frame_sync(dev)
            })
            .await
            .map_err(|e| HalError::Io(format!("spawn_blocking join error: {e:?}")))?
        }

        async fn list_controls(&self, id: &DeviceId) -> HalResult<Vec<ControlCapabilityInfo>> {
            let id = id.clone();
            tokio::task::spawn_blocking(move || V4l2UvcBackend::list_controls_sync(&id))
                .await
                .map_err(|e| HalError::Io(format!("spawn_blocking join error: {e:?}")))?
        }

        async fn get_control(&self, id: &DeviceId, control_id: u32) -> HalResult<i64> {
            let state = Arc::clone(&self.state);
            let id = id.clone();
            tokio::task::spawn_blocking(move || {
                let guard = state.lock().unwrap();
                let dev = guard.as_ref().ok_or(HalError::DeviceNotOpen)?;
                if dev.path != id.0 {
                    return Err(HalError::DeviceNotOpen);
                }
                V4l2UvcBackend::get_control_sync(dev, control_id)
            })
            .await
            .map_err(|e| HalError::Io(format!("spawn_blocking join error: {e:?}")))?
        }

        async fn set_control(&self, id: &DeviceId, control_id: u32, value: i64) -> HalResult<()> {
            let state = Arc::clone(&self.state);
            let id = id.clone();
            tokio::task::spawn_blocking(move || {
                let guard = state.lock().unwrap();
                let dev = guard.as_ref().ok_or(HalError::DeviceNotOpen)?;
                if dev.path != id.0 {
                    return Err(HalError::DeviceNotOpen);
                }
                V4l2UvcBackend::set_control_sync(dev, control_id, value)
            })
            .await
            .map_err(|e| HalError::Io(format!("spawn_blocking join error: {e:?}")))?
        }

        /// The format the driver actually granted at open time — authoritative
        /// for telemetry, per the same rule the WMF backend follows.
        async fn current_format(&self, id: &DeviceId) -> HalResult<Option<FormatDescriptor>> {
            let guard = self.state.lock().unwrap();
            Ok(match guard.as_ref() {
                Some(d) if d.path == id.0 => Some(d.format.clone()),
                _ => None,
            })
        }
    }

    #[cfg(test)]
    mod abi_tests {
        use super::*;
        use std::mem::size_of;

        /// A wrong `sizeof` here does **not** surface as an error: the kernel
        /// reads or writes the wrong number of bytes through our pointer and
        /// the corruption shows up somewhere else entirely. These are the
        /// x86_64 layouts from `linux/videodev2.h`.
        #[test]
        fn v4l2_abi_struct_sizes_match_kernel() {
            assert_eq!(size_of::<V4l2PixFormat>(), 48, "v4l2_pix_format");
            assert_eq!(size_of::<V4l2Format>(), 208, "v4l2_format");
            assert_eq!(size_of::<V4l2RequestBuffers>(), 20, "v4l2_requestbuffers");
            assert_eq!(size_of::<V4l2Timecode>(), 16, "v4l2_timecode");
            assert_eq!(size_of::<V4l2Buffer>(), 88, "v4l2_buffer");
            assert_eq!(size_of::<V4l2Control>(), 8, "v4l2_control");
            assert_eq!(size_of::<V4l2QueryCtrl>(), 68, "v4l2_queryctrl");
        }

        /// Sizes alone are **not** sufficient. Declaring `m_offset` as `u32`
        /// instead of `u64` — a plausible mistake, since the union's `offset`
        /// member really is a `__u32` while the union itself is 8 bytes — leaves
        /// the total at 88 because the trailing padding absorbs it, yet shifts
        /// `length` and everything after it by 4. The kernel would then read the
        /// buffer length from the wrong 4 bytes. Offsets are the real invariant.
        #[test]
        fn v4l2_buffer_field_offsets_match_kernel() {
            use std::mem::offset_of;
            assert_eq!(offset_of!(V4l2Buffer, index), 0);
            assert_eq!(offset_of!(V4l2Buffer, typ), 4);
            assert_eq!(offset_of!(V4l2Buffer, bytesused), 8);
            assert_eq!(offset_of!(V4l2Buffer, flags), 12);
            assert_eq!(offset_of!(V4l2Buffer, field), 16);
            // 20..24 is padding: timeval is 8-aligned on x86_64.
            assert_eq!(offset_of!(V4l2Buffer, timestamp), 24);
            assert_eq!(offset_of!(V4l2Buffer, timecode), 40);
            assert_eq!(offset_of!(V4l2Buffer, sequence), 56);
            assert_eq!(offset_of!(V4l2Buffer, memory), 60);
            assert_eq!(offset_of!(V4l2Buffer, m_offset), 64);
            assert_eq!(offset_of!(V4l2Buffer, length), 72);
            assert_eq!(offset_of!(V4l2Buffer, reserved2), 76);
        }

        /// Same reasoning for the format structs: `pix` must start at 8, after
        /// `type` plus 4 bytes of padding.
        #[test]
        fn v4l2_format_field_offsets_match_kernel() {
            use std::mem::offset_of;
            assert_eq!(offset_of!(V4l2Format, typ), 0);
            assert_eq!(offset_of!(V4l2Format, pix), 8);
            assert_eq!(offset_of!(V4l2PixFormat, width), 0);
            assert_eq!(offset_of!(V4l2PixFormat, height), 4);
            assert_eq!(offset_of!(V4l2PixFormat, pixelformat), 8);
            assert_eq!(offset_of!(V4l2PixFormat, field), 12);
            assert_eq!(offset_of!(V4l2PixFormat, bytesperline), 16);
            assert_eq!(offset_of!(V4l2PixFormat, sizeimage), 20);
        }

        /// The ioctl numbers encode the struct size in bits 16..30. Recompute
        /// them from `size_of` so a struct edit cannot silently desynchronise a
        /// hand-written constant from the type it describes.
        #[test]
        fn v4l2_ioctl_codes_match_their_struct_sizes() {
            const DIR_WRITE: libc::c_ulong = 1 << 30;
            const DIR_READ_WRITE: libc::c_ulong = 3 << 30;
            let enc = |dir: libc::c_ulong, nr: libc::c_ulong, size: usize| {
                dir | ((size as libc::c_ulong) << 16) | ((b'V' as libc::c_ulong) << 8) | nr
            };

            assert_eq!(VIDIOC_G_FMT, enc(DIR_READ_WRITE, 4, size_of::<V4l2Format>()));
            assert_eq!(VIDIOC_S_FMT, enc(DIR_READ_WRITE, 5, size_of::<V4l2Format>()));
            assert_eq!(
                VIDIOC_REQBUFS,
                enc(DIR_READ_WRITE, 8, size_of::<V4l2RequestBuffers>())
            );
            assert_eq!(
                VIDIOC_QUERYBUF,
                enc(DIR_READ_WRITE, 9, size_of::<V4l2Buffer>())
            );
            assert_eq!(VIDIOC_QBUF, enc(DIR_READ_WRITE, 15, size_of::<V4l2Buffer>()));
            assert_eq!(
                VIDIOC_DQBUF,
                enc(DIR_READ_WRITE, 17, size_of::<V4l2Buffer>())
            );
            assert_eq!(
                VIDIOC_G_CTRL,
                enc(DIR_READ_WRITE, 27, size_of::<V4l2Control>())
            );
            assert_eq!(
                VIDIOC_S_CTRL,
                enc(DIR_READ_WRITE, 28, size_of::<V4l2Control>())
            );
            assert_eq!(
                VIDIOC_QUERYCTRL,
                enc(DIR_READ_WRITE, 36, size_of::<V4l2QueryCtrl>())
            );
            // STREAMON/STREAMOFF take a plain int, write-only.
            assert_eq!(
                VIDIOC_STREAMON,
                enc(DIR_WRITE, 18, size_of::<libc::c_int>())
            );
            assert_eq!(
                VIDIOC_STREAMOFF,
                enc(DIR_WRITE, 19, size_of::<libc::c_int>())
            );
        }

        /// The pre-existing enumeration ioctls, checked the same way, so the
        /// whole table is covered by one rule rather than two conventions.
        #[test]
        fn v4l2_enumeration_ioctl_codes_match_their_struct_sizes() {
            const DIR_READ: libc::c_ulong = 2 << 30;
            const DIR_READ_WRITE: libc::c_ulong = 3 << 30;
            let enc = |dir: libc::c_ulong, nr: libc::c_ulong, size: usize| {
                dir | ((size as libc::c_ulong) << 16) | ((b'V' as libc::c_ulong) << 8) | nr
            };
            assert_eq!(
                VIDIOC_QUERYCAP,
                enc(DIR_READ, 0, size_of::<V4l2Capability>())
            );
            assert_eq!(VIDIOC_ENUM_FMT, enc(DIR_READ_WRITE, 2, size_of::<V4l2FmtDesc>()));
            assert_eq!(
                VIDIOC_ENUM_FRAMESIZES,
                enc(DIR_READ_WRITE, 74, size_of::<V4l2FrmSizeEnum>())
            );
            assert_eq!(
                VIDIOC_ENUM_FRAMEINTERVALS,
                enc(DIR_READ_WRITE, 75, size_of::<V4l2FrmIvalEnum>())
            );
        }
    }
}
