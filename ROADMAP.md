# Iris — ROADMAP (declared stubs and known gaps)

Per Agent Constitution Article VII §2–3: anything not fully implemented is declared here rather
than left silently incomplete. Format:

```
- [ ] STUB: <function/module name> — <what real implementation replaces it> — <reason it was deferred>
```

## Linux (V4L2) — opened 2026-08-01 during the Ubuntu 26.04 intake

- [ ] STUB: `iris-hal/v4l2_backend.rs` frame-read methods (5 sites returning `HalError::NotImplemented`)
      — real V4L2 streaming capture (`VIDIOC_REQBUFS` / `VIDIOC_QBUF` / `VIDIOC_DQBUF` mmap loop,
      mirroring the WMF `ReadSample` path) — deferred: the Linux backend was landed as
      enumerate + probe only; the capture pipeline was never wired up. **This is the remaining
      blocker for real-frame proof on Linux.** Enumeration and capability probing ARE implemented
      and are verified against real hardware (see `handoffs.md`, 2026-08-01).

- [ ] STUB: MJPEG preview decode — `iris-ui/ui_app.rs` frame→RGBA conversion has an empty
      `PixelFormat::Mjpeg` arm — a JPEG decoder (e.g. `zune-jpeg`, or `image` with only the jpeg
      feature) feeding the existing RGBA path — deferred: Iris is deliberately dependency-light
      (the V4L2 backend uses raw ioctls via libc, no bindgen), so adding a decode dependency is a
      design decision for the operator, not an incidental fix. Current behaviour is safe: `pixels`
      is left empty, the `pixels.len() == w*h*4` guard skips the texture update, and the preview
      holds its previous frame rather than rendering garbage.

- [ ] STUB: MJPEG ROI cropping — `iris-capture/src/service.rs` `apply_roi` `PixelFormat::Mjpeg` arm
      is a no-op setting `is_cropped = false` — crop-after-decode once the decoder above exists —
      deferred: MJPEG is a compressed stream with no pixel grid, so byte-slicing it would corrupt
      the JPEG. Refusing to crop is the correct behaviour until a decode step exists.

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

Enumerating the modes is therefore correct and necessary, but **selecting** an MJPEG mode is not yet
useful end-to-end until the two decode stubs above are filled. Callers should check
`PixelFormat::is_raw()` before treating frame data as a pixel grid.
