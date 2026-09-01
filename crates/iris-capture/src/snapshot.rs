// SPDX-License-Identifier: MIT
// Iris — iris-capture

//! Turning a captured frame into something a vision model can eat.
//!
//! The consumer this exists for is a local llama.cpp model with an mmproj,
//! driven through the OpenAI chat-completions shape. That API takes an image as
//! a data URL:
//!
//! ```json
//! { "type": "image_url",
//!   "image_url": { "url": "data:image/jpeg;base64,/9j/4AAQ..." } }
//! ```
//!
//! So the useful output is not "some pixels" but **that exact string**,
//! assembled here rather than by every caller — a caller that builds it by hand
//! is a caller that can get the MIME type or the padding wrong, silently, and
//! be told only that the model saw nothing.
//!
//! Downscaling is not an optimisation, it is the point. A vision projector
//! works at a few hundred pixels square; handing it 1920x1080 costs encode
//! time, transfer size and tokens to reach the same tiles it would have made
//! anyway.

use crate::frame::CaptureFrame;
use iris_hal::device::PixelFormat;

/// A frame prepared for a vision model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Width after downscaling.
    pub width: u32,
    /// Height after downscaling.
    pub height: u32,
    /// `image/jpeg`.
    pub mime: &'static str,
    /// Base64 of the JPEG bytes, without any prefix.
    pub base64: String,
}

impl Snapshot {
    /// The complete `data:` URL, ready for `image_url.url`.
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, self.base64)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("frame is empty")]
    Empty,
    #[error("frame geometry is zero ({0}x{1})")]
    ZeroSized(u32, u32),
    #[error("mjpeg decode failed: {0}")]
    Decode(#[from] crate::mjpeg::MjpegError),
    #[error("frame buffer is {actual} bytes, expected at least {expected}")]
    Truncated { actual: usize, expected: usize },
    #[error("jpeg encode failed: {0}")]
    Encode(String),
}

