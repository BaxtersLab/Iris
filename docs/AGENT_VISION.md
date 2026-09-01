# Giving an agent eyes

Iris exists so an agent can **see outside the box**. This is how it asks.

One HTTP request, one JPEG, ready to drop into an OpenAI-format vision message.

```bash
curl -s 'http://127.0.0.1:9180/frame?max_width=768'
```

```json
{
  "sequence": 151,
  "width": 512,
  "height": 288,
  "captured_us": 1756759123456789,
  "age_ms": 27,
  "mirrored": false,
  "mime": "image/jpeg",
  "data_url": "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQ..."
}
```

`data_url` is the **complete** string for `image_url.url`. It is assembled by
Iris rather than by each caller, because a caller building it by hand can get
the MIME type or the base64 padding wrong silently — and the only symptom is a
model that reports seeing nothing.

## Straight into llama.cpp

```python
import requests

frame = requests.get("http://127.0.0.1:9180/frame?max_width=768").json()

requests.post("http://127.0.0.1:8080/v1/chat/completions", json={
    "model": "local",
    "messages": [{
        "role": "user",
        "content": [
            {"type": "text", "text": "What is in front of the camera?"},
            {"type": "image_url", "image_url": {"url": frame["data_url"]}},
        ],
    }],
})
```

That is the whole integration. No client library, no second transport, no
framing to implement.

## Parameters

| query | default | meaning |
|---|---|---|
| `max_width` | `768` | Downscale so the image is at most this wide, preserving aspect ratio. `0` disables scaling. Never upscales. |
| `quality` | `80` | JPEG quality, 1–100. |
| `max_age_ms` | `10000` | Refuse a frame older than this. `0` disables the check. |
| `mirror` | the window's toggle | `1` flips left-to-right, `0` does not. Omit to follow the Mirror button. |

**Why 768 by default.** A vision projector works on tiles a few hundred pixels
square. Handing it 1920x1080 costs encode time, transfer size and tokens to
reach the same tiles it would have produced anyway. Ask for more when you have
a reason; the camera's full resolution is always available with `max_width=0`.

## Responses

| status | meaning |
|---|---|
| `200` | A frame. `sequence` increases per captured frame — compare it to tell a fresh frame from one you already have. |
| `503` | Nothing captured yet. Iris is running but capture has not produced a frame; start capture. |
| `500` | The frame could not be converted. The message says why. |

Every successful answer carries **`age_ms`** — how old the frame was when it was
served, computed here so no caller has to reason about clock skew. `captured_us`
is the absolute capture time if you want it.

### On mirroring

Webcams commonly present a mirrored "selfie" view, which puts **text held up to
the camera backwards**. The Mirror button on the window's control strip flips
the image, and it flips **what this endpoint returns as well** — the reason to
flip is so a model can read the text, so a toggle that only changed the preview
would look like it worked and change nothing that matters.

Each response reports `mirrored`, and a request can override the toggle for one
call with `?mirror=1` or `?mirror=0` without disturbing the window.

### On staleness

**Latency is normal and expected.** Frames are sampled from a live feed; an
agent sees some of them, not all, and the one it gets is a few tens of
milliseconds old. Measured on the reference camera: the ring runs at ~31 fps and
`/frame` answers with frames **15–34 ms old**.

The `max_age_ms` guard is not there to police that. It exists for one case: when
capture has **stopped**, the ring still holds the last image from whenever it
stopped, and a model handed that will describe it confidently with nothing to
indicate the camera is no longer looking. The default of ten seconds catches a
dead feed without ever tripping on a slow one. Tighten it if your use is
reactive, or set `max_age_ms=0` if you want the last frame whenever it was.

## What this is not

**It is a pull, not a stream.** Ask when you want to look. There is no
subscription and no push over HTTP, because the consumer this was built for is
a model that looks when it decides to, not thirty times a second. Frame fan-out
to several *in-process* consumers exists (`iris-stream`), and a push transport
to another process does not — that is declared in `ROADMAP.md` rather than
half-built.

**Iris does no inference.** It hands over pixels and says nothing about what is
in them. *Iris is a tool, not a brain.*

## Where the frame comes from

```
camera ──▶ CaptureService ──▶ StreamService ──┬──▶ ring buffer ──▶ /frame
                                              └──▶ subscribers ──▶ the window
```

The ring is maintained whatever the stream mode, so `/frame` answers whether or
not anything else is watching — including with the window closed and the app
running headless.

## Binding elsewhere

The listener is `127.0.0.1:9180`, shared with `/metrics`. Change it with
`METRICS_BIND`:

```bash
METRICS_BIND=127.0.0.1:9500 ./run.sh
```

**Loopback by default, deliberately.** This endpoint serves a live camera. It
has no authentication, so binding it to a routable address puts the room on the
network. If you need that, put a reverse proxy with auth in front rather than
changing the bind address.
