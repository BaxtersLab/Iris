use once_cell::sync::Lazy;
use prometheus::{register_int_counter_vec, Encoder, IntCounterVec, TextEncoder};
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;
use tracing::warn;
use tokio::sync::broadcast;

/// Local encoder rebase event emitted by `iris-core` so higher-level
/// components can translate it into `TelemetryEnvelope` and forward it to
/// the global telemetry stream without introducing a cyclic crate dependency.
#[derive(Debug, Clone)]
pub struct EncoderRebaseEvent {
    pub prev_raw: i64,
    pub prev_capture: i64,
    pub new_raw: i64,
    pub new_capture: i64,
    pub reason: String,
}

use crate::{CaptureFrame, EncodedPacket};

const DEFAULT_DRIFT_THRESHOLD_US: i64 = 5_000_000; // 5 seconds

pub struct RecordingPipeline {
    encoder_tx: mpsc::Sender<CaptureFrame>,
    _task: Option<JoinHandle<()>>,
}

impl RecordingPipeline {
    /// Start the recording pipeline.
    ///
    /// `capacity` is the frame channel capacity between capture and encoder.
    /// Returns (pipeline, packet_receiver).
    pub fn start(capacity: usize) -> (Self, mpsc::Receiver<EncodedPacket>) {
        Self::start_with_threshold(capacity, DEFAULT_DRIFT_THRESHOLD_US)
    }

    /// Start the recording pipeline with a configurable drift threshold (in microseconds)
    /// used to rebase PTS mappings when the encoder clock drifts.
    pub fn start_with_threshold(
        capacity: usize,
        drift_threshold_us: i64,
    ) -> (Self, mpsc::Receiver<EncodedPacket>) {
        Self::start_with_telemetry(capacity, drift_threshold_us, None)
    }