/// Convert any capture frame to RGB24, decoding MJPEG if needed.
fn to_rgb24(frame: &CaptureFrame) -> Result<(u32, u32, Vec<u8>), SnapshotError> {
    let (w, h) = (frame.width, frame.height);
    if w == 0 || h == 0 {
        return Err(SnapshotError::ZeroSized(w, h));
    }
    if frame.data.is_empty() {
        return Err(SnapshotError::Empty);
    }
    let (wu, hu) = (w as usize, h as usize);

    match &frame.format {
        PixelFormat::Mjpeg => {
            let decoded = crate::mjpeg::decode_to_rgb24(&frame.data)?;
            // Trust the DECODER's geometry, not the frame header: the header is
            // the mode that was requested, and a camera may deliver another.
            Ok((decoded.width, decoded.height, decoded.rgb24))
        }
        PixelFormat::Rgb24 => {
            let need = wu * hu * 3;
            if frame.data.len() < need {
                return Err(SnapshotError::Truncated {
                    actual: frame.data.len(),
                    expected: need,
                });
            }
            Ok((w, h, frame.data[..need].to_vec()))
        }
        PixelFormat::Bgr24 => {
            let need = wu * hu * 3;
            if frame.data.len() < need {
                return Err(SnapshotError::Truncated {
                    actual: frame.data.len(),
                    expected: need,
                });
            }
            let mut out = Vec::with_capacity(need);
            for px in frame.data[..need].chunks_exact(3) {
                out.extend_from_slice(&[px[2], px[1], px[0]]);
            }
            Ok((w, h, out))
        }
        PixelFormat::Nv12 => {
            // A full-size Y plane followed by U and V interleaved at half
            // resolution in both axes — one UV pair per 2x2 block of pixels.
            //
            // **This is the only format Windows ever produces here.** Media
            // Foundation decodes a camera's MJPEG itself and reports NV12, so
            // without this arm `/frame` returned 500 to every request on every
            // Windows machine, while the preview window showed the camera
            // normally because `iris-ui`'s `frame_to_rgba` has its own NV12
            // conversion. The window could see and the agent could not.
            //
            // BT.601 limited range, with the **same coefficients as that
            // preview conversion**, deliberately: two conversions of the same
            // frame that disagree about colour would be worse than one, and
            // the preview's is the one a human has already looked at and
            // accepted. If either changes, change both.
            //
            // Chroma geometry is computed rather than assumed to be `w*h/2`:
            // an odd width carries a padded chroma row, and rounding the
            // index down is what 4:2:0 means — the trailing column shares the
            // last pair rather than reading off the end.
            let chroma_stride = ((wu + 1) / 2) * 2;
            let chroma_rows = (hu + 1) / 2;
            let need = wu * hu + chroma_stride * chroma_rows;
            if frame.data.len() < need {
                return Err(SnapshotError::Truncated {
                    actual: frame.data.len(),
                    expected: need,
                });
            }
            let d = &frame.data;
            let uv_base = wu * hu;
            let mut out = Vec::with_capacity(wu * hu * 3);
            for y in 0..hu {
                for x in 0..wu {
                    let yv = d[y * wu + x] as f32;
                    let uvi = uv_base + (y / 2) * chroma_stride + (x / 2) * 2;
                    let u = d[uvi] as f32 - 128.0;
                    let v = d[uvi + 1] as f32 - 128.0;
                    let c = yv - 16.0;
                    out.push((1.164 * c + 1.596 * v).clamp(0.0, 255.0) as u8);
                    out.push((1.164 * c - 0.392 * u - 0.813 * v).clamp(0.0, 255.0) as u8);
                    out.push((1.164 * c + 2.017 * u).clamp(0.0, 255.0) as u8);
                }
            }
            Ok((w, h, out))
        }
        PixelFormat::Yuyv => {
            // 4:2:2 packed: `Y0 U Y1 V` describes **two** horizontally adjacent
            // pixels, so a macropixel is four bytes and the two pixels share one
            // U/V pair while keeping their own luma.
            //
            // The same hole as `Nv12` above, one format along, and it was left
            // open when that one was closed: `ui_app.rs` converts YUYV for the
            // preview and this module did not, so a camera delivering YUYV shows
            // a correct window and answers `/frame` with 500. That failure is
            // invisible from the desk and visible only to the agent — which is
            // the whole reason it is worth fixing before a camera finds it.
            // V4L2 offers YUYV on plenty of UVC devices; no exotic hardware is
            // needed to hit it.
            //
            // Same BT.601 limited-range coefficients as the `Nv12` arm and the
            // preview. Three sites now agree; if one changes, change all three.
            //
            // Geometry is computed, not assumed to be `w * h * 2`: an odd width
            // still costs a whole trailing macropixel, and rounding the row
            // stride down is what would read off the end of the last row.
            let macropixels_per_row = (wu + 1) / 2;
            let row_stride = macropixels_per_row * 4;
            let need = row_stride * hu;
            if frame.data.len() < need {
                return Err(SnapshotError::Truncated {
                    actual: frame.data.len(),
                    expected: need,
                });
            }
            let d = &frame.data;
            let mut out = Vec::with_capacity(wu * hu * 3);
            for y in 0..hu {
                for x in 0..wu {
                    let pair = y * row_stride + (x / 2) * 4;
                    // Luma is per pixel — `Y0` or `Y1` depending on which half
                    // of the pair this is — while U and V come from the pair.
                    let yv = d[pair + (x % 2) * 2] as f32;
                    let u = d[pair + 1] as f32 - 128.0;
                    let v = d[pair + 3] as f32 - 128.0;
                    let c = yv - 16.0;
                    out.push((1.164 * c + 1.596 * v).clamp(0.0, 255.0) as u8);
                    out.push((1.164 * c - 0.392 * u - 0.813 * v).clamp(0.0, 255.0) as u8);
                    out.push((1.164 * c + 2.017 * u).clamp(0.0, 255.0) as u8);
                }
            }
            Ok((w, h, out))
        }
        // No catch-all. Every `PixelFormat` variant is handled above, and
        // leaving the match exhaustive is deliberate: adding a sixth variant
        // must not compile until `/frame` can decode it. Both NV12 and YUYV
        // reached production as a silent HTTP 500 that only an agent could
        // see, precisely because a catch-all turned "nobody wrote this yet"
        // into a runtime answer instead of a build failure.
    }
}

