# Iris — ROADMAP (declared stubs and known gaps)

Per Agent Constitution Article VII §2–3: anything not fully implemented is declared here rather
than left silently incomplete. Format:

```
- [ ] STUB: <function/module name> — <what real implementation replaces it> — <reason it was deferred>
```

## Linux (V4L2) — opened 2026-08-01 during the Ubuntu 26.04 intake

- [x] **DONE 2026-08-17 (`23b15eb`)** — ~~STUB: `iris-hal/v4l2_backend.rs` frame-read methods~~.
      Full streaming capture: REQBUFS / QUERYBUF + mmap / QBUF / DQBUF / STREAMON, plus
      G_CTRL / S_CTRL / QUERYCTRL and `current_format`. `grep -c NotImplemented` on that file
      is now 0. Verified on real hardware at 1920x1080 @30fps MJPEG, 6/6 consecutive runs.
      Two findings worth keeping: buffers flagged `V4L2_BUF_FLAG_ERROR` must be discarded and
      retried, because the first frame after STREAMON is often a torn JPEG (valid SOI,
      plausible length, truncated payload); and for the ABI structs **field offsets are the
      invariant, not sizes** — declaring `m_offset` as `u32` keeps `v4l2_buffer` at 88 bytes
      while shifting every later field by 4, so a size assertion stays green.

- [x] **DONE 2026-08-17 (`c8b6f85`)** — ~~STUB: MJPEG preview decode~~. `zune-jpeg` was chosen
      (as this entry suggested); pure Rust, no bindgen, no system libs, so the dependency-light
      rule holds. Decode lives in `iris_capture::mjpeg`, **not** the HAL, so `read_frame` keeps
      returning the untouched compressed stream for recording and IPC and only the pixel
      consumers pay the cost. The safe-fallback behaviour described below is retained for decode
      failures and geometry mismatches.

- [x] **DONE 2026-08-17 (`c8b6f85`)** — ~~STUB: MJPEG ROI cropping~~. `apply_roi` decodes, crops
      via the existing RGB24 path, and sets `format = Rgb24` — the frame genuinely stops being
      compressed, so telemetry says so. On decode failure it falls back to the old behaviour:
      untouched and uncropped, never byte-sliced. `apply_roi` is now `pub(crate)` and has tests;
      it had none before.
      **Caveat carried forward:** `zune-jpeg` reports *success* on a truncated JPEG, returning a
      partially-filled frame. `decode_to_rgb24` therefore requires an EOI marker before decoding.
      Anything else in the estate decoding hardware-sourced JPEG needs its own completeness check.

## Windows (WMF) — duplicate backend, opened 2026-08-17

- [ ] STUB: `iris-hal/wmf_backend.rs` — `WmfUvcBackend` has the same **5 `NotImplemented`**
      methods (`open_device`, `close_device`, `read_frame`, `get_control`, `set_control`) that
      `v4l2_backend.rs` had until `23b15eb`. **This was not previously declared here**, which
      Article VII §2–3 requires.
      **It is not a live capture bug.** There are two WMF implementations and both are in use:
      `iris_hal::backend::WmfBackend` is fully implemented and is what `bootstrap.rs:276` uses
      for actual capture; `wmf_backend.rs`'s `WmfUvcBackend` is the enumeration-only mirror of
      the old V4L2 backend and `bootstrap.rs:74` calls only its `enumerate_sync`. So Windows
      capture works — but the stubs are `pub` and reachable, and the duplication is a trap for
      anyone who picks the wrong one.
      **Resolution is a design decision, not an incidental fix:** either delete
      `wmf_backend.rs` and route enumeration through `backend::WmfBackend`, or implement its
      five methods by delegating to the real one. Needs a Windows box to verify either way —
      this box cannot build or test the WMF path at all.

## GUI memory leak — FOUND AND FIXED (2026-08-01, Linux workstation)

