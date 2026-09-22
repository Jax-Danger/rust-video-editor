# Meridian

Meridian is a desktop non-linear editor written in Rust. The timeline is frame-accurate, the chrome is dense, and colour, transforms, and transitions live in the same project model as the cut.

`cargo run` opens the **Meridian** window on the sample sequence *Northline — Opening*.

## Workspace

| Crate | Role |
| --- | --- |
| `editor-core` | Timebase, project model, edit engine, grades, captions, templates, JSON I/O |
| `editor-media` | Probe, preview decode, timeline export, and Whisper captions. Stub by default; `ffprobe` / `ffmpeg` behind the `ffmpeg` feature; `whisper-cli` behind the `whisper` feature |
| `editor-app` | egui / eframe shell. Binary name: `meridian` |

`editor-core` never stores an edit in floating-point seconds. A [`Timebase`](crates/editor-core/src/time.rs) is a rational frame rate. A [`Frame`](crates/editor-core/src/time.rs) is an `i64` index on that rate. Media time is an integer tick count (`MediaTime`) and converts onto the sequence with rounded rational arithmetic. Drop-frame timecode is used for 29.97 and 59.94.

A project holds bins, media, multicam groups, and sequences. A sequence has video, audio, and caption tracks. Clips can be linked (picture and sound stay selected and split together). Markers, in/out, transitions, and title generators sit on the sequence.

Edits are transactional. `Session` clones the project, runs the operation, and pushes the previous project onto the undo stack only when the edit succeeds. Interactive slider drags snapshot once at drag start.

## Build and run

Rust **1.88** or newer. The egui stack in this lockfile pulls crates that declare that minimum (the package `rust-version` is still 1.85). This tree was built with 1.98.

```bash
cargo build
cargo test --workspace
cargo run
```

`editor-app` is the default workspace member, so `cargo run` from the repository root launches Meridian. `cargo test` without `--workspace` only runs that crate. The window title is `Meridian — <project>`.

### Fedora

Daily editing needs a desktop session, GTK 3 (native file dialogs), and ffmpeg. The 1× mix plays through ALSA. On Fedora Workstation that device is PipeWire's ALSA plugin, so both the ALSA library and `pipewire-alsa` are required.

```bash
sudo dnf install gcc gcc-c++ cmake pkg-config gtk3-devel alsa-lib-devel pipewire-alsa ffmpeg \
  dejavu-sans-fonts libxkbcommon-x11 mesa-libGL mesa-libEGL
cargo run -p editor-app --features ffmpeg
```

`gtk3-devel` is required to compile: the open, save, and import dialogs link GTK 3. `alsa-lib-devel` is required only for `--features ffmpeg`, which is the build that links rodio and plays the mix. `pipewire-alsa` makes that ALSA default device the PipeWire sink (Pulse-only setups stay silent until it is installed). The default `cargo build` / `cargo test` does not link ALSA and does not spawn ffmpeg. Auto Caption with a local model is a separate build: `cargo run -p editor-app --features whisper` (that feature includes ffmpeg). `gcc-c++` and `cmake` are only for building whisper.cpp itself — use `g++`, not Clang, because the Cloud and Fedora Clang packages often cannot find `libstdc++`. `dejavu-sans-fonts` is the UI font when it is installed. Caption burn-in does not use it: preview and export stamp the same built-in 8×8 bitmap.

### Debian / Ubuntu

```bash
sudo apt install gcc g++ cmake pkg-config libgtk-3-dev libasound2-dev ffmpeg \
  fonts-dejavu-core libxkbcommon-x11-0 libegl1 libgl1
cargo run -p editor-app --features ffmpeg
```

The first launch loads an in-memory example: three picture tracks, a lower-third title, linked interview audio, a cross dissolve, a wipe, keyframed picture-in-picture, captions, and markers. **File → Open Example** returns to it. **File → Save** and **File → Save As** write pretty JSON through a native dialog. A copy of that example lives at [`samples/northline-opening.json`](samples/northline-opening.json). **File → Open** (Ctrl+O) reloads a project file.

## Import

**File → Import** (Ctrl+I) opens a native file picker for video, audio, and stills. Dropping files onto the media pool does the same. The app probes each file (`ffprobe` when built with `--features ffmpeg`), stores the canonical path on the asset, and adds it to the selected bin (or the first bin when none is selected). Project `.json` files are rejected here; use **File → Open**. **File → Import from Path…** is the typed-path fallback.

Drag a pool item onto the timeline to overwrite at the drop frame. Hold Shift while dropping to insert and ripple. Double-click opens the clip in the **source monitor**. Overwrite / Insert (or `,` / `.`) place the source in–out range at the program playhead. One import batch is a single undo step. Paths round-trip in the project JSON. An empty pool says to import. A missing file is marked offline in the pool and on the clip, and the tooltip shows the path.

Stills shorter than five seconds are held for five seconds at 24 fps so they can be cut.

## Media pool bins

