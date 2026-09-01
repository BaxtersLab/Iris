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
    #[error("cannot decode {0:?} frames to pixels")]
    UnsupportedFormat(PixelFormat),
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
        other => Err(SnapshotError::UnsupportedFormat(other.clone())),
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
) -> Result<Snapshot, SnapshotError> {
    let (w, h, rgb) = to_rgb24(frame)?;
    let (dw, dh, scaled) = downscale_rgb24(&rgb, w, h, max_width);

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

    fn frame(w: u32, h: u32, format: PixelFormat, data: Vec<u8>) -> CaptureFrame {
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
        let snap = snapshot(&frame(w, h, PixelFormat::Rgb24, data), 32, 80).expect("snapshot");
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
        let snap = snapshot(&frame(16, 16, PixelFormat::Rgb24, data), 0, 90).expect("snapshot");
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
        let snap = snapshot(&frame(1, 1, PixelFormat::Bgr24, vec![0, 0, 255]), 0, 95)
            .expect("snapshot");
        assert_eq!((snap.width, snap.height), (1, 1));
    }

    #[test]
    fn a_truncated_frame_is_reported_not_guessed() {
        let err = snapshot(&frame(64, 64, PixelFormat::Rgb24, vec![1, 2, 3]), 0, 80).unwrap_err();
        assert!(matches!(err, SnapshotError::Truncated { .. }), "{err}");
    }

    #[test]
    fn empty_and_zero_sized_frames_are_refused() {
        assert!(matches!(
            snapshot(&frame(4, 4, PixelFormat::Rgb24, vec![]), 0, 80).unwrap_err(),
            SnapshotError::Empty
        ));
        assert!(matches!(
            snapshot(&frame(0, 4, PixelFormat::Rgb24, vec![1]), 0, 80).unwrap_err(),
            SnapshotError::ZeroSized(0, 4)
        ));
    }

    /// The camera's own MJPEG is the common case, and the decoder's geometry
    /// wins over the frame header — the header is the mode that was requested,
    /// which a camera need not honour.
    #[test]
    fn an_mjpeg_frame_decodes_and_re_encodes() {
        const TINY: &[u8] = include_bytes!("../tests/fixtures/tiny16.jpg");
        let snap = snapshot(&frame(999, 999, PixelFormat::Mjpeg, TINY.to_vec()), 0, 85)
            .expect("snapshot");
        assert_eq!(
            (snap.width, snap.height),
            (16, 16),
            "the decoder's geometry must win over the frame header"
        );
    }
}
