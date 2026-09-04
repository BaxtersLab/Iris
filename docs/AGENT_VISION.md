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
| `503` | Nothing captured yet, the newest frame is too old, **or the frame carries no detail** — see below. The body says which. |
| `500` | The frame could not be converted. The message says why. |

Every successful answer carries **`age_ms`** — how old the frame was when it was
served, computed here so no caller has to reason about clock skew. `captured_us`
is the absolute capture time if you want it.

## `GET /sense` — numbers, without a model

`/frame` encodes a JPEG and base64s it: tens of kilobytes, and the wrong price
for *"has anything changed?"*. `/sense` answers that in arithmetic.

```json
{"sequence":11,"age_ms":44,"width":128,"height":72,
 "brightness":114.254,"detail":97.984,"motion":1.660}
```

| field | meaning |
|---|---|
| `brightness` | Mean luma, 0..255. A dark room reads ~10 on this hardware. |
| `detail` | Luma standard deviation — 0 means a featureless frame. |
| `motion` | Mean absolute luma change against an earlier ring frame, or **`null`**. |
| `width` / `height` | Working resolution (default 128 wide; `?width=` overrides). |

`?back=N` chooses how far back to compare (default 1 = the previous frame).

**`motion` is `null`, never `0`, when there is no comparison frame.** "No
earlier frame yet" and "nothing moved" are different facts, and a caller
thresholding on `motion < 2` would read a cold start as a still room. A
resolution change also yields `null` — a mode renegotiation is not movement.

### The escalation this is for

```
  brightness   1 frame,  arithmetic   is it dark?        → curtains, lamp
  motion       2 frames, arithmetic   did something move? → wake the model
  vision model 1 frame,  ~23s CPU     what/who is it?     → speak, decide
```

**The point is that the model runs only when the cheap layers say something
happened.** A vision model cannot be polled in a loop asking "anything yet?" —
but frames can be diffed all day. This is also the safety boundary: a model that
[invents a room from a blank field](#a-served-frame-is-not-a-guarantee-of-visible-content)
must never be the thing that moves an actuator or opens a speaker. The
arithmetic gates the fallible part.

### Measured, and the caveat that matters

| condition | `motion` |
|---|---|
| synthetic backend — frames identical by construction | **0.000** exactly |
| real camera, auto-exposure still ramping after open | 3 – 10 |
| real camera, still scene, exposure settled | **~1.7 and falling** |

**Auto-exposure registers as motion.** On opening the device, brightness climbed
79.8 → 114.3 over roughly two seconds while `motion` tracked it up to 10 — with
nothing moving in front of the lens. Anything thresholding on `motion` alone will
false-alarm on every device open and every lighting change.

Two ways to handle it, and the second is better:

* **ignore the first couple of seconds** after capture starts; or
* **compare `motion` against the change in `brightness`.** An exposure shift
  moves the *whole* frame roughly uniformly, so `motion` tracks `Δbrightness`; a
  person moving changes a *region*, so `motion` rises while `brightness` barely
  moves. In the settled samples above, brightness moved 0.007 between readings
  while motion was 1.66 — those are trivially distinguishable.

## The integration contract

Decided 2026-09-03 by measurement, not preference. **The harness attaches the
frame; the model does not fetch it.**

```
  user turn ──► harness ──GET /frame──► Iris ──► 200 + data_url
                   │                          └─► 503 + reason
                   ▼
        build one chat-completions request:
          [ {"type":"text",      "text": <the user's question, phrased to
                                          permit "nothing">},
            {"type":"image_url", "image_url":{"url": <data_url>}} ]
                   ▼
             vision slot (A4)  ──►  answer
```

On a **503** the harness puts the reason into the conversation as text — *"the
camera cannot be read right now: <reason>"* — and sends **no image**. It must not
silently omit the frame and let the model answer from imagination, which is the
failure this endpoint's `detail` check exists to prevent.

### Why not a `look()` tool the model calls

That was the intended shape, and it does not work on this estate today. Measured
on 2026-09-03:

| test | result |
|---|---|
| vision model + `look` tool, `tool_choice: auto` | no tool call — refuses, *"as an AI language model…"* |
| vision model + `look`, `tool_choice` **forced** | no tool call |
| text model (Hermes-2-Pro-Mistral-7B) + ordinary `get_weather` tool | no tool call |

The mechanical reason is not the models. **Neither GGUF carries a tool-capable
chat template** — `/props` shows templates of 1057 and 1281 characters, and
neither mentions `tools` or `tool_call` anywhere. `llama-server` therefore has
nothing to render a `tools` array into: it is dropped, and the model never sees
that a tool exists. It is answering a bare question, not declining an offer.

So this is a **packaging prerequisite in the model layer** — a chat template that
renders tool definitions — and it belongs to SOC / GGUF-Chatbox, not to Iris.
Nothing in Iris needs to change when it lands.

### What changes when tool-calling does work

The two-slot design already accommodates it, and this is the reason the split is
worth keeping. In the OpenAI format **a tool result is text, never an image**, so
a `look()` tool cannot hand a picture to the model that called it. It does not
need to:

* **A5 (text)** reasons and calls `look()`;
* the harness fetches `/frame` and asks **A4 (vision)** to describe it;
* **A4's description is the tool result** — text, which the format supports.

Iris's role is identical in both designs: serve a frame, or say why it cannot.

### A served frame is not a guarantee of visible content

**This is the most important paragraph in this document.**

A vision model handed a featureless image **does not abstain — it invents.**
Measured on 2026-09-03 against Iris's own synthetic frame (a uniform grey field,
RGB 130,130,130, one distinct colour across 331 776 pixels), a local Qwen2.5-VL
answered:

> "The image is a black and white photograph of a person standing in a room. The
> person is wearing a white shirt... The room has a window with curtains, and
> there is a table with a lamp on it."

A person, a window, curtains and a lamp, in a blank grey field — **in the same
confident register as its correct description of a real camera frame.** A covered
lens, an unlit room, a dead sensor or a mock backend therefore yields fabricated
content that nothing downstream can distinguish from truth.

So `/frame` **refuses** a frame that carries no information:

```
503  {"error":"the frame carries no detail (luma sd 0.000, floor 0.5) — a covered
      lens, an unlit scene or a synthetic backend. A vision model will describe
      this frame rather than report it as blank.",
      "detail":0.000,"min_detail":0.5}
```

| parameter | meaning |
|---|---|
| `min_detail` | Floor in **tenths** of a unit of luma standard deviation. Default `5` (= 0.5). `0` disables the check. |
| `detail` | Returned on **every** success too, so a caller can apply a stricter policy of its own. |

**The threshold is deliberately low, and that is a measurement, not a guess.** A
real frame from a **dark room** on the development box measured **3.17** — a
perfectly legitimate picture — while the synthetic frame measured **0.000**. A
floor of 5 or 10, which intuitively "looks low", would refuse real pictures of
dim scenes. What Iris can prove is the *absence* of variation, not the presence
of a subject; the guard catches the blank frame and deliberately nothing more.
If you want "too dark to be useful" refused as well, read `detail` and decide —
do not assume a 200 means the picture is worth looking at.

### Ask in a way that lets the model say "nothing"

Defence in depth, **not** the mechanism — the guard above is the mechanism.

Prompting measurably helps and is not sufficient. On the same blank frame and the
same model:

| prompt | answer |
|---|---|
| *"Describe what you see"* | a person, a shirt, a window, curtains, a lamp |
| *"…if the image is blank or uniform, say exactly that"* | **"The image is blank."** |
| *"Is there any discernible object? Yes or no first"* | *"No. The image is **completely black**…"* |

Three things follow. The **naive** phrasing is the dangerous one, and it is the
one most people write first. The third answer is right in verdict and wrong in
detail — the field is mid-grey, not black — so **the model's own account of a
blank frame is unreliable and cannot be used as the detector**. And Iris does not
control your prompt, which is why the pixel-space check exists regardless of how
you ask.

Give the model explicit permission to report nothing, and treat a 503 from
`/frame` as the authoritative "I cannot see".

### Pixel formats — every format the camera can deliver

`/frame` decodes **MJPEG, NV12, YUYV, RGB24 and BGR24**, which is every variant
`PixelFormat` has, and the match is deliberately left exhaustive so that adding
a sixth cannot compile until this endpoint handles it.

That guard exists because the alternative was tried. NV12 and YUYV both went
unhandled behind a catch-all, and the result was not a visible breakage: the
**preview window kept working**, because `iris-ui` carries its own colour
conversion, while `/frame` answered `500 cannot decode … to pixels` to every
request. On Windows that was total and permanent — Media Foundation decodes a
camera's MJPEG itself and reports NV12, so *every* frame took the dead path.

The window could see and the agent could not. Since the agent is the only
consumer that ever calls this endpoint, nothing on the desk showed it. A format
this endpoint cannot decode is now a build failure rather than a runtime 500.

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