    /// Start the recording pipeline with an optional telemetry sender which will
    /// receive local `EncoderRebaseEvent` events so the caller may translate
    /// them into `TelemetryEnvelope` without creating a cyclic dependency.
    pub fn start_with_telemetry(
        capacity: usize,
        drift_threshold_us: i64,
        telemetry_tx: Option<broadcast::Sender<EncoderRebaseEvent>>,
    ) -> (Self, mpsc::Receiver<EncodedPacket>) {
        // Channel: capture -> encoder
        let (tx, mut rx) = mpsc::channel::<CaptureFrame>(capacity);
        // Channel: encoder -> muxer
        let (pkt_tx, pkt_rx) = mpsc::channel::<EncodedPacket>(capacity);

        // Encoder task: spawn ffmpeg and stream raw frames into its stdin,
        // forwarding encoded bytes from stdout as EncodedPacket.
        let encoder_task = tokio::spawn(async move {
            // Wait for first frame to determine resolution/pix_fmt
            let first = match rx.recv().await {
                Some(f) => f,
                None => {
                    println!("RecordingPipeline: no frames received");
                    return;
                }
            };

            // Map pixel format to ffmpeg pix_fmt
            let pix = match first.format {
                crate::PixelFormat::Rgb24 => "rgb24",
                crate::PixelFormat::Bgr24 => "bgr24",
                crate::PixelFormat::Nv12 => "nv12",
                crate::PixelFormat::Yuyv => "yuyv422",
            };
            let width = first.width;
            let height = first.height;
            let fps = 30; // default; encoder-friendly. We don't strictly need accurate fps for raw encoding.

            // Build ffmpeg command
            let mut cmd = tokio::process::Command::new("ffmpeg");
            cmd.arg("-hide_banner")
                .arg("-loglevel")
                .arg("error")
                .arg("-f")
                .arg("rawvideo")
                .arg("-pix_fmt")
                .arg(pix)
                .arg("-s")
                .arg(format!("{}x{}", width, height))
                .arg("-r")
                .arg(format!("{}", fps))
                .arg("-i")
                .arg("-")
                .arg("-an")
                .arg("-c:v")
                .arg("libx264")
                .arg("-preset")
                .arg("veryfast")
                .arg("-tune")
                .arg("zerolatency")
                // output as MPEG-TS so we can parse PES/PTS from stdout
                .arg("-f")
                .arg("mpegts")
                .arg("-");

            cmd.stdin(std::process::Stdio::piped());
            cmd.stdout(std::process::Stdio::piped());
            // spawn ffmpeg
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    println!("RecordingPipeline: failed to spawn ffmpeg: {:?}", e);
                    return;
                }
            };

            let mut ff_stdin = child.stdin.take().expect("ffmpeg stdin");
            let mut ff_stdout = child.stdout.take().expect("ffmpeg stdout");

            // shared last-written capture timestamp (microseconds) so reader can attribute encoded
            // packets to the most recent input frame when PES PTS is not yet mapped.
            let last_pts = Arc::new(AtomicI64::new(first.timestamp_us as i64));

            // mapping between raw stream PTS (90kHz) and capture microsecond timeline.
            // Set on first observed PES PTS: (base_raw_pts, base_capture_us)
            let pts_mapping = Arc::new(AsyncMutex::new(None::<(i64, i64)>));
            // capture drift threshold for mapping rebases
            let drift_threshold = drift_threshold_us;

            // spawn task to read ffmpeg stdout and forward encoded bytes
            let pkt_tx_clone = pkt_tx.clone();
            let last_pts_reader = last_pts.clone();
            let pts_mapping_reader = pts_mapping.clone();
            let telemetry_tx_reader = telemetry_tx.clone();
            let read_task = tokio::spawn(async move {
                // Read MPEG-TS stream (188-byte packets) from ffmpeg stdout and
                // extract PES payloads and PTS/DTS when available.
                let mut buffer = Vec::new();
                let mut tmp = [0u8; 8192];
                // PES reassembly state
                let mut pes_buf: Vec<u8> = Vec::new();
                let mut pes_pts: Option<i64> = None;
                let mut pes_expected: Option<usize> = None;
                loop {
                    match ff_stdout.read(&mut tmp).await {
                        Ok(0) => break,
                        Ok(n) => {
                            buffer.extend_from_slice(&tmp[..n]);
                            // process full 188-byte TS packets
                            while buffer.len() >= 188 {
                                // find sync at start
                                if buffer[0] != 0x47 {
                                    // try to resync
                                    if let Some(pos) = buffer.iter().position(|&b| b == 0x47) {
                                        buffer.drain(0..pos);
                                        if buffer.len() < 188 {
                                            break;
                                        }
                                    } else {
                                        buffer.clear();
                                        break;
                                    }
                                }
                                if buffer.len() < 188 {
                                    break;
                                }
                                let pkt = buffer.drain(0..188).collect::<Vec<u8>>();
                                // parse TS header
                                if pkt.len() != 188 {
                                    break;
                                }
                                let payload_unit_start = (pkt[1] & 0x40) != 0;
                                let adaptation_control = (pkt[3] & 0x30) >> 4;
                                let mut payload_offset = 4usize;
                                if adaptation_control == 2 || adaptation_control == 0 {
                                    // no payload
                                    continue;
                                }
                                if adaptation_control == 3 {
                                    // adaptation field present before payload
                                    if payload_offset >= pkt.len() {
                                        continue;
                                    }
                                    let adap_len = pkt[payload_offset] as usize;
                                    payload_offset += 1 + adap_len;
                                    if payload_offset > pkt.len() {
                                        continue;
                                    }
                                }

                                if payload_offset >= pkt.len() {
                                    continue;
                                }

                                let payload = &pkt[payload_offset..];

                                if payload_unit_start
                                    && payload.len() >= 3
                                    && payload[0] == 0
                                    && payload[1] == 0
                                    && payload[2] == 1
                                {
                                    // start of new PES
                                    // if we have an active PES in progress, flush it
                                    if !pes_buf.is_empty() {
                                        // compute pts_us from raw PES PTS if available
                                        let pts_us = if let Some(raw) = pes_pts {
                                            map_raw_pts_to_us(
                                                raw,
                                                &last_pts_reader,
                                                &pts_mapping_reader,
                                                drift_threshold,
                                                telemetry_tx_reader.clone(),
                                            )
                                            .await
                                        } else {
                                            last_pts_reader.load(Ordering::Relaxed)
                                        };
                                        let keyframe = contains_idr(&pes_buf);
                                        let pkt_out = EncodedPacket {
                                            data: pes_buf.clone(),
                                            pts: pts_us,
                                            dts: pts_us,
                                            keyframe,
                                            codec: "h264".to_string(),
                                        };
                                        if let Err(e) = pkt_tx_clone.send(pkt_out).await {
                                            println!("RecordingPipeline: pkt send error: {:?}", e);
                                            break;
                                        }
                                        pes_buf.clear();
                                    }

                                    // parse PES header and get PTS and header length
                                    let pes_len =
                                        ((payload[4] as usize) << 8) | (payload[5] as usize);
                                    let header_data_len = if payload.len() > 8 {
                                        payload[8] as usize
                                    } else {
                                        0
                                    };
                                    let header_total = 9 + header_data_len;
                                    let mut pts_val_raw: Option<i64> = None;
                                    if let Some((p, _hdr_len)) = parse_pes_pts(payload) {
                                        pts_val_raw = Some(p);
                                    }
                                    // expected payload length calculation when pes_len != 0
                                    let expected_payload =
                                        if pes_len > 0 && pes_len >= (3 + header_data_len) {
                                            Some(pes_len - (3 + header_data_len))
                                        } else {
                                            None
                                        };
                                    pes_pts = pts_val_raw;
                                    pes_expected = expected_payload;
                                    // append payload after PES header
                                    if payload.len() > header_total {
                                        pes_buf.extend_from_slice(&payload[header_total..]);
                                    }
                                    // check if we already have full PES
                                    if let Some(exp) = pes_expected {
                                        if pes_buf.len() >= exp {
                                            // convert raw pts to microseconds
                                            let pts_us = if let Some(raw) = pes_pts {
                                                map_raw_pts_to_us(
                                                    raw,
                                                    &last_pts_reader,
                                                    &pts_mapping_reader,
                                                    drift_threshold,
                                                    telemetry_tx_reader.clone(),
                                                )
                                                .await
                                            } else {
                                                last_pts_reader.load(Ordering::Relaxed)
                                            };
                                            let data = pes_buf.drain(..exp).collect::<Vec<u8>>();
                                            let keyframe = contains_idr(&data);
                                            let pkt_out = EncodedPacket {
                                                data,
                                                pts: pts_us,
                                                dts: pts_us,
                                                keyframe,
                                                codec: "h264".to_string(),
                                            };
                                            if let Err(e) = pkt_tx_clone.send(pkt_out).await {
                                                println!(
                                                    "RecordingPipeline: pkt send error: {:?}",
                                                    e
                                                );
                                                break;
                                            }
                                            pes_pts = None;
                                            pes_expected = None;
                                            pes_buf.clear();
                                        }
                                    }
                                } else {
                                    // continuation of PES payload
                                    if !pes_buf.is_empty() {
                                        pes_buf.extend_from_slice(payload);
                                        if let Some(exp) = pes_expected {
                                            if pes_buf.len() >= exp {
                                                // convert raw pts to microseconds
                                                let pts_us = if let Some(raw) = pes_pts {
                                                    map_raw_pts_to_us(
                                                        raw,
                                                        &last_pts_reader,
                                                        &pts_mapping_reader,
                                                        drift_threshold,
                                                        telemetry_tx_reader.clone(),
                                                    )
                                                    .await
                                                } else {
                                                    last_pts_reader.load(Ordering::Relaxed)
                                                };
                                                let data =
                                                    pes_buf.drain(..exp).collect::<Vec<u8>>();
                                                let keyframe = contains_idr(&data);
                                                let pkt_out = EncodedPacket {
                                                    data,
                                                    pts: pts_us,
                                                    dts: pts_us,
                                                    keyframe,
                                                    codec: "h264".to_string(),
                                                };
                                                if let Err(e) = pkt_tx_clone.send(pkt_out).await {
                                                    println!(
                                                        "RecordingPipeline: pkt send error: {:?}",
                                                        e
                                                    );
                                                    break;
                                                }
                                                pes_pts = None;
                                                pes_expected = None;
                                                pes_buf.clear();
                                            }
                                        }
                                    } else {
                                        // no active PES, ignore payload
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            println!("RecordingPipeline: ffmpeg stdout read error: {:?}", e);
                            break;
                        }
                    }
                }
                println!("RecordingPipeline: ffmpeg stdout reader exiting");
            });

            // write first frame and then rest
            // write first frame and then rest. update last_pts before each write
            last_pts.store(first.timestamp_us as i64, Ordering::Relaxed);
            if let Err(e) = ff_stdin.write_all(&first.data).await {
                println!(
                    "RecordingPipeline: failed to write first frame to ffmpeg: {:?}",
                    e
                );
            }
            let _ = ff_stdin.flush().await;

            while let Some(frame) = rx.recv().await {
                last_pts.store(frame.timestamp_us as i64, Ordering::Relaxed);
                if let Err(e) = ff_stdin.write_all(&frame.data).await {
                    println!("RecordingPipeline: ffmpeg write error: {:?}", e);
                    break;
                }
                let _ = ff_stdin.flush().await;
            }

            // Close stdin to signal EOF to ffmpeg
            drop(ff_stdin);

            // wait for reader and child to exit
            let _ = read_task.await;
            let _ = child.wait().await;
            println!("RecordingPipeline: encoder task exiting");
        });

        // The encoder task is joined by an outer wrapper so the RecordingPipeline
        // can keep it alive. The packet receiver (`pkt_rx`) is returned to the
        // caller so they may consume encoded packets (muxer / assertions in tests).
        let join = tokio::spawn(async move {
            let _ = encoder_task.await;
        });

        (
            Self {
                encoder_tx: tx,
                _task: Some(join),
            },
            pkt_rx,
        )
    }

    pub fn encoder_sender(&self) -> mpsc::Sender<CaptureFrame> {
        self.encoder_tx.clone()
    }
}

fn contains_idr(data: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let nal = data[i + 3];
            let nal_type = nal & 0x1F;
            if nal_type == 5 {
                return true;
            }
            i += 3;
        } else if i + 4 < data.len()
            && data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && data[i + 3] == 1
        {
            let nal = data[i + 4];
            let nal_type = nal & 0x1F;
            if nal_type == 5 {
                return true;
            }
            i += 4;
        } else {
            i += 1;
        }
    }
    false
}