- [x] **FIXED**: `iris-ui` leaked one full RGBA preview image per captured frame
      (~27 MB/s at 30 fps; 788 MB → 4.8 GB in under a minute).
      **Root cause:** `ui_app.rs` called `ctx.load_texture("iris_preview", ...)` on every frame.
      `Context::load_texture` **allocates a new texture on every call** — it does not replace an
      existing one, despite the code comment there saying "create or replace texture". Overwriting
      `self.preview_texture` did not free the previous texture, so every preview image ever produced
      stayed alive.
      **Fix:** call `TextureHandle::set()` on the existing handle (reusing the allocation) and only
      fall back to `ctx.load_texture` when no handle exists yet.
      **Verified with heaptrack over identical 25 s runs:**

      | | before | after |
      |---|---|---|
      | peak heap | 607.34 MB | **24.81 MB** |
      | total leaked | 605.66 MB | **22.82 MB** |
      | RSS trend | 308 → 590 MB in 10 s | **flat: 158 MB, net −1 MB over 20 s** |

      heaptrack named the site exactly: *584.91 MB leaked over 476 calls* from
      `ui_app.rs:414` → `ColorImage::from_rgba_unmultiplied`, i.e. 1.23 MB per call =
      640×480×4 exactly. Residual 22.8 MB is one-time startup allocation (fonts, GL context) held
      until exit and is **not** growing.

- [ ] PERF (not a leak, still worth doing): the frame drain in `ui_app.rs` is
      `loop { rx.try_recv() }` and converts **every** queued frame to RGBA, uploading each one,
      even though only the last is ever displayed. Draining to the newest frame and converting once
      would cut redundant conversion work at high frame rates.

**Measured on Ubuntu 26.04 / NVIDIA 595.58.03, mock backend, GUI window open:**

| Config | Observed rate | RSS growth | Per frame |
|---|---|---|---|
| 3840x2160 @ 30 fps | ~27 fps | **~29 MB/s** (788 MB -> 4.77 GB in ~50 s) | ~1.1 MB |
| 640x480 @ 30 fps | ~22 fps | **~27 MB/s** | ~1.25 MB |
| 640x480 @ 5 fps | 5 fps | **~0 MB/s** (2 MB / 12 s) | ~33 kB |

**What this rules in and out:**
- **NOT proportional to frame size** — 640x480 is 36x less pixel data than 4K yet leaks at the same
  rate. So raw frame buffers and preview textures are *not* the driver.
- **Strongly dependent on frame RATE, and superlinearly so** — ~38x more growth per frame at 30 fps
  than at 5 fps, and essentially stable at 5 fps. That is the signature of a **producer/consumer
  imbalance**: something downstream cannot keep up at 30 fps and accumulates, rather than a fixed
  per-frame leak.
- Memory is fully returned to the OS on exit, so it is process-local heap growth, not a system leak.
- **Not isolated to UI vs pipeline.** The obvious experiment — headless mode (`IRIS_UI_HEADLESS=1`) —
  runs a fixed ~4 s script and exits before a meaningful sample can be taken, so it could not be
  used to separate the egui/eframe path from the capture/telemetry path. Making the headless runner
  loop for a configurable duration would make this cheap to settle.

**Why it matters:** at ~27 MB/s a machine with 8 GB free exhausts memory in roughly five minutes.
This box has 121 GB, which is the only reason the app looked healthy. Baxters OS targets ordinary
hardware, so this must be fixed before Iris ships in the ISO.

**Suspects worth checking first** (untested hypotheses, in order): the per-frame
`ctx.load_texture("iris_preview", ...)` call in `iris-ui/ui_app.rs` (egui defers texture frees to
end-of-frame, so a UI repainting slower than the capture rate may hold several large textures at
once); the `broadcast::channel(4096)` telemetry ring in `bootstrap.rs:245` combined with a slow
receiver; and the `loop { rx.try_recv() }` drain in `ui_app.rs`, which converts **every** queued
frame to RGBA and uploads a texture for each, discarding all but the last.

## Notes on MJPEG support (added 2026-08-01)

`PixelFormat::Mjpeg` was added so V4L2 enumeration reports what the hardware actually offers.
Previously `fourcc_to_pixel_format` returned `None` for `MJPG` (and a test asserted that), which
meant unmapped formats were skipped by the `VIDIOC_ENUM_FMT` loop entirely. On USB 2.0 UVC cameras
nearly every mode above ~640x480 is MJPEG-only, because uncompressed 1080p exceeds USB 2.0
bandwidth — so a 1080p-capable camera enumerated as **640x480-only** on Linux while reporting its
full 9 modes on Windows (Media Foundation decodes MJPEG transparently and reports NV12).

Enumerating the modes is therefore correct and necessary. **As of 2026-08-17 (`c8b6f85`) both
decode stubs are filled, so selecting an MJPEG mode is now useful end to end** — verified against
real hardware at 1920x1080. Callers should still check `PixelFormat::is_raw()` before treating
frame data as a pixel grid: `read_frame` deliberately returns the compressed bytes untouched, and
decoding is an explicit step via `iris_capture::mjpeg::decode_to_rgb24`.