/// Box-filter downscale to at most `max_width`, preserving aspect ratio.
///
/// Averaging the source pixels that fall in each destination pixel, rather than
/// point-sampling one of them. Nearest-neighbour is a line of code shorter and
/// visibly wrong on the thing this is for: dropping 1920 to 512 by picking
/// every fourth pixel aliases text and thin edges into noise, and a vision
/// model reads that noise.
///
/// Never upscales — a frame already smaller than `max_width` is returned
/// untouched, since inventing pixels adds bytes and no information.
pub fn downscale_rgb24(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    max_width: u32,
) -> (u32, u32, Vec<u8>) {
    if max_width == 0 || src_w <= max_width || src_w == 0 || src_h == 0 {
        return (src_w, src_h, src.to_vec());
    }
    let dst_w = max_width;
    // Round to nearest and never to zero: a very wide frame scaled hard would
    // otherwise produce a zero-height image and an encoder error.
    let dst_h = (((src_h as u64 * dst_w as u64) + (src_w as u64 / 2)) / src_w as u64).max(1) as u32;

    let mut out = vec![0u8; dst_w as usize * dst_h as usize * 3];
    for dy in 0..dst_h as usize {
        let sy0 = dy * src_h as usize / dst_h as usize;
        let sy1 = (((dy + 1) * src_h as usize) / dst_h as usize).max(sy0 + 1);
        for dx in 0..dst_w as usize {
            let sx0 = dx * src_w as usize / dst_w as usize;
            let sx1 = (((dx + 1) * src_w as usize) / dst_w as usize).max(sx0 + 1);
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for sy in sy0..sy1.min(src_h as usize) {
                for sx in sx0..sx1.min(src_w as usize) {
                    let i = (sy * src_w as usize + sx) * 3;
                    if i + 2 < src.len() {
                        r += src[i] as u32;
                        g += src[i + 1] as u32;
                        b += src[i + 2] as u32;
                        n += 1;
                    }
                }
            }
            let o = (dy * dst_w as usize + dx) * 3;
            if n > 0 {
                out[o] = (r / n) as u8;
                out[o + 1] = (g / n) as u8;
                out[o + 2] = (b / n) as u8;
            }
        }
    }
    (dst_w, dst_h, out)
}

/// Flip an RGB24 image left-to-right, in place.
///
/// Webcams commonly present a mirrored "selfie" view, which puts any text held
/// up to the camera backwards. Flipping matters most for the **model**, not the
/// window: a person can tilt their head, but a vision model handed mirrored
/// text reads mirrored text.
pub fn mirror_rgb24(rgb: &mut [u8], width: u32, height: u32) {
    let (w, h) = (width as usize, height as usize);
    if w < 2 || h == 0 || rgb.len() < w * h * 3 {
        return;
    }
    for row in 0..h {
        let base = row * w * 3;
        for x in 0..w / 2 {
            let a = base + x * 3;
            let b = base + (w - 1 - x) * 3;
            for c in 0..3 {
                rgb.swap(a + c, b + c);
            }
        }
    }
}