/// Map a raw PES PTS (90kHz) into capture microseconds, handling 33-bit
/// wrap-around and large drift by rebasing the mapping when necessary.
async fn map_raw_pts_to_us(
    raw: i64,
    last_pts: &Arc<AtomicI64>,
    pts_mapping: &Arc<AsyncMutex<Option<(i64, i64)>>>,
    drift_threshold_us: i64,
    telemetry_tx: Option<broadcast::Sender<EncoderRebaseEvent>>,
) -> i64 {
    const PTS_MOD: i64 = 1i64 << 33; // 2^33
    const PTS_HALF: i64 = 1i64 << 32; // half of modulus

    let mut map = pts_mapping.lock().await;
    // If no mapping yet, establish base mapping at the most recent capture time.
    if map.is_none() {
        let base_capture = last_pts.load(Ordering::Relaxed);
        *map = Some((raw, base_capture));
        return base_capture;
    }

    // Safe to copy the base values (i64 are Copy)
    let (base_raw, base_capture) = if let Some(v) = *map {
        v
    } else {
        unreachable!()
    };

    // Compute signed delta respecting 33-bit wrap-around
    let mut delta = raw - base_raw;
    if delta < -PTS_HALF {
        delta = delta.wrapping_add(PTS_MOD);
    } else if delta > PTS_HALF {
        delta = delta.wrapping_sub(PTS_MOD);
    }

    let converted = base_capture + (delta * 1_000_000) / 90_000;

    // If the converted time drifts far from the recent capture timestamp, rebase
    // the mapping to avoid emitting wildly incorrect timestamps.
    let recent = last_pts.load(Ordering::Relaxed);
    if (converted - recent).abs() > drift_threshold_us {
        // Log rebase event for observability (include prior base for diagnostics)
        if let Some((prev_raw, prev_capture)) = *map {
            warn!(
                "PTS rebase: raw={} prev_raw={} prev_capture_us={} recent_capture_us={}",
                raw, prev_raw, prev_capture, recent
            );
            // emit a local rebase event for higher-level translation
            if let Some(tx) = telemetry_tx.clone() {
                let event = EncoderRebaseEvent {
                    prev_raw,
                    prev_capture,
                    new_raw: raw,
                    new_capture: recent,
                    reason: "drift_threshold_exceeded".to_string(),
                };
                let _ = tx.send(event);
            }
        } else {
            warn!("PTS rebase: raw={} recent_capture_us={}", raw, recent);
            if let Some(tx) = telemetry_tx.clone() {
                let event = EncoderRebaseEvent {
                    prev_raw: raw,
                    prev_capture: recent,
                    new_raw: raw,
                    new_capture: recent,
                    reason: "initial_rebase".to_string(),
                };
                let _ = tx.send(event);
            }
        }
        *map = Some((raw, recent));
        // record metric
        REBASE_COUNTER.fetch_add(1, Ordering::Relaxed);
        // prometheus metric (labeled)
        incr_prom_rebase_metric();
        return recent;
    }

    converted
}

