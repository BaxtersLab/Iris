//! MJPEG decode for the paths that need a pixel grid.
//!
//! An MJPEG stream is one baseline JPEG per frame, so "decode" here is a single
//! `zune-jpeg` pass with no inter-frame state.
//!
//! This lives in `iris-capture` rather than in the HAL on purpose. `read_frame`
//! returns exactly what the driver delivered, so consumers that want the
//! compressed stream — recording, forwarding over IPC — still get it untouched.
//! Only the two consumers that genuinely need pixels pay the decode cost: the
//! UI preview, and ROI cropping (a compressed frame has no pixel grid to
//! slice, so cropping it requires decoding first).

use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// A decoded frame: tightly packed RGB24, three bytes per pixel, no stride
/// padding. `rgb24.len()` is always exactly `width * height * 3` — this is
/// checked on construction rather than assumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub rgb24: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MjpegError {
    /// The decoder rejected the byte stream.
    Decode(String),
    /// It decoded, but the reported dimensions and the buffer length disagree.
    /// Treated as an error rather than trusted, because every downstream
    /// consumer indexes the buffer using those dimensions.
    Inconsistent(String),
}

impl std::fmt::Display for MjpegError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MjpegError::Decode(m) => write!(f, "MJPEG decode failed: {m}"),
            MjpegError::Inconsistent(m) => write!(f, "MJPEG decode inconsistent: {m}"),
        }
    }
}

impl std::error::Error for MjpegError {}

/// Decode one MJPEG/JPEG frame to tightly packed RGB24.
pub fn decode_to_rgb24(data: &[u8]) -> Result<DecodedFrame, MjpegError> {
    // `zune-jpeg` is lenient about truncation: given half a JPEG it returns a
    // partially-filled frame and reports success. That is exactly the wrong
    // behaviour here, because a torn frame is a thing this hardware actually
    // produces (see the V4L2 backend's V4L2_BUF_FLAG_ERROR retry). Reject it
    // up front instead.
    //
    // Searching for the EOI marker is reliable rather than heuristic: JPEG
    // byte-stuffing requires a literal 0xFF inside entropy-coded data to be
    // followed by 0x00, so an FF D9 pair cannot occur except as a real EOI.
    if !data.windows(2).any(|w| w == [0xFF, 0xD9]) {
        return Err(MjpegError::Decode(
            "no EOI marker: frame is truncated or not a JPEG".into(),
        ));
    }

    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options);

    let rgb24 = decoder
        .decode()
        .map_err(|e| MjpegError::Decode(e.to_string()))?;

    let info = decoder
        .info()
        .ok_or_else(|| MjpegError::Decode("decoder reported no image info".into()))?;

    let width = info.width as u32;
    let height = info.height as u32;

    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|p| p.checked_mul(3))
        .ok_or_else(|| {
            MjpegError::Inconsistent(format!("{width}x{height} overflows a pixel count"))
        })?;

    if rgb24.len() != expected {
        return Err(MjpegError::Inconsistent(format!(
            "decoder returned {} bytes for {width}x{height} RGB24, expected {expected}",
            rgb24.len()
        )));
    }

    Ok(DecodedFrame {
        width,
        height,
        rgb24,
    })
}

/// Convert tightly packed RGB24 to RGBA8 with a fully opaque alpha channel,
/// which is what the egui texture upload wants.
pub fn rgb24_to_rgba8(rgb24: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb24.len() / 3 * 4);
    for px in rgb24.chunks_exact(3) {
        out.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real 16x16 baseline JPEG produced by ffmpeg's `testsrc` pattern.
    /// Embedded rather than generated at test time so the suite does not depend
    /// on an external tool being present to exercise the decoder.
    const TINY_JPEG: &[u8] = include_bytes!("../tests/fixtures/tiny16.jpg");

    #[test]
    fn decodes_a_real_jpeg_to_the_right_geometry() {
        let f = decode_to_rgb24(TINY_JPEG).expect("fixture must decode");
        assert_eq!((f.width, f.height), (16, 16));
        assert_eq!(f.rgb24.len(), 16 * 16 * 3);
    }

    /// The testsrc pattern is colourful, so a correct decode cannot be a
    /// uniform buffer. This is what catches a decoder that "succeeds" while
    /// returning zeroed or garbage output.
    #[test]
    fn decoded_pixels_are_not_uniform() {
        let f = decode_to_rgb24(TINY_JPEG).expect("fixture must decode");
        let first = f.rgb24[0];
        assert!(
            f.rgb24.iter().any(|&b| b != first),
            "every byte identical — decoder returned a blank buffer"
        );
    }

    #[test]
    fn garbage_is_rejected_not_silently_accepted() {
        assert!(decode_to_rgb24(&[0xFF, 0xD8, 0x00, 0x01, 0x02]).is_err());
        assert!(decode_to_rgb24(&[]).is_err());
        // A valid SOI followed by nothing decodable must not yield a frame.
        assert!(decode_to_rgb24(&[0xFF, 0xD8, 0xFF, 0xD9]).is_err());
    }

    #[test]
    fn truncating_a_valid_jpeg_is_an_error() {
        let half = &TINY_JPEG[..TINY_JPEG.len() / 2];
        assert!(
            decode_to_rgb24(half).is_err(),
            "a truncated JPEG must not decode to a partial frame"
        );
    }

    #[test]
    fn rgb24_to_rgba8_expands_and_sets_opaque_alpha() {
        let rgb = [1u8, 2, 3, 4, 5, 6];
        assert_eq!(
            rgb24_to_rgba8(&rgb),
            vec![1, 2, 3, 0xFF, 4, 5, 6, 0xFF],
            "each pixel gains an opaque alpha byte"
        );
    }
}