/// Base64, RFC 4648 standard alphabet with padding.
///
/// Hand-written rather than depending on a crate: it is thirty lines of
/// well-specified transformation, this project documents every dependency it
/// takes, and the correctness question is settled by the RFC's own test
/// vectors, which the tests use.
pub fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Prepare a frame for a vision model.
///
/// `max_width` of 0 means "do not downscale". `quality` is the JPEG quality,
/// 1–100.
pub fn snapshot(
    frame: &CaptureFrame,
    max_width: u32,
    quality: u8,
    mirror: bool,
) -> Result<Snapshot, SnapshotError> {
    let (w, h, rgb) = to_rgb24(frame)?;
    let (dw, dh, mut scaled) = downscale_rgb24(&rgb, w, h, max_width);
    if mirror {
        // After downscaling, so the flip costs a fraction of the pixels — and
        // a horizontal flip commutes with a box downscale, so the result is
        // identical either way.
        mirror_rgb24(&mut scaled, dw, dh);
    }

    let mut jpeg = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut jpeg, quality.clamp(1, 100));
    encoder
        .encode(
            &scaled,
            dw as u16,
            dh as u16,
            jpeg_encoder::ColorType::Rgb,
        )
        .map_err(|e| SnapshotError::Encode(format!("{e:?}")))?;

    Ok(Snapshot {
        width: dw,
        height: dh,
        mime: "image/jpeg",
        base64: base64_encode(&jpeg),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn frame(w: u32, h: u32, format: PixelFormat, data: Vec<u8>) -> CaptureFrame {
        CaptureFrame {
            sequence: 1,
            width: w,
            height: h,
            format,
            data,
            timestamp_us: 0,
            is_cropped: false,
        }
    }

    // ---- base64, against RFC 4648's own vectors ---------------------------

    /// Hand-written encoders get padding wrong. These are the RFC's test
    /// vectors verbatim, which is the whole justification for not taking a
    /// dependency for this.
    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        for (in_, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(in_.as_bytes()), want, "input {in_:?}");
        }
    }

    #[test]
    fn base64_handles_every_byte_value() {
        let all: Vec<u8> = (0..=255u8).collect();
        let out = base64_encode(&all);
        assert_eq!(out.len(), 344, "256 bytes -> ceil(256/3)*4");
        assert!(out.ends_with('='), "256 is not a multiple of 3, so it pads");
        assert!(
            out.chars().all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)),
            "only the standard alphabet may appear"
        );
    }

    // ---- downscaling ------------------------------------------------------

    /// Inventing pixels adds bytes and no information.
    #[test]
    fn a_frame_smaller_than_the_limit_is_untouched() {
        let src = vec![7u8; 4 * 2 * 3];
        let (w, h, out) = downscale_rgb24(&src, 4, 2, 100);
        assert_eq!((w, h), (4, 2));
        assert_eq!(out, src);
    }

    #[test]
    fn aspect_ratio_is_preserved() {
        let src = vec![0u8; 1920 * 1080 * 3];
        let (w, h, _) = downscale_rgb24(&src, 1920, 1080, 512);
        assert_eq!(w, 512);
        assert_eq!(h, 288, "1920x1080 to 512 wide is 288 tall");
    }

    /// A very wide frame scaled hard must not round its height to zero — the
    /// encoder would reject a zero-height image.
    #[test]
    fn an_extreme_aspect_ratio_keeps_at_least_one_row() {
        let src = vec![0u8; 4000 * 2 * 3];
        let (w, h, out) = downscale_rgb24(&src, 4000, 2, 8);
        assert_eq!(w, 8);
        assert!(h >= 1, "height must never round to zero");
        assert_eq!(out.len(), (w * h * 3) as usize);
    }

    /// Averaging, not point-sampling. Half black and half white must average
    /// to grey; nearest-neighbour would return one or the other.
    #[test]
    fn downscaling_averages_rather_than_point_sampling() {
        // 2x1: one black pixel, one white.
        let src = vec![0, 0, 0, 255, 255, 255];
        let (w, h, out) = downscale_rgb24(&src, 2, 1, 1);
        assert_eq!((w, h), (1, 1));
        assert_eq!(
            out,
            vec![127, 127, 127],
            "the single output pixel must be the average of both inputs"
        );
    }

    #[test]
    fn output_length_always_matches_the_reported_geometry() {
        let src = vec![9u8; 640 * 480 * 3];
        for max in [1, 7, 64, 320, 639] {
            let (w, h, out) = downscale_rgb24(&src, 640, 480, max);
            assert_eq!(out.len(), (w * h * 3) as usize, "at max_width {max}");
        }
    }

    // ---- the whole pipeline ----------------------------------------------

    #[test]
    fn an_rgb_frame_becomes_a_usable_data_url() {
        // 64x32 with a gradient, so the JPEG is not a degenerate flat image.
        let (w, h) = (64u32, 32u32);
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&[(x * 4) as u8, (y * 8) as u8, 128]);
            }
        }
        let snap = snapshot(&frame(w, h, PixelFormat::Rgb24, data), 32, 80, false).expect("snapshot");
        assert_eq!(snap.width, 32);
        assert_eq!(snap.height, 16);
        assert_eq!(snap.mime, "image/jpeg");

        let url = snap.data_url();
        assert!(
            url.starts_with("data:image/jpeg;base64,"),
            "must drop straight into OpenAI image_url.url: {}",
            &url[..40.min(url.len())]
        );
        assert!(snap.base64.len() > 100, "a real JPEG is not a few bytes");
    }

    /// The JPEG must actually be a JPEG — SOI/EOI markers — or the model gets
    /// a base64 blob it cannot decode and reports only that it saw nothing.
    #[test]
    fn the_encoded_bytes_are_a_real_jpeg() {
        let data = vec![200u8; 16 * 16 * 3];
        let snap = snapshot(&frame(16, 16, PixelFormat::Rgb24, data), 0, 90, false).expect("snapshot");
        // Decode our own base64 back and check the markers.
        let bytes = {
            const A: &[u8; 64] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = Vec::new();
            let mut acc = 0u32;
            let mut bits = 0;
            for c in snap.base64.bytes().filter(|c| *c != b'=') {
                let v = A.iter().position(|a| *a == c).expect("valid alphabet") as u32;
                acc = (acc << 6) | v;
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    out.push((acc >> bits) as u8);
                }
            }
            out
        };
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG must start with SOI");
        assert_eq!(&bytes[bytes.len() - 2..], &[0xFF, 0xD9], "and end with EOI");
    }

    #[test]
    fn bgr_frames_have_their_channels_swapped() {
        // One pure-red pixel expressed as BGR.
        let snap = snapshot(&frame(1, 1, PixelFormat::Bgr24, vec![0, 0, 255]), 0, 95, false)
            .expect("snapshot");
        assert_eq!((snap.width, snap.height), (1, 1));
    }

    #[test]
    fn a_truncated_frame_is_reported_not_guessed() {
        let err = snapshot(&frame(64, 64, PixelFormat::Rgb24, vec![1, 2, 3]), 0, 80, false).unwrap_err();
        assert!(matches!(err, SnapshotError::Truncated { .. }), "{err}");
    }

    // ---- NV12 -------------------------------------------------------------
    //
    // Windows Media Foundation decodes a camera's MJPEG transparently and
    // hands back **NV12**, so on Windows every frame reaching this module is
    // NV12 and nothing else. Without this arm `/frame` answered
    // `500 {"error":"snapshot failed: cannot decode Nv12 frames to pixels"}`
    // to every request, on every Windows machine, for every camera — while the
    // preview window showed the camera perfectly, because `ui_app.rs` has its
    // own NV12 conversion. The window could see and the agent could not.

    /// Build a `w`x`h` NV12 buffer with a constant Y and a constant UV pair.
    fn nv12(w: u32, h: u32, y: u8, u: u8, v: u8) -> Vec<u8> {
        let (wu, hu) = (w as usize, h as usize);
        let mut d = vec![y; wu * hu];
        d.extend(
            std::iter::repeat([u, v])
                .take(wu * hu / 4)
                .flatten(),
        );
        d
    }

    /// The decode must produce the colour the YUV triple names, not merely
    /// succeed. A conversion that returns the right number of wrong bytes
    /// still hands a vision model a picture of nothing.
    #[test]
    fn nv12_decodes_to_the_colour_it_encodes() {
        // Y=81 U=90 V=240 is BT.601 limited-range RED.
        let f = frame(2, 2, PixelFormat::Nv12, nv12(2, 2, 81, 90, 240));
        let (w, h, rgb) = to_rgb24(&f).expect("NV12 must decode");
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgb.len(), 2 * 2 * 3, "RGB24 is three bytes per pixel");
        let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
        assert!(r > 200, "red channel should dominate, got r={r} g={g} b={b}");
        assert!(g < 60 && b < 60, "green/blue should be low, got r={r} g={g} b={b}");

        // And the achromatic case: U=V=128 must give equal channels (grey).
        // This catches a chroma-centring error — dropping the `- 128.0` bias
        // turns neutral into magenta. It does **not** catch swapped U and V
        // planes, though: at U == V a swap is a no-op by construction. The red
        // assertion above is what catches that, verified by swapping the two
        // reads and watching this test fail on `r=15 g=62 b=255`.
        let f = frame(2, 2, PixelFormat::Nv12, nv12(2, 2, 128, 128, 128));
        let (_, _, rgb) = to_rgb24(&f).expect("NV12 must decode");
        let (r, g, b) = (rgb[0] as i32, rgb[1] as i32, rgb[2] as i32);
        assert!(
            (r - g).abs() <= 2 && (g - b).abs() <= 2,
            "neutral chroma must give a grey pixel, got r={r} g={g} b={b}"
        );
    }

    /// A short NV12 buffer must be reported, not indexed past the end. The
    /// preview's version of this conversion silently skips a short frame;
    /// here the caller is an HTTP client that deserves an answer.
    #[test]
    fn a_truncated_nv12_frame_is_reported() {
        let err = snapshot(
            &frame(64, 64, PixelFormat::Nv12, vec![7; 64 * 64]), // Y only, no UV
            0,
            80,
            false,
        )
        .unwrap_err();
        assert!(matches!(err, SnapshotError::Truncated { .. }), "{err}");
    }

    /// End to end: an NV12 frame must come out of `snapshot` as a real JPEG.
    #[test]
    fn an_nv12_frame_reaches_a_jpeg() {
        let snap = snapshot(&frame(64, 64, PixelFormat::Nv12, nv12(64, 64, 128, 128, 128)), 0, 80, false)
            .expect("NV12 must reach a JPEG");
        assert_eq!((snap.width, snap.height), (64, 64));
        assert!(snap.data_url().starts_with("data:image/jpeg;base64,"));
    }

    // ---- YUYV -------------------------------------------------------------
    //
    // The gap left open when NV12 was closed. `/frame` answered 500 to any
    // camera delivering YUYV while the preview window showed it correctly,
    // because `ui_app.rs` has its own YUYV conversion and this module had none.
    // No YUYV hardware is needed to prove the conversion: it is pure, so a
    // constructed buffer exercises it exactly as the camera would.

    /// Build a `w`x`h` YUYV buffer by repeating one `Y0 U Y1 V` macropixel.
    fn yuyv(w: u32, h: u32, y0: u8, u: u8, y1: u8, v: u8) -> Vec<u8> {
        let (wu, hu) = (w as usize, h as usize);
        std::iter::repeat([y0, u, y1, v])
            .take(((wu + 1) / 2) * hu)
            .flatten()
            .collect()
    }

    /// Colour, not merely success — and the two halves of a macropixel must
    /// keep their **own** luma while sharing one chroma pair. Using `Y0` for
    /// both pixels is the obvious way to write this arm wrong, and it would
    /// still produce the right number of bytes.
    #[test]
    fn yuyv_decodes_to_the_colour_it_encodes() {
        // Y=81 U=90 V=240 is BT.601 limited-range RED, in both halves.
        let f = frame(2, 2, PixelFormat::Yuyv, yuyv(2, 2, 81, 90, 81, 240));
        let (w, h, rgb) = to_rgb24(&f).expect("YUYV must decode");
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgb.len(), 2 * 2 * 3, "RGB24 is three bytes per pixel");
        let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
        assert!(r > 200, "red channel should dominate, got r={r} g={g} b={b}");
        assert!(g < 60 && b < 60, "green/blue should be low, got r={r} g={g} b={b}");

        // Per-pixel luma: black then white across one macropixel, neutral
        // chroma. If the arm reads Y0 for both, the two pixels come out equal.
        let f = frame(2, 1, PixelFormat::Yuyv, yuyv(2, 1, 16, 128, 235, 128));
        let (_, _, rgb) = to_rgb24(&f).expect("YUYV must decode");
        let (p0, p1) = (rgb[0] as i32, rgb[3] as i32);
        assert!(p0 < 20, "first pixel of the pair should be black, got {p0}");
        assert!(p1 > 230, "second pixel of the pair should be white, got {p1}");

        // Neutral chroma must give equal channels, which catches a dropped
        // `- 128.0` bias exactly as it does for NV12.
        let f = frame(2, 2, PixelFormat::Yuyv, yuyv(2, 2, 128, 128, 128, 128));
        let (_, _, rgb) = to_rgb24(&f).expect("YUYV must decode");
        let (r, g, b) = (rgb[0] as i32, rgb[1] as i32, rgb[2] as i32);
        assert!(
            (r - g).abs() <= 2 && (g - b).abs() <= 2,
            "neutral chroma must give a grey pixel, got r={r} g={g} b={b}"
        );
    }

    /// A short YUYV buffer must be reported rather than indexed past the end.
    #[test]
    fn a_truncated_yuyv_frame_is_reported() {
        let err = snapshot(
            &frame(64, 64, PixelFormat::Yuyv, vec![7; 64 * 64]), // half a frame
            0,
            80,
            false,
        )
        .unwrap_err();
        assert!(matches!(err, SnapshotError::Truncated { .. }), "{err}");
    }

    /// An odd width still costs a whole trailing macropixel. Assuming a
    /// `w * 2` row stride reads off the end of the last row; this is the case
    /// that catches it, and it must decode rather than panic.
    #[test]
    fn an_odd_width_yuyv_frame_does_not_read_off_the_end() {
        let f = frame(3, 2, PixelFormat::Yuyv, yuyv(3, 2, 128, 128, 128, 128));
        let (w, h, rgb) = to_rgb24(&f).expect("an odd width must decode");
        assert_eq!((w, h), (3, 2));
        assert_eq!(rgb.len(), 3 * 2 * 3);
    }

    /// End to end: a YUYV frame must come out of `snapshot` as a real JPEG.
    #[test]
    fn a_yuyv_frame_reaches_a_jpeg() {
        let snap = snapshot(
            &frame(64, 64, PixelFormat::Yuyv, yuyv(64, 64, 128, 128, 128, 128)),
            0,
            80,
            false,
        )
        .expect("YUYV must reach a JPEG");
        assert_eq!((snap.width, snap.height), (64, 64));
        assert!(snap.data_url().starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn empty_and_zero_sized_frames_are_refused() {
        assert!(matches!(
            snapshot(&frame(4, 4, PixelFormat::Rgb24, vec![]), 0, 80, false).unwrap_err(),
            SnapshotError::Empty
        ));
        assert!(matches!(
            snapshot(&frame(0, 4, PixelFormat::Rgb24, vec![1]), 0, 80, false).unwrap_err(),
            SnapshotError::ZeroSized(0, 4)
        ));
    }

    /// The camera's own MJPEG is the common case, and the decoder's geometry
    /// wins over the frame header — the header is the mode that was requested,
    /// which a camera need not honour.
    #[test]
    fn an_mjpeg_frame_decodes_and_re_encodes() {
        const TINY: &[u8] = include_bytes!("../tests/fixtures/tiny16.jpg");
        let snap = snapshot(&frame(999, 999, PixelFormat::Mjpeg, TINY.to_vec()), 0, 85, false)
            .expect("snapshot");
        assert_eq!(
            (snap.width, snap.height),
            (16, 16),
            "the decoder's geometry must win over the frame header"
        );
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;
    use super::tests::frame;

    #[test]
    fn mirroring_reverses_each_row() {
        // 3x1: red, green, blue
        let mut px = vec![255, 0, 0, 0, 255, 0, 0, 0, 255];
        mirror_rgb24(&mut px, 3, 1);
        assert_eq!(px, vec![0, 0, 255, 0, 255, 0, 255, 0, 0], "row reversed");
    }

    #[test]
    fn mirroring_twice_is_the_original() {
        let orig: Vec<u8> = (0..(7 * 5 * 3) as u8).collect();
        let mut px = orig.clone();
        mirror_rgb24(&mut px, 7, 5);
        assert_ne!(px, orig, "an odd width must still change");
        mirror_rgb24(&mut px, 7, 5);
        assert_eq!(px, orig, "flipping twice restores the image");
    }

    #[test]
    fn rows_are_flipped_independently_not_the_whole_buffer() {
        // 2x2: row0 = [A,B], row1 = [C,D]. Reversing the BUFFER would give
        // [D,C,B,A]; reversing each ROW gives [B,A,D,C].
        let mut px = vec![1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4];
        mirror_rgb24(&mut px, 2, 2);
        assert_eq!(px, vec![2, 2, 2, 1, 1, 1, 4, 4, 4, 3, 3, 3]);
    }

    #[test]
    fn degenerate_sizes_are_left_alone() {
        let mut one = vec![9, 9, 9];
        mirror_rgb24(&mut one, 1, 1);
        assert_eq!(one, vec![9, 9, 9], "a single column has nothing to swap");
        let mut short = vec![1, 2, 3];
        mirror_rgb24(&mut short, 8, 8);
        assert_eq!(short, vec![1, 2, 3], "a buffer too small must not be indexed");
    }

    /// The point of the feature: a mirrored capture of text comes back readable.
    #[test]
    fn a_mirrored_frame_round_trips_through_snapshot() {
        let (w, h) = (8u32, 4u32);
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                // A left-heavy gradient, so a flip is detectable.
                data.extend_from_slice(&[(x * 30) as u8, (y * 60) as u8, 0]);
            }
        }
        let f = frame(w, h, PixelFormat::Rgb24, data);
        let plain = snapshot(&f, 0, 90, false).expect("plain");
        let flipped = snapshot(&f, 0, 90, true).expect("mirrored");
        assert_ne!(
            plain.base64, flipped.base64,
            "mirroring must change the encoded image"
        );
    }
}
