use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub String);

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormatDescriptor {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub pixel_format: PixelFormat,
}

/// A capture format the caller would like the device to deliver.
///
/// **Advisory, not a command.** V4L2's `VIDIOC_S_FMT` and Media Foundation's
/// `SetCurrentMediaType` both *negotiate*: the driver is free to substitute the
/// nearest mode it supports, and it reports what it actually granted. So a
/// caller must read back what it got rather than assume — which is why
/// `current_format` stays authoritative for telemetry.
///
/// This type exists because for most of Iris's life nothing was requested at
/// all: `capture.pixel_format`, `capture.width` and `capture.height` were
/// validated, passed down, and then overwritten by whatever mode the device
/// happened to already be in. The knob was honoured by the mock backend alone.
#[derive(Debug, Clone, PartialEq)]
pub struct FormatRequest {
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    /// Frames per second. `0` means "do not ask", leaving the device's current
    /// interval alone.
    ///
    /// Negotiated **separately from the pixel mode** on Linux — `VIDIOC_S_PARM`
    /// carries the interval, `VIDIOC_S_FMT` carries width/height/fourcc — which
    /// is exactly why this field outlived two rounds of format negotiation
    /// still being honoured by the mock backend alone. It was never in the
    /// struct being fixed. On Windows it is an attribute of the media type, so
    /// the two platforms reach it by genuinely different routes.
    pub fps: u32,
}

impl FormatRequest {
    /// Does a delivered mode satisfy this request **exactly**?
    ///
    /// Exactly is the point. Media Foundation does not clamp — it takes the
    /// media type the caller picked — so on Windows "no match" is a real
    /// outcome that Iris must resolve itself, and the one thing it must never
    /// do is substitute a nearest mode silently. A caller that asked for
    /// 640x480 YUY2, got 1920x1080 NV12 and was not told has no way to notice.
    /// That is exactly how `capture.pixel_format` came to be honoured by the
    /// mock backend alone on both platforms until 2026-09-02.
    ///
    /// V4L2 reaches the same place by the opposite route: `VIDIOC_S_FMT`
    /// *does* clamp and still returns success, so a difference there is normal
    /// and equally silent unless someone compares.
    /// **`fps` is deliberately excluded**, and a test pins that.
    ///
    /// This decides which *native mode* satisfies the request, and frame rate is
    /// not part of a mode on either platform in the same way: on Linux it is a
    /// separate ioctl applied after the format is set, and on Windows a single
    /// resolution/subtype pair is offered at several rates as distinct media
    /// types. Folding fps in here would make the Windows search demand an exact
    /// rate match and find nothing on a camera whose default rate differs —
    /// turning a working negotiation into a silent fallback.
    ///
    /// The consequence is real and worth knowing: a device offering one
    /// resolution at two frame rates gets whichever the search reaches first.
    /// Choosing between them is `FormatRequest::fps`'s job, through the
    /// platform's own mechanism.
    pub fn is_satisfied_by(&self, width: u32, height: u32, pixel_format: &PixelFormat) -> bool {
        self.width == width && self.height == height && self.pixel_format == *pixel_format
    }

    /// `None` when the request was granted, or a description of the difference.
    ///
    /// Names **both** sides on purpose: "not available" without saying what is
    /// running instead leaves a caller unable to tell whether anything is.
    /// `granted_fps` is `Some` only where the platform actually negotiates a
    /// frame rate. `None` means "not asked, or not knowable here" and the rate
    /// is left out of the comparison entirely — which is honest, and is what
    /// the Windows backend passes until it matches `MF_MT_FRAME_RATE`.
    /// Comparing against a rate nobody read would invent mismatches.
    pub fn describe_mismatch(
        &self,
        width: u32,
        height: u32,
        pixel_format: &PixelFormat,
        granted_fps: Option<u32>,
    ) -> Option<String> {
        let mode_ok = self.is_satisfied_by(width, height, pixel_format);
        // `fps == 0` means nothing was asked, so nothing can be wrong.
        let fps_ok = match (self.fps, granted_fps) {
            (0, _) | (_, None) => true,
            (want, Some(got)) => want == got,
        };
        if mode_ok && fps_ok {
            return None;
        }
        let fmt_rate = |r: Option<u32>| match r {
            Some(v) => format!(" @{v}fps"),
            None => String::new(),
        };
        let asked_rate = if self.fps == 0 {
            String::new()
        } else {
            format!(" @{}fps", self.fps)
        };
        Some(format!(
            "requested {}x{} {}{} but the device is delivering {}x{} {}{}",
            self.width,
            self.height,
            self.pixel_format,
            asked_rate,
            width,
            height,
            pixel_format,
            fmt_rate(granted_fps)
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PixelFormat {
    Rgb24,
    Bgr24,
    Nv12,
    Yuyv,
    /// Motion-JPEG — a **compressed** stream, one JPEG image per frame.
    ///
    /// Unlike every other variant this is not raw pixel data, so it cannot be
    /// cropped, strided or indexed without being decoded first. Callers must
    /// treat it as an opaque byte blob.
    ///
    /// It is enumerated because on USB 2.0 UVC cameras nearly every mode above
    /// ~640x480 is MJPEG-only (uncompressed 1080p exceeds USB 2.0 bandwidth);
    /// skipping it makes such cameras appear far more limited than they are.
    /// Windows Media Foundation decodes MJPEG transparently and reports NV12,
    /// so this variant is only ever produced by the Linux V4L2 path.
    Mjpeg,
}

impl PixelFormat {
    /// True when the variant is raw pixel data that can be cropped/indexed
    /// directly. False for compressed streams, which must be decoded first.
    pub fn is_raw(&self) -> bool {
        !matches!(self, PixelFormat::Mjpeg)
    }

    /// Parse the `capture.pixel_format` string from `iris.toml`.
    ///
    /// Case-insensitive, and `yuy2` is accepted as the Windows spelling of the
    /// same 4:2:2 layout `yuyv` names on Linux — the two are the same fourcc.
    ///
    /// The names `IrisConfig::validate` accepts and the names this parses are
    /// held together by a test in this crate; before 2026-08-31 nothing parsed
    /// this field at all, so the config list had drifted to include `bgra8`
    /// (which no Iris backend has ever produced) while omitting `rgb24` and
    /// `bgr24`, one of which was what capture actually ran.
    pub fn from_config_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "rgb24" => Some(PixelFormat::Rgb24),
            "bgr24" => Some(PixelFormat::Bgr24),
            "nv12" => Some(PixelFormat::Nv12),
            "yuyv" | "yuy2" => Some(PixelFormat::Yuyv),
            "mjpeg" | "mjpg" => Some(PixelFormat::Mjpeg),
            _ => None,
        }
    }

    /// The canonical `iris.toml` spelling of this format.
    pub fn config_name(&self) -> &'static str {
        match self {
            PixelFormat::Rgb24 => "rgb24",
            PixelFormat::Bgr24 => "bgr24",
            PixelFormat::Nv12 => "nv12",
            PixelFormat::Yuyv => "yuyv",
            PixelFormat::Mjpeg => "mjpeg",
        }
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PixelFormat::Rgb24 => write!(f, "RGB24"),
            PixelFormat::Bgr24 => write!(f, "BGR24"),
            PixelFormat::Nv12 => write!(f, "NV12"),
            PixelFormat::Yuyv => write!(f, "YUYV"),
            PixelFormat::Mjpeg => write!(f, "MJPEG"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceCapabilities {
    pub formats: Vec<FormatDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlCapabilityInfo {
    pub id: u32,
    pub name: String,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub default: i64,
}
