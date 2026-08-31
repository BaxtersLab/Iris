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

- [x] **DONE 2026-08-31** — ~~PERF: the frame drain in `ui_app.rs` converts every queued frame~~.
      It was filed as a perf nit. It was **a hang**. The loop only ended when `try_recv` returned
      `Empty`, and each iteration converted a frame inline on the UI thread, so once a conversion
      cost about as much as the interval between frames the producer refilled the channel as fast
      as the loop drained it and `update()` never returned to the event loop.
      **Measured on this box, debug build, mock backend, 30 s runs, no user input:**

      | config | repaints BEFORE | repaints AFTER |
      |---|---|---|
      | 640x480 NV12 @30 (`target_fps` 2) | 647 / 15 s | — (never reproduced; the loop kept up) |
      | 640x480 NV12 @30 | **1** | **453** |
      | 3840x2160 NV12 @30 | **1** | **25** |

      **Reproduced on real hardware the same day**, `32e6:9221` at 1920x1080 MJPEG,
      `IRIS_BACKEND=v4l2`, 30 s, no user input, against a build of `8364f2c`:
      **3 repaints before, 34 after, with 589 frames captured in both runs** — so
      the whole difference is the UI thread. It is 3 rather than 1 because MJPEG
      arrives over USB with jitter, so the old loop occasionally reached `Empty`
      and escaped; that intermittency is why it survived. On the camera the fix
      avoids 87 full 1920x1080 JPEG decodes in 30 s
      (`received=117 converted=30 skipped=87`).

      One repaint in thirty seconds is a frozen window with a pinned core. The fix is
      `drain_to_newest()`: pop the backlog without converting, convert only the survivor, and cap
      one drain at `MAX_DRAIN_PER_REPAINT` so the UI thread does bounded work no matter how far
      behind it is. Conversions saved, same runs: 355 of 742 frames at 640x480 (52.2% converted),
      94 of 115 at 4K (18.3% converted). MJPEG makes the old shape worse still, since each
      conversion is then a full JPEG decode.

- [x] **DONE 2026-08-31** — the app never asked to be repainted. `grep -r request_repaint` returned
      **nothing**. egui's run loop is reactive: it calls `update()` on input and window events and
      otherwise sleeps, so the live camera preview only advanced while the pointer or keyboard was
      generating events over the window. Unfocused or covered, it froze on the last frame while
      capture ran on behind it. `drive_repaints()` now paces the window at ~60 Hz while a capture
      receiver exists and 4 Hz when idle.

- [x] **DONE 2026-08-31** — latent panic in the preview conversion. The RGB24/BGR24 arms read
      `d[i + 1]` and `d[i + 2]` off an index step, so a buffer whose length was not a multiple of 3
      — a short read, a truncated frame — indexed past the end and panicked **on the UI thread**.
      Now `chunks_exact(3).take(w * h)`, so a short buffer degrades to a skipped frame.
      Falsified: restoring the index form fails `a_ragged_rgb_buffer_does_not_panic` with an
      out-of-bounds panic at that exact line.

## Config surface — declared but not routed, opened and closed 2026-08-31

- [x] **DONE 2026-08-31** — `capture.pixel_format` and `capture.drop_policy` were dead strings.
      `bootstrap.rs` hardcoded `PixelFormat::Bgr24` and `DropPolicy::Oldest` while both fields sat
      in `IrisConfig`, were serialised, and were range-checked by `validate()`. The default config
      says `nv12`; capture ran BGR24. Proven by the frame size in telemetry: `size_bytes: 921600`
      (640x480x3) before, `460800` (640x480x1.5) after, and `614400` (x2) with `pixel_format
      = "yuy2"`. Same shape as the 2026-08-01 finding that `IrisConfig::load()` itself was never
      called — Article XI §3, one level down.

- [x] **DONE 2026-08-31** — `IrisConfig::validate()` had **no caller outside its own tests**. A
      file with an out-of-range width, an unknown drop policy or a nonsense log level was accepted
      whole. `main.rs` now validates after loading and falls back to defaults with the reason
      printed. Proven: `drop_policy = "banana"` →
      `config invalid (config error: drop_policy must be 'oldest' or 'newest'); falling back to defaults`.

- [x] **DONE 2026-08-31** — the accepted pixel-format list was wrong in both directions. It named
      `bgra8`, which no Iris backend has ever produced and which is 4 bytes per pixel where the
      nearest variant is 3, and it omitted `rgb24` and `bgr24` — one of which was what capture was
      actually running. `ALLOWED_PIXEL_FORMATS` now lives in `iris-core` and a test in `iris-hal`
      asserts every name it accepts can be parsed by `PixelFormat::from_config_name`, so the two
      cannot drift apart again. `yuy2` is kept as the Windows spelling of `yuyv`.

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

**Suspects worth checking first** *(historical — kept for the reasoning; all three are now
settled, so do not re-open them)*: the per-frame `ctx.load_texture("iris_preview", ...)` call in
`iris-ui/ui_app.rs` — **this was the leak**, fixed 2026-08-01; the `broadcast::channel(4096)`
telemetry ring in `bootstrap.rs` combined with a slow receiver — **not implicated**; and the
`loop { rx.try_recv() }` drain, which converts every queued frame and discards all but the last —
**real, and worse than suspected**: it was a UI-thread livelock, fixed 2026-08-31 above.

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