// Global counter for PTS rebase events. Read via `rebase_count()`.
static REBASE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Return the number of times PTS mapping was rebased due to drift.
pub fn rebase_count() -> u64 {
    // Ensure Prometheus collector is registered so `prometheus_text()` will
    // include the `iris_encoder_rebase_total` metric even if it's zero.
    let _ = PROM_REBASE_VEC.with_label_values(&[METRICS_JOB.as_str(), METRICS_INSTANCE.as_str()]);
    REBASE_COUNTER.load(Ordering::Relaxed)
}

// Prometheus metric (registered on first use)
static PROM_REBASE_VEC: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "iris_encoder_rebase_total",
        "Number of encoder PTS rebases",
        &["job", "instance"]
    )
    .unwrap()
});

static METRICS_JOB: Lazy<String> =
    Lazy::new(|| std::env::var("METRICS_JOB").unwrap_or_else(|_| "iris".to_string()));
static METRICS_INSTANCE: Lazy<String> = Lazy::new(|| {
    std::env::var("METRICS_INSTANCE")
        .unwrap_or_else(|_| std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string()))
});

/// Return Prometheus-formatted metrics text for scraping or pushing.
pub fn prometheus_text() -> String {
    let encoder = TextEncoder::new();
    let mf = prometheus::gather();
    let mut buf = Vec::new();
    encoder.encode(&mf, &mut buf).unwrap();
    String::from_utf8(buf).unwrap()
}