The pool is organised into bins (folders). Bins can nest: **New Bin** creates a root folder; right-click a bin for **New Sub-bin**, **Rename**, or **Delete Bin**. Click the triangle to expand or collapse a branch. Click a bin to select it — imports land there. Drag pool items onto a bin header (or use **Move to Bin** in the row menu) to move them. Deleting a bin moves its media to the parent bin and reparents child bins. The last bin cannot be deleted.

Bins round-trip in project JSON (`bins` with optional `parent`). The Northline example uses `Master`, `Interviews`, `B-Roll`, and `Audio`.

## Editing

The toolbar exposes Select, Razor, Ripple, Roll, Slip, and Slide, plus Overwrite and Insert. Track headers have lock, mute, and solo. Snapping is on by default (`S` toggles it).

| Gesture | Result |
| --- | --- |
| Overwrite | Places the source in–out range of the selected pool item at the program playhead, trimming or splitting what it covers |
| Insert | Same, then ripples later clips on sync-locked tracks |
| `C` or Ctrl+K | Razor at the playhead (linked partners split together) |
| Razor tool + click | Splits the clip under the pointer; transitions follow the right half |
| Delete / Backspace | Lift (leaves a gap). Shift+Delete ripple-deletes |
| Click a clip | Select it. Shift-click adds, Ctrl-click toggles. Esc clears the selection |
| Drag body | Move (lift, then overwrite) |
| Drag edge | Trim. Ripple, roll, slip, and slide follow the active tool |
| Double-click pool item | Open in the source monitor |

Linked selection is on by default so picture and sound move together. Inserting a linked pair is one ripple, not two.

### Markers

Press **M** to add a sequence marker at the program playhead. Markers store a name, colour, frame, and optional comment. They appear on the timeline ruler as coloured triangles (matching the marker colour) and snap like clip edges when snapping is on.

**[** and **]** jump to the previous or next marker. Click a ruler marker to jump and select it. With a marker selected and no clip selection, **Delete** removes it. The inspector lists markers when nothing is selected: click a row to jump, edit the name and colour, or delete from there.

Markers persist under each sequence in project JSON (`markers` with `id`, `frame`, `name`, `color`, and optional `comment`).

The Edit workspace shows a **source monitor** and a **program monitor** side by side. Source plays the selected pool clip with its own playhead and in/out marks. Program stays on the active sequence. Click a monitor (or press `\`) to focus it; Space / JKL / I / O then apply to that monitor. Overwrite and Insert always edit the sequence at the program playhead using the source marks (full file when unmarked).

Click or drag the timeline ruler, or the bar under either monitor, to scrub. The decoded preview follows that playhead. Playback and scrubbing reuse one ffmpeg job for a short burst of frames, then a memory cache and a disk cache (see [Long projects](#long-projects)) so a second pass over the same frames does not spawn ffmpeg again. The timeline scrolls horizontally by a frame origin, so an hour zoomed to single frames stays a viewport-sized strip instead of a multi-million-pixel layout. Scroll pans; Ctrl+scroll, Alt+scroll, a pinch, or a vertical middle-drag zooms around the cursor. `+` / `−` and the slider zoom around the center of the panel. The zoom slider is logarithmic, from single frames (64 px/frame) out to about 12 px per minute at 24 fps, which fits an hour in the panel. `F` or Shift+Z fits the sequence and scrolls back to the start. Double-click empty timeline space or the ruler to play or pause. Off-screen clips are not drawn. The clip under the pointer is a binary search on the sorted track, plus any clip that runs underneath later shots.

Track headers show a **T** target toggle on video and audio lanes. Armed tracks receive overwrite, insert, and Q/W ripple trims. Keys `1`–`9` toggle V1–V9; Shift+`1`–`9` toggle A1–A9.

## Multicam

A multicam group ties two or more video angles to one sync origin. Each angle has a video asset, an optional separate audio file, and a sync offset: the source frame that lines up with group time 0. Offset 0 means the file starts with the group. A camera that rolled early gets a positive offset so its clap lands on the same group frame as the other angles.

**Create.** In the media pool, Shift-click to add clips or Ctrl-click to toggle them. Pick at least two video items. Audio-only items in that selection attach, in order, as dedicated audio for those angles. **Create Multicam** (or **Edit → Create Multicam from Pool**) overwrites a clip at the playhead on the first unlocked video track and links an audio clip when any angle has sound. The timeline length is the overlap of the angles. A longer angle is left as a tail handle. The first selected video is angle 1.

**Switch.** When a multicam clip is the picture under the playhead, the program monitor grows an angle bank. Click an angle, use the same buttons in the inspector, or press Alt+`1`–`9`. Plain `1`–`9` still toggle track targets. If the playhead is inside the clip, Meridian razors there and the right-hand piece takes the new angle, picture and linked multicam audio together. On the first frame of the clip the whole clip retargets and nothing is split. Interior cuts that were written without a razor are drawn as ticks on the clip. Preview and Deliver both resolve the active angle through the shared compositor, so the exported frame is the angle on screen. Playback and export audio follow the same cuts.

**Sync.** The inspector lists each angle's sync offset in source frames. Dragging it slips that camera against the group without moving the timeline clip. The offset lives on the group, so every clip that uses the group stays in step.

Groups, offsets, and angle cuts are stored in the project JSON (`multicam_groups` on the project, `multicam` on the clip, cuts in group time). Projects saved before this field still load; an empty group list is omitted.

## Nested sequences (compound clips)

A nested clip is one timeline item that rasterizes a child sequence. The child lives in `project.sequences` like any other sequence. The parent clip's `source_in` / `source_out` are frames inside that child. Preview and Deliver composite the child at that frame and paint the result as one layer on the parent, including grades and transforms on the nest clip itself.

**Create.** Select one or more clips (linked partners expand automatically). **Edit → Nest Selection** moves them into a new child sequence and replaces them with nested clips on the same tracks — video and audio stay linked when both were nested. The child is named `Nested`, `Nested 2`, and so on. Multicam clips cannot be nested directly.

**Open.** Double-click a nested clip on the timeline, choose **Sequence → Open Nested Sequence**, or select a nested clip and open it from the menu. The program monitor shows a **← Parent** control while you are inside a nest. **Sequence → Close Nested Sequence** returns to the parent. Edits inside the child show on the parent immediately because both paths share the same compositor.

**Export.** Nested picture and audio recurse through the shared `compose_layers` path. A nest inside a nest is supported up to eight levels deep; deeper nesting is rejected to avoid cycles and runaway cost.

Nested bindings round-trip in project JSON (`nested: { "sequence": <id> }` on the clip). Older projects load with nested clips off.

## Keyboard

Shortcuts are global while you are not typing in a text field. The same list is under **Help → Keyboard Shortcuts**. On macOS, Command replaces Ctrl.

| Key | Action |
| --- | --- |
| Space | Play / pause at 1× |
| J / K / L | Reverse shuttle, stop, forward shuttle (tap again to go faster, up to 8×) |
| Left / Right | Step one frame |
| Shift+Left / Right | Jump 10 frames |
| Ctrl+Left / Right | Jump one second |
| Up / Down | Previous / next edit |
| Home / End | Go to start / end of the focused monitor |
| I / O | Mark in / out on the focused monitor (source or program) |
| \ | Toggle focus between source and program monitors |
| M | Add marker |
| [ / ] | Previous / next marker |
| V | Select tool |
| C or Ctrl+K | Razor at the playhead (also selects the razor tool) |
| / | Razor at the playhead (keeps the active tool) |
| Q / W | Ripple trim previous / next edit to the playhead (targeted tracks) |
| , / . | Overwrite / insert source in–out at the program playhead |
| 1–9 | Toggle video track target (V1–V9) |
| Shift+1–9 | Toggle audio track target (A1–A9) |
| Alt+1–9 | Switch multicam angle at the playhead |
| B / N / Y / U | Ripple, roll, slip, slide tools |
| S | Toggle snapping |
| Delete / Backspace | Lift delete |
| Shift+Delete | Ripple delete |
| Esc | Clear selection |
| Ctrl+Z | Undo |
| Ctrl+Shift+Z or Ctrl+Y | Redo |
| Ctrl+S | Save |
| Ctrl+O | Open project |
| Ctrl+N | New project |
| Ctrl+I | Import media |
| + / − | Zoom timeline |
| F / Shift+Z | Fit sequence in the timeline |
| Ctrl+scroll, Alt+scroll, or pinch | Zoom timeline |
| Middle-drag (vertical) | Zoom timeline |
| Scroll | Pan timeline |
| Double-click empty timeline | Play / pause |

Reverse shuttle and rates other than 1× play the picture and stay silent. At 1×, the ffmpeg build decodes interleaved stereo PCM at 48 kHz in about two-second chunks and plays it through the default ALSA device (rodio, via PipeWire on Fedora). The viewer shows a stereo meter and a badge: `Audio`, `Buffering`, `Silent`, `No device`, or `No audio`. A missing device does not stop the picture. The default build (no `ffmpeg` feature) does not link an audio backend; its badge is `No audio` and the status line says to rebuild with `--features ffmpeg`.

The **Audio** workspace is the mixer. Each audio track has a fader (−∞ to +12 dB, unity at 0 dB), a constant-power pan (center is unity, so an unpanned clip is unchanged), a 3-band EQ (low shelf 200 Hz, mid peak 1 kHz, high shelf 8 kHz, ±12 dB, plus an optional 80 Hz low-cut), mute, solo, and peak / RMS / peak-hold meters. Double-click an EQ band to reset it to 0 dB. A red clip lamp latches when that strip, or the master, reaches full scale; click it to clear. The master fader sits on the bus after the tracks. Fader, pan, EQ, mute, solo, master, and clip gain are applied while the chunk plays, so a move is heard without waiting for the next decode. Clip gain is an `AnimatedF32`: the inspector diamond (and the one on the strip) keys it at the playhead. The slider is 0 to 2; the value is stored up to 4. EQ settings persist on each track in project JSON (`eq.low`, `eq.mid`, `eq.high`, `eq.low_cut`; omitted when flat). Playback, the meters, caption extraction, and export all use the same rules: audible tracks only, clip gain, then pre-fader track EQ, then track fader × pan, then the master fader, then a hard limit at full scale.

Colour and transform parameters are `AnimatedF32`: a constant until you add a keyframe, then linear or hold interpolation in clip-relative frames. The inspector diamond toggles a key at the playhead.

The **Colour** workspace is Resolve-leaning without decorative chrome:

- **Scopes** (left column) — waveform luma and a vectorscope fed from the composited program frame
- **Wheels** — Lift, Gamma, and Gain offset wheels with keyframeable RGB offsets per channel
- **Luma curve** — drag the interior control points; endpoints stay fixed at black and white
- **White balance** — temperature / tint offset pad (same as before, now grouped with the wheels)
- **Lighting** sliders — exposure, contrast, highlights, shadows, temperature, tint, saturation

Wheels, curve, and sliders all run through the shared `GradeSample` compositor, so preview and Deliver match.

Transitions are centered on a cut and consume head and tail handles. Duration is in sequence frames. **Timeline → Add Transition** (or the cut menu) can place:

| Transition | Behaviour |
| --- | --- |
| Cross dissolve | Linear opacity mix |
| Wipe | Directional reveal (`angle_deg`: 0° L→R, 90° T→B, …) |
| Push | Both clips slide together |
| Dip to black | Outgoing fades out, then incoming fades in through black |
| Dip to white | Same through a white plate |
| Slide | Outgoing stays put; incoming slides over (left or right) |
| Blur dissolve | Cross dissolve with a blur peak at the midpoint |
| Iris | Circular reveal from the frame centre |

The inspector **Effects** section (video clips) adds stackable filters that run in the shared compositor for preview and Deliver:

| Effect | Parameter |
| --- | --- |
| Blur | Radius in sequence pixels (box blur) |
| Vignette | Amount and softness |
| Crop | Left / right / top / bottom insets (letterbox/pillarbox) |
| Sharpen | Unsharp-mask strength |

Filter parameters are `AnimatedF32` like grade and transform.

## Adjustment layers

**File → New Adjustment Layer**, or **New Adjustment Layer** in the media pool, drops a five-second adjustment clip on the highest video track that has room at the playhead. If every video track is busy, Meridian adds a track and places the layer there. The clip is selected so the inspector can edit grade, transform, and effects.

An adjustment layer has no media file and no pixels of its own. For its duration it grades and filters everything visually below it on the timeline — lower video tracks and any clips on the same track that composite before it — then higher tracks paint on top unchanged. Preview and Deliver both run through the shared `compose_layers` path. The timeline labels the clip `A` plus its name. The `adjustment` flag round-trips in project JSON; older projects load with adjustment clips off.

## Titles

**File → New Title**, or **New Title** in the media pool, drops a five-second title generator on the highest video track that has room at the playhead. If every video track is busy, Meridian adds a track and places the title there. The clip is selected so the inspector can edit it.

A title is not a media file. The clip stores the text, font size (a fraction of the frame height), colour, alignment, normalized position, and plate opacity. Those fields round-trip in the project JSON. Empty text draws nothing. The timeline labels the clip `T` plus the first line.

Preview and Deliver rasterize that generator in `compose_layers` — the same composite that grades, transforms, and stacks picture. Glyphs are the built-in 8×8 bitmap, scaled to the font size, with a straight-alpha plate behind the block. A title on a higher video track paints over the picture under it. Opacity and transform on the clip still move the whole layer. When decode is off, the program monitor paints that same raster over the proxy cards, in track order, instead of a coloured stand-in.

Northline opens with **NORTHLINE** on V3 for the first five seconds.

## Workspaces

- **Edit** — media pool, dual source/program viewers, inspector, captions, timeline
- **Colour** — scopes, viewer, lift/gamma/gain wheels, luma curve, white balance, and grade sliders
- **Audio** — the mixer docks in this page: faders, pan, mute, solo, and meters in the track bay, with the program meter on the right and the timeline still underneath
- **Deliver** — export presets (YouTube, Shorts, Instagram, ProRes master, H.265, audio WAV), codec/container, in/out, and **Export**. With `--features ffmpeg` this encodes a real file. Without that feature, Export still writes the JSON manifest and says the encoder is compiled out.

The **source monitor** decodes the selected pool clip through the same ffmpeg preview path (proxy when preferred). It does not composite grades, titles, or captions — those stay on the program side. The **program monitor** draws mute/solo, grade, transform, dissolve, wipe, push, and caption burn-in as proxy cards when decode is off. Title generators are the exception: proxy and decode both stamp the shared bitmap. With `--features ffmpeg` it decodes every visible video layer under the playhead and runs the same CPU compositor Deliver uses, titles included. Play, keyboard stepping, and mouse scrubbing follow the focused monitor's timebase. If ffmpeg is missing or every layer is offline, the proxy stays up and the viewer says why. Active caption cues are burned into the decoded program picture; on the proxy they are drawn with the UI font in the same bottom safe area.

### Program monitor GPU upload

The composited program frame stays a CPU RGBA buffer. Deliver encodes that same `compose_layers` output and does not read the display texture, so export remains the source of truth. Scopes read the CPU buffer too.

When a composite is ready, the program monitor uploads it to **one** GPU texture and samples that while you scrub or play. A repeated frame is not uploaded again. A new frame at the same canvas size replaces the texels in place. The header badge reads **GPU** while that path is active.

```
cargo run -p editor-app --features ffmpeg
cargo run -p editor-app --features "ffmpeg,wgpu"
```

The first command uses eframe's default glow renderer and uploads with `texSubImage2D`. The second selects the wgpu renderer and uploads with `queue.write_texture` (`Rgba8UnormSrgb`, rows padded to 256 bytes when the width requires it).

**CPU fallback.** The badge reads **CPU** when neither GPU context is current: a headless test, a glow context that did not come up, or a `wgpu` build with no adapter. That path still reuses a single egui texture instead of allocating one per frame. It goes through a `ColorImage` copy, which is the slower display path. Proxy cards (decode off, or every layer offline) do not upload.

### What matches, and what is still approximate

Preview (`--features ffmpeg`) and Deliver call one compositor in `editor-media`. Higher video tracks paint later, so they sit on top. Muted video is skipped; solo hides the other video tracks.

| Piece | Shared? | Notes |
| --- | --- | --- |
| Grade | Yes, one formula | Luma curve, exposure in stops, contrast about mid grey, lift/gamma/gain wheel offsets by tonal region, split shadow/highlight lift, temperature, tint, then Rec.709 luma saturation. Evaluated on the decoded RGB values as stored — not scene-linear, and not a film print |
| Opacity, scale, position, rotation, anchor | Yes | Straight alpha over. Anchor is the normalized point that sits on the position |
| Cross dissolve | Yes | Incoming blends over an opaque outgoing plate, which is a linear mix when that plate is opaque |
| Wipe | Yes | `angle_deg` is the direction the reveal travels: 0° left to right, 90° top to bottom, 180° from the right, 270° from the bottom. Other angles use the same half-plane per pixel. The proxy only clips axis-aligned wipes; a diagonal wipe on the proxy is a stand-in |
| Push | Yes | Direction is left, right, up, or down. Both sides slide; they are not a crossfade |
| Dip to black / white | Yes | Two-stage opacity through empty (black) or a white plate |
| Slide | Yes | Outgoing is stationary; incoming slides from off-screen |
| Blur dissolve | Yes | Cross dissolve plus a shared blur radius that peaks at the cut |
| Iris | Yes | Circular mask from the frame centre |
| Blur, vignette, crop, sharpen | Yes | Per-clip filters applied before the layer is composited |
| Captions | Placement yes, glyphs mostly | Both burn the same 8×8 bitmap into the picture (about 32px at 1080p, 48px bottom margin). The proxy uses the UI font instead. Soft `mov_text` subtitles are still written. H.264 then quantizes the burn-in |
| Titles | Yes | Generator clips on a video track. Text, size, colour, alignment, position, and plate are rasterized in `compose_layers` for the monitor and for Deliver. The proxy draws that same bitmap in track order |
| Stacking | Yes | Simple alpha over only. No blend modes, no motion blur, no track mattes |
| Program display | Preview only | The monitor uploads the CPU composite to one GPU texture (glow by default, wgpu with `--features wgpu`). **CPU** in the program header means that upload fell back to a reused egui texture. Deliver ignores the display texture |

A source is stretched to fill the clip's quad. It is not letterboxed when its aspect differs from the sequence. The monitor fits the sequence inside 960×540 before compositing; export composites at sequence size, with a decoded edge capped at 1920px. On the Northline picture-in-picture frame, a 16×16 block average of the H.264 export sits within about 1.3 levels of the CPU composite. That is the same picture through a codec, not a bit-identical file. Keyframes are evaluated on every frame in both paths.

## Clip speed

Select a video or audio clip. The inspector **Speed** section sets a constant rate from 25% to 400%, with presets at 25, 50, 100, 200, and 400. **Reverse** plays that same source range backwards. **Ramp over clip** draws a straight line from the start speed at the first frame to **Ramp to** at the out point.

The rate is stored on the clip as an [`AnimatedF32`](crates/editor-core/src/effects.rs). A constant is `rate.base` with no keys. A ramp is a key at frame 0 and a key at frame `-1`. Frame `-1` means the clip's current out point, so a trim keeps the ramp stretched across whatever duration the clip has now. 100% forward is omitted from the project JSON. Older projects load as 100%.

Changing the speed or the ramp ripples the clip's timeline duration so the same source in and out are consumed at the new average rate. Later clips on that track, and on sync-locked tracks, shift with the edit. Linked clips take the same speed and the same duration, so a linked picture and audio clip stay in line. Reverse does not change the duration. The timeline clip label shows the rate (`200%`, `100–300%`, `100% Rev`).

Preview and Deliver sample picture through one function, `source_frame_at`. A 200% clip steps two source frames per timeline frame. A 50% clip holds each source frame for two timeline frames. A ramp follows the integral of the line. Once the integral leaves the source in/out, the picture holds the first or last included frame.

Audio on a retimed clip is muted during playback and left out of the Deliver graph. Deliver names the clip in its report. A constant 100% forward clip, and a ramp that sits on 100% at both ends, still play. Reverse is muted too. The picture stays on the timeline clock; the sound does not play at the wrong rate and drift. Pitch-preserving resample is a follow-up.

## Templates

**File → New Project** lists the presets in [`templates/`](templates/). They set resolution, frame rate, and the default video, audio, and caption tracks.

| File | Sequence |
| --- | --- |
| `blank.json` | 1920×1080, 24 fps, one track of each kind |
| `youtube-1080p24.json` | 1920×1080, 24 fps |
| `youtube-1080p30.json` | 1920×1080, 30 fps |
| `vertical-9x16.json` | 1080×1920, 30 fps |
| `cinematic-widescreen.json` | 2048×858 (2.39:1), 24 fps |

The same JSON is embedded with `include_str!`, so the app does not depend on the current working directory. `load_template_dir` reads a folder of the same shape if you add presets later.

## Media probe and preview

`editor_media::probe` returns duration, resolution, codecs, and whether the file is offline. `editor_media::decode_frames` turns a timestamp into RGBA for the source and program viewers.

The default build never links or spawns ffmpeg. Probe classifies the extension, invents a stable duration from the file name (stills become a five-second hold), and marks missing files offline. Decode returns `FeatureDisabled`. Drop a `<file>.probe.json` sidecar next to the media if you want the stub to return exact values.

### Install ffmpeg and ffprobe

Preview shells out to the `ffmpeg` and `ffprobe` binaries. It does not link libav, so any system build is enough, including one without proprietary codecs you don't have. The sample clips are H.264 + AAC, which a normal ffmpeg package can read. Other files decode only when that same binary has a decoder for them.

| Platform | Install |
| --- | --- |
| macOS | `brew install ffmpeg` |
| Debian / Ubuntu | `sudo apt install ffmpeg` |
| Fedora | `sudo dnf install ffmpeg` |
| Arch | `sudo pacman -S ffmpeg` |
| Windows | `winget install Gyan.FFmpeg` (or `choco install ffmpeg`), then open a new terminal so `ffmpeg` and `ffprobe` are on `PATH` |

Check with `ffmpeg -version` and `ffprobe -version`.

### Run with real pictures

```bash
cargo run -p editor-app --features ffmpeg
```

`cargo run` and `cargo test` without the feature stay on the placeholder cards and do not need ffmpeg installed.

The example sequence *Northline — Opening* points its three picture clips at synthetic files in [`samples/media/`](samples/media/):

| File | What you see | Length |
| --- | --- | --- |
| `interview.mp4` | `testsrc2` card with a running clock | 480 frames, 24 fps |
| `city_broll.mp4` | classic `testsrc` card | 288 frames, 24 fps |
| `aerial.mp4` | the same card with a moving hue | 192 frames, 24 fps |

They are generated, not third-party footage. Regenerate them with [`scripts/generate-sample-media.sh`](scripts/generate-sample-media.sh) if you have ffmpeg. Launching from the repo root (or any subdirectory) resolves those relative paths. The viewer stacks every visible video layer, so V2's keyed aerial sits on top of V1 instead of replacing it.

**File → Import** probes any file `ffprobe` can open and writes the absolute path into the project. Offline media and a missing `ffmpeg` binary leave the shell usable and put the reason on the program monitor and on the clip.

## Long projects

A long cut stays interactive when preview is looking at proxies, decoded frames are cached, and the timeline only paints what is on screen. **Sequence → Dense Sequence (400 clips)** builds a synthetic 13-minute cut of the sample interview so you can try the zoom without importing footage. Export ignores proxies and always encodes the original files.

### Proxies

A proxy is a H.264 file at most **960 pixels wide** (shorter sources stay at their own width), `libx264` `veryfast` CRF 23, no B-frames, GOP 12, no audio, `+faststart`. Picture preview can use it. Audio playback, captions, and Deliver keep reading the original.

| | |
| --- | --- |
| Saved project | `<project directory>/<name>.meridian/proxies/` |
| Unsaved project | `$XDG_CACHE_HOME/meridian/proxies` or `~/.cache/meridian/proxies` |

The pool toggle **Proxies / Full** is **Prefer Proxies**. When it is on, a layer decodes its proxy if that file is on disk and falls back to the original if it is not. **Full** always decodes the original. The program header shows `Proxy` when a proxy file was actually used, and `Full*` when proxies are preferred but this frame fell back. Generating proxies turns the preference on.

**Selected** proxies the selected pool item. **Project** proxies every online video in the project. Progress is the status line (`Proxy 2/5 — interview.mp4`). **Cancel** stops between files; finished files stay attached. The same commands are under **File**. The proxy path is stored on the media asset and saved with the project. A missing proxy is not an error.

On Fedora, with the ffmpeg build:

```bash
sudo dnf install ffmpeg
cargo run -p editor-app --features ffmpeg
```

Import the camera originals (or open the example), choose **Project** in the media pool, wait for the status line, then leave **Proxies** on while you cut. Switch to **Full** when you need to judge a grade. Deliver still writes the originals.

The default `cargo build` does not spawn ffmpeg. **Selected** / **Project** then report that the feature is compiled out.

### Decode cache

Preview frames are keyed by the file being decoded (original or proxy), the source frame, and the decode size.

| Cache | Default | Where |
| --- | --- | --- |
| Memory | 256 MiB, and at most 192 frames | Process RAM, dropped on quit |
| Disk | 512 MiB, and at most 4096 files | `$XDG_CACHE_HOME/meridian/frames` or `~/.cache/meridian/frames` |

The disk key also includes the source file's modification time, so replacing a file does not replay a stale frame. One frame larger than the budget is kept; older frames are deleted until the rest fits. A burst is read from disk only when every frame in that burst is already stored. Memory is checked first, then disk, then ffmpeg.

### Timeline scale

Tracks are sorted by start frame. Painting and hit-testing use a binary search for the clips that intersect the viewport, including a clip that starts off-screen and runs into view. A clip that ends after a later clip — a title or another angle on the same track — is indexed when the project is normalized and is still drawn and hit-tested when the search window starts after it. The ruler spaces ticks from single frames out to hours, and only for the visible range. Scroll position is a frame number, not a pixel offset into a strip as wide as the sequence, so frame zoom an hour in stays precise. At the overview zoom, clips thinner than a couple of pixels draw as a solid mark instead of a labelled block. **Fit** (Shift+Z) uses the panel width, so a long sequence actually fits. `+` / `−` and the zoom slider keep the playhead where it is, so an overview does not jump to the tail.

### Relink

Offline media shows an amber dot, the word Offline, and an **Offline** pill. Online media shows a green dot and the word Online. A row with a proxy file adds `Proxy` on that line.

**File → Relink Media…**, the pool **Relink** button, or a click on the Offline pill picks a new path for the selected item. The media id stays, so timeline clips remain linked. The path, duration, and codecs are probed again, and any proxy is cleared because it belonged to the previous file. One relink is one undo step.

## Captions

Each sequence can carry a caption track of editable `CaptionCue`s. **Auto Caption** transcribes a range and replaces that track in one undo step.

The range is the selected clip, otherwise the marked in and out when both are set, otherwise the whole sequence. The ffmpeg build mixes the audible audio in that range — mute, solo, and clip gain included, with silence in the gaps — to a 16 kHz mono wav. Word times on that wav map linearly onto sequence frames, then words are grouped into cues (a pause over 0.45 s, a cue longer than about 2.8 s, 48 characters, or sentence punctuation).

### Stub

The default build, and any build that cannot find `whisper-cli` or a model, uses `StubTranscriber`. It slices the range into roughly two-second cues and cycles a fixed phrase list. No network, no model. The captions panel says which backend it will use. If Whisper runs and returns no speech, or the process fails, the same stub cues are written and the status line keeps the error.

### Local Whisper

```bash
cargo run -p editor-app --features whisper
```

`whisper` turns on the ffmpeg feature as well. It does not download a model and it does not link whisper.cpp, so `cargo test` without the feature never needs either. At runtime the app looks for `whisper-cli` (or `whisper-cpp`) on `PATH`, then `~/.local/bin/whisper-cli`, `~/.cache/meridian/whisper.cpp/build/bin/whisper-cli`, `/usr/local/bin`, and `/usr/bin`. Override with `WHISPER_BIN`. The model search is `WHISPER_MODEL`, then the first of `ggml-tiny.en.bin`, `ggml-base.en.bin`, `ggml-tiny.bin`, `ggml-base.bin`, and `ggml-small.en.bin` under `~/.cache/whisper`, then `./models/ggml-tiny.en.bin`.

On Fedora, build whisper.cpp with GCC:

```bash
sudo dnf install git cmake gcc gcc-c++
git clone --depth 1 https://github.com/ggml-org/whisper.cpp.git
cmake -S whisper.cpp -B whisper.cpp/build -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_C_COMPILER=gcc -DCMAKE_CXX_COMPILER=g++
cmake --build whisper.cpp/build -j --target whisper-cli
install -D whisper.cpp/build/bin/whisper-cli "$HOME/.local/bin/whisper-cli"
mkdir -p "$HOME/.cache/whisper"
curl -L -o "$HOME/.cache/whisper/ggml-tiny.en.bin" \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin
```

Debian and Ubuntu use the same cmake line after `sudo apt install git cmake g++`. A larger `ggml-base.en.bin` or `ggml-small.en.bin` in that cache directory is picked up automatically when tiny is absent; set `WHISPER_MODEL` to force one. [`scripts/setup-whisper.sh`](scripts/setup-whisper.sh) does the clone, the GCC build, and the tiny.en download.

The Northline sample audio is sine tones, so Whisper on the example falls back to stub cues. Import a clip that contains speech, select it, and run **Auto Caption**.

`editor_core::parse_stt_json` reads whisper.cpp `-ojf` token offsets (milliseconds) and OpenAI `segments` / `words`. `CaptionTranscriber` is still the seam for another engine.

## Deliver

**Deliver → Export** runs ffmpeg and writes the file in the path field (default `/tmp/meridian-export.mp4`). The tested path is H.264 + AAC in an mp4. H.265, ProRes, and DNxHR are passed through as codec arguments when that ffmpeg build has the encoder.

### Export presets

The **Preset** menu at the top of Deliver applies a profile that sets codec, container, suggested filename suffix, burn-captions default, and use in/out default. Built-in presets live in [`presets/deliver/`](presets/deliver/):

| Preset | Codec / container | Notes |
|--------|-------------------|-------|
| YouTube 1080p | H.264 / mp4 | AAC 192 kbps, burned captions on |
| YouTube Shorts | H.264 / mp4 | Vertical-friendly naming suffix |
| Instagram | H.264 / mp4 | AAC 128 kbps |
| Master (ProRes 422) | ProRes 422 / mov | Archive handoff; needs `prores_ks` in ffmpeg |
| H.265 (smaller file) | H.265 / mp4 | CRF 20 when no bitrate hint is set |
| Audio only (WAV) | PCM / wav | Mixed timeline bus only — skips picture encode |

Selecting a preset fills the format fields and rewrites the output path from the sequence name plus the preset suffix (for example `Northline-Opening_youtube.mp4`). **Save JSON** writes the current settings as a custom preset under `~/.config/meridian/presets/deliver/`. The last-used Deliver settings are restored on the next launch from `~/.config/meridian/deliver-last.json`.

Optional **video** and **audio bitrate hints** on a preset replace the default CRF / AAC rate in the ffmpeg arguments. ProRes and DNxHR show a note when selected; if the encoder is missing, Export surfaces the ffmpeg error.

The graph is the sequence, or the marked in/out when that checkbox is on. Picture is rasterized with the shared compositor (one frame at a time, decoded in short bursts) and piped to ffmpeg as raw video. Audio is still an ffmpeg filter graph:

- Visible video tracks, bottom to top, with each clip's opacity, transform, and grade. A muted video track is skipped, so the tracks under it show through. Solo hides the other video tracks. An empty stack is black.
- A cross dissolve, wipe, or push on a track composites the outgoing and incoming clips for that frame. Wipe angle and push direction follow the project. Anchor is included.
- Audio uses the mixer bus: mute and solo, clip gain (with keyframes), pre-fader track EQ, track fader, constant-power pan, then the master fader. Offline audio is skipped and named in the report. Offline video that is actually visible fails the export.
- Captions are burned with the shared bitmap when **Burn captions into the picture** is checked, and written as `mov_text` soft subtitles on mp4 and mov. Uncheck the box to keep them soft only. MXF does not get the soft-sub input.

A progress bar follows ffmpeg's `out_time`. **Cancel** sends `SIGTERM`. A failed or cancelled encode deletes the partial file. Without `--features ffmpeg`, Export writes the JSON manifest instead and explains how to rebuild.

## Tests

`cargo test --workspace` covers timebase conversion and drop-frame timecode, overwrite, insert, razor, lift and ripple delete, move, trim, ripple, roll, slip, slide, transitions, keyframes, undo, templates, deliver presets (apply, custom JSON, last-settings round-trip), the sample project round-trip, imported media paths in JSON, media-pool bins (create, rename, move, delete, JSON round-trip), sequence markers (add, edit, delete, JSON round-trip), proxy attach and relink, timeline culling on an 800-clip sequence, ruler spacing across an hour, the stub probe, still-image holds, the ffprobe JSON parser, preview frame-request bounds, proxy argument planning and preview fallback, the disk frame cache, caption JSON parsing, the export plan (grade, picture-in-picture, dissolve, gain, pan, fader, burned captions, deliver bitrate hints, audio-only WAV planning, retimed source frames, muted retimed audio), multicam sync offsets, razor angle switches, group-time cuts, JSON round-trip, and raster frames that follow the active angle, nested sequence create/frame mapping/JSON round-trip/export raster, the mix bus (pan law, mute, solo, keyframed gain, 3-band EQ, peak and RMS), clip speed (constant 25–400%, reverse, a linear ramp, JSON round-trip, and duration ripple), adjustment layers (JSON round-trip, track placement, composite stacking), and the shared composite (luma curve, lift/gamma/gain wheels, grade split, dissolve mix, wipe angle, push, dip, slide, blur dissolve, iris, clip filters, anchor, caption burn-in, adjustment grade-below). It does not spawn ffmpeg or whisper.

`cargo test -p editor-media --features ffmpeg` also encodes a short H.264/AAC mp4 when `ffmpeg` is on `PATH`. `cargo test -p editor-app --features whisper` builds the local speech-to-text path; the binary and model are resolved at runtime, not at compile time.

## Roadmap

- Pitch-preserving audio resample for retimed clips. Picture speed is sampled in preview and export; retimed audio is muted so it does not drift
- GPU viewer. The CPU composite already stacks tracks, grades, transforms, eight transitions, and four clip filters; it is not a full optical-flow or blend-mode engine
- Fairlight-class dynamics and track sends. The mixer already has faders, pan, 3-band EQ, mute, solo, meters, and keyframed clip gain
- OFX-style plugins for third-party effects
- Scene-linear grading and a hinted caption font. Preview and export already share the display-space formula and the bitmap burn-in
- Gang edits and a command palette

## License

[MIT](LICENSE). Copyright 2026 Meridian Contributors.