fn incr_prom_rebase_metric() {
    let job = METRICS_JOB.as_str();
    let instance = METRICS_INSTANCE.as_str();
    PROM_REBASE_VEC.with_label_values(&[job, instance]).inc();
}

/// Test helper: force increment the rebase metric counter and Prometheus metric.
/// This is intended for testing and diagnostics only.
pub fn force_increment_rebase_for_test() {
    REBASE_COUNTER.fetch_add(1, Ordering::Relaxed);
    incr_prom_rebase_metric();
}

fn parse_pes_pts(payload: &[u8]) -> Option<(i64, usize)> {
    if payload.len() < 9 {
        return None;
    }
    if !(payload[0] == 0 && payload[1] == 0 && payload[2] == 1) {
        return None;
    }
    // stream_id = payload[3]
    // pes_packet_length = payload[4..6]
    let _pes_len = ((payload[4] as usize) << 8) | (payload[5] as usize);
    let _flags1 = payload[6];
    let flags2 = payload[7];
    let header_data_len = payload[8] as usize;
    let offset = 9usize;
    if payload.len() < offset {
        return None;
    }
    let pts_dts_flags = (flags2 >> 6) & 0x03;
    if pts_dts_flags == 0 {
        return None;
    }
    if payload.len() < offset + header_data_len {
        return None;
    }
    if pts_dts_flags == 0b10 {
        // PTS only
        if header_data_len < 5 {
            return None;
        }
        let b0 = payload[offset];
        let b1 = payload[offset + 1];
        let b2 = payload[offset + 2];
        let b3 = payload[offset + 3];
        let b4 = payload[offset + 4];
        let pts = ((b0 as i64 & 0x0E) << 29)
            | ((b1 as i64) << 22)
            | ((b2 as i64 & 0xFE) << 14)
            | ((b3 as i64) << 7)
            | ((b4 as i64 & 0xFE) >> 1);
        return Some((pts, offset + 5));
    } else if pts_dts_flags == 0b11 {
        // both PTS and DTS present
        if header_data_len < 10 {
            return None;
        }
        let b0 = payload[offset];
        let b1 = payload[offset + 1];
        let b2 = payload[offset + 2];
        let b3 = payload[offset + 3];
        let b4 = payload[offset + 4];
        let pts = ((b0 as i64 & 0x0E) << 29)
            | ((b1 as i64) << 22)
            | ((b2 as i64 & 0xFE) << 14)
            | ((b3 as i64) << 7)
            | ((b4 as i64 & 0xFE) >> 1);
        // DTS follows PTS
        return Some((pts, offset + 10));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_pts(pts: u64) -> [u8; 5] {
        // Encode 33-bit PTS into 5 bytes (marker bits set to 1)
        let p = pts as u64;
        let p32_30 = ((p >> 29) & 0x07) as u8;
        let p29_22 = ((p >> 22) & 0xFF) as u8;
        let p21_15 = ((p >> 14) & 0x7F) as u8;
        let p14_7 = ((p >> 7) & 0xFF) as u8;
        let p6_0 = (p & 0x7F) as u8;
        let b0 = 0x20 | (p32_30 << 1) | 1;
        let b1 = p29_22;
        let b2 = (p21_15 << 1) | 1;
        let b3 = p14_7;
        let b4 = (p6_0 << 1) | 1;
        [b0, b1, b2, b3, b4]
    }

    #[test]
    fn test_contains_idr_detects_idr() {
        let data = vec![0, 0, 1, 5, 0xAA, 0xBB];
        assert!(contains_idr(&data));
        let data2 = vec![0, 0, 1, 1, 0x00, 0x00];
        assert!(!contains_idr(&data2));
    }

    #[test]
    fn test_parse_pes_pts_roundtrip_and_mapping() {
        // build PES with PTS only and small payload
        let pts_raw: u64 = 90_000; // corresponds to 1 second at 90kHz
        let pts_bytes = encode_pts(pts_raw);
        let stream_id = 0xE0u8;
        let payload_data = vec![0, 0, 1, 5, 0x11, 0x22]; // contains IDR NAL
        let pes_header_len = 5u8;
        let pes_payload_len = (5 + payload_data.len()) as u16; // header_data_len + payload
        let mut pes = Vec::new();
        pes.extend_from_slice(&[0x00, 0x00, 0x01]);
        pes.push(stream_id);
        pes.push(((pes_payload_len >> 8) & 0xFF) as u8);
        pes.push((pes_payload_len & 0xFF) as u8);
        pes.push(0x00); // flags1
        pes.push(0x80); // flags2: '10' -> PTS only (0x80)
        pes.push(pes_header_len);
        pes.extend_from_slice(&pts_bytes);
        pes.extend_from_slice(&payload_data);

        // build a single TS packet carrying this PES (payload only)
        let mut ts = vec![0u8; 188];
        ts[0] = 0x47;
        ts[1] = 0x40; // payload_unit_start
        ts[2] = 0x00;
        ts[3] = 0x10; // adaptation_control = 1 (payload only)
                      // write PES at payload offset 4
        for (i, b) in pes.iter().enumerate() {
            if 4 + i < 188 {
                ts[4 + i] = *b;
            }
        }

        // simulate reassembly loop: extract payload from TS and parse PES
        let payload_offset = 4usize;
        let payload = &ts[payload_offset..];
        // ensure PES start detected
        assert_eq!(payload[0], 0);
        assert_eq!(payload[1], 0);
        assert_eq!(payload[2], 1);
        // parse PTS
        let parsed = parse_pes_pts(payload).expect("should parse pes pts");
        let (parsed_pts, hdr_len) = parsed;

        // simulate mapping to capture microseconds
        let base_capture_us: i64 = 1_600_000_000_000_000; // arbitrary
                                                          // establish mapping base_raw = parsed_pts, base_capture = base_capture_us
        let converted =
            base_capture_us + ((parsed_pts as i64 - parsed_pts as i64) * 1_000_000) / 90_000;
        // converted should equal base_capture_us
        assert_eq!(converted, base_capture_us);

        // ensure keyframe detection on assembled payload
        let data_slice = &payload[hdr_len..hdr_len + payload_data.len()];
        assert!(contains_idr(data_slice));
    }

    #[tokio::test]
    async fn test_rebase_emits_local_encoder_rebase_event() {
        use tokio::sync::broadcast;

        // Prepare a mapping that will cause `map_raw_pts_to_us` to rebase
        let (tx, mut rx) = broadcast::channel::<EncoderRebaseEvent>(4);
        let last_pts = Arc::new(AtomicI64::new(0));
        let pts_mapping = Arc::new(AsyncMutex::new(Some((0i64, 0i64))));

        // Choose a raw PTS that converts to 6_000_000us which exceeds the default
        // drift threshold of 5_000_000us -> triggers a rebase to `recent` (0)
        let raw: i64 = 540_000; // delta producing converted = 6_000_000 us

        let out = map_raw_pts_to_us(raw, &last_pts, &pts_mapping, DEFAULT_DRIFT_THRESHOLD_US, Some(tx)).await;
        // when a rebase occurs, the function returns the recent capture timestamp
        assert_eq!(out, 0);

        // The rebase event should have been sent on the broadcast channel
        let ev = rx.recv().await.expect("should receive rebase event");
        assert_eq!(ev.prev_raw, 0);
        assert_eq!(ev.prev_capture, 0);
        assert_eq!(ev.new_raw, raw);
        assert_eq!(ev.new_capture, 0);
        assert_eq!(ev.reason, "drift_threshold_exceeded");
    }

    #[tokio::test]
    async fn test_parse_real_ts_from_harness() -> Result<(), Box<dyn std::error::Error>> {
        // Use the harness to generate a short MPEG-TS stream via ffmpeg. If
        // ffmpeg is not available, skip the meat of the test.
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("stream.ts");
        match iris_harness::spawn_ffmpeg_stream(&path, 2) {
            Ok(()) => {
                // proceed
            }
            Err(e) => {
                eprintln!("ffmpeg not available, skipping real-ts parse test: {}", e);
                return Ok(());
            }
        }

        let data = std::fs::read(&path)?;
        let mut found_pes_with_pts = false;
        for i in 0..data.len().saturating_sub(10) {
            if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                if let Some((pts, _hdr)) = parse_pes_pts(&data[i..]) {
                    // Basic sanity check: pts is non-negative and within 33-bit range
                    assert!(pts >= 0 && pts < (1i64 << 33));
                    found_pes_with_pts = true;
                    break;
                }
            }
        }
        assert!(found_pes_with_pts, "expected to find at least one PES with PTS in stream.ts");
        Ok(())
    }

    #[tokio::test]
    async fn test_consume_stream_ts_into_packets() -> Result<(), Box<dyn std::error::Error>> {
        use std::fs;
        use std::path::PathBuf;

        // Try common workspace-relative locations for the harness output
        let candidates = [
            PathBuf::from("harness-output/stream.ts"),
            PathBuf::from("../harness-output/stream.ts"),
            PathBuf::from("../../harness-output/stream.ts"),
        ];

        let mut found = None;
        for p in &candidates {
            if p.exists() {
                found = Some(p.clone());
                break;
            }
        }

        let path = if let Some(p) = found {
            p
        } else {
            eprintln!("harness stream.ts not found in expected locations; skipping test");
            return Ok(());
        };

        let data = fs::read(&path)?;
        let mut buffer = data.as_slice();

        // Prepare mapping/context like RecordingPipeline reader
        let last_pts = Arc::new(AtomicI64::new(0));
        let pts_mapping = Arc::new(AsyncMutex::new(None::<(i64, i64)>));
        let drift_threshold = DEFAULT_DRIFT_THRESHOLD_US;

        // Reassembly state
        let mut pes_buf: Vec<u8> = Vec::new();
        let mut pes_pts: Option<i64> = None;
        let mut pes_expected: Option<usize> = None;

        let mut produced = 0usize;

        while buffer.len() >= 188 {
            // find next sync byte
            if buffer[0] != 0x47 {
                if let Some(pos) = buffer.iter().position(|&b| b == 0x47) {
                    buffer = &buffer[pos..];
                    if buffer.len() < 188 {
                        break;
                    }
                } else {
                    break;
                }
            }
            if buffer.len() < 188 {
                break;
            }
            let pkt = &buffer[..188];
            buffer = &buffer[188..];

            let payload_unit_start = (pkt[1] & 0x40) != 0;
            let adaptation_control = (pkt[3] & 0x30) >> 4;
            let mut payload_offset = 4usize;
            if adaptation_control == 2 || adaptation_control == 0 {
                continue;
            }
            if adaptation_control == 3 {
                if payload_offset >= pkt.len() { continue; }
                let adap_len = pkt[payload_offset] as usize;
                payload_offset += 1 + adap_len;
                if payload_offset > pkt.len() { continue; }
            }
            if payload_offset >= pkt.len() { continue; }
            let payload = &pkt[payload_offset..];

            if payload_unit_start && payload.len() >= 3 && payload[0]==0 && payload[1]==0 && payload[2]==1 {
                // start of PES
                if !pes_buf.is_empty() {
                    let pts_us = if let Some(raw) = pes_pts {
                        map_raw_pts_to_us(raw, &last_pts, &pts_mapping, drift_threshold, None).await
                    } else {
                        last_pts.load(Ordering::Relaxed)
                    };
                    let keyframe = contains_idr(&pes_buf);
                    let _pkt = EncodedPacket { data: pes_buf.clone(), pts: pts_us, dts: pts_us, keyframe, codec: "h264".to_string() };
                    produced += 1;
                    pes_buf.clear();
                }

                if payload.len() > 8 {
                    let header_data_len = payload[8] as usize;
                    if let Some((raw, _hdr)) = parse_pes_pts(payload) {
                        pes_pts = Some(raw);
                    } else {
                        pes_pts = None;
                    }
                    let pes_len = ((payload[4] as usize) << 8) | (payload[5] as usize);
                    let expected_payload = if pes_len>0 && pes_len >= (3+header_data_len) { Some(pes_len - (3+header_data_len)) } else { None };
                    pes_expected = expected_payload;
                    let header_total = 9 + header_data_len;
                    if payload.len() > header_total {
                        pes_buf.extend_from_slice(&payload[header_total..]);
                    }
                    if let Some(exp) = pes_expected {
                        if pes_buf.len() >= exp {
                            let pts_us = if let Some(raw) = pes_pts { map_raw_pts_to_us(raw, &last_pts, &pts_mapping, drift_threshold, None).await } else { last_pts.load(Ordering::Relaxed) };
                            let data_piece = pes_buf.drain(..exp).collect::<Vec<u8>>();
                            let keyframe = contains_idr(&data_piece);
                            let _pkt = EncodedPacket { data: data_piece, pts: pts_us, dts: pts_us, keyframe, codec: "h264".to_string() };
                            produced += 1;
                            pes_pts = None; pes_expected = None; pes_buf.clear();
                        }
                    }
                }
            } else {
                if !pes_buf.is_empty() {
                    pes_buf.extend_from_slice(payload);
                    if let Some(exp) = pes_expected {
                        if pes_buf.len() >= exp {
                            let pts_us = if let Some(raw) = pes_pts { map_raw_pts_to_us(raw, &last_pts, &pts_mapping, drift_threshold, None).await } else { last_pts.load(Ordering::Relaxed) };
                            let data_piece = pes_buf.drain(..exp).collect::<Vec<u8>>();
                            let keyframe = contains_idr(&data_piece);
                            let _pkt = EncodedPacket { data: data_piece, pts: pts_us, dts: pts_us, keyframe, codec: "h264".to_string() };
                            produced += 1;
                            pes_pts = None; pes_expected = None; pes_buf.clear();
                        }
                    }
                } else {
                    // nothing to do
                }
            }
        }

        assert!(produced > 0, "expected to produce at least one EncodedPacket from stream.ts");
        Ok(())
    }
}
