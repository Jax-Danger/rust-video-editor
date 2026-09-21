# Meridian

Meridian is a desktop non-linear editor written in Rust. The timeline is frame-accurate, the chrome is dense, and colour, transforms, and transitions live in the same project model as the cut.

`cargo run` opens the **Meridian** window on the sample sequence *Northline — Opening*.

## Workspace

| Crate | Role |
| --- | --- |
| `editor-core` | Timebase, project model, edit engine, grades, captions, templates, JSON I/O |
| `editor-media` | Probe and preview decode. Stub by default; `ffprobe` / `ffmpeg` CLI behind the `ffmpeg` feature |
| `editor-app` | egui / eframe shell. Binary name: `meridian` |

`editor-core` never stores an edit in floating-point seconds. A [`Timebase`](crates/editor-core/src/time.rs) is a rational frame rate. A [`Frame`](crates/editor-core/src/time.rs) is an `i64` index on that rate. Media time is an integer tick count (`MediaTime`) and converts onto the sequence with rounded rational arithmetic. Drop-frame timecode is used for 29.97 and 59.94.

A project holds bins, media, and sequences. A sequence has video, audio, and caption tracks. Clips can be linked (picture and sound stay selected and split together). Markers, in/out, and transitions sit on the sequence.

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

Daily editing needs a desktop session, GTK 3 (native file dialogs), and ffmpeg. Audio playback also needs ALSA (PipeWire's ALSA plugin is enough).

```bash
sudo dnf install gcc pkg-config gtk3-devel alsa-lib-devel ffmpeg \
  libxkbcommon-x11 mesa-libGL mesa-libEGL pipewire-alsa
cargo run -p editor-app --features ffmpeg
```

`gtk3-devel` is required to compile: the open, save, and import dialogs link GTK 3. `alsa-lib-devel` is required only for `--features ffmpeg`, which is the build that plays audio. The default `cargo build` / `cargo test` does not link ALSA.

### Debian / Ubuntu

```bash
sudo apt install gcc pkg-config libgtk-3-dev libasound2-dev ffmpeg \
  libxkbcommon-x11-0 libegl1 libgl1
cargo run -p editor-app --features ffmpeg
```

The first launch loads an in-memory example: three picture tracks, linked interview audio, a cross dissolve, a wipe, keyframed picture-in-picture, captions, and markers. **File → Open Example** returns to it. **File → Save** and **File → Save As** write pretty JSON through a native dialog. A copy of that example lives at [`samples/northline-opening.json`](samples/northline-opening.json). **File → Open** (Ctrl+O) reloads a project file.

## Import

**File → Import** (Ctrl+I) opens a native file picker for video, audio, and stills. Dropping files onto the media pool does the same. The app probes each file (`ffprobe` when built with `--features ffmpeg`), stores the canonical path on the asset, and adds it to the first bin. Project `.json` files are rejected here; use **File → Open**. **File → Import from Path…** is the typed-path fallback.

Drag a pool item onto the timeline to overwrite at the drop frame. Hold Shift while dropping to insert and ripple. Double-click, or the Overwrite / Insert buttons, place the selected item at the playhead. One import batch is a single undo step. Paths round-trip in the project JSON. An empty pool says to import. A missing file is marked offline in the pool and on the clip, and the tooltip shows the path.

Stills shorter than five seconds are held for five seconds at 24 fps so they can be cut.

## Editing

The toolbar exposes Select, Razor, Ripple, Roll, Slip, and Slide, plus Overwrite and Insert. Track headers have lock, mute, and solo. Snapping is on by default (`S` toggles it).

| Gesture | Result |
| --- | --- |
| Overwrite | Places the selected pool item at the playhead, trimming or splitting what it covers |
| Insert | Same, then ripples later clips on sync-locked tracks |
| `C` or Ctrl+K | Razor at the playhead (linked partners split together) |
| Razor tool + click | Splits the clip under the pointer; transitions follow the right half |
| Delete / Backspace | Lift (leaves a gap). Shift+Delete ripple-deletes |
| Click a clip | Select it. Shift-click adds, Ctrl-click toggles. Esc clears the selection |
| Drag body | Move (lift, then overwrite) |
| Drag edge | Trim. Ripple, roll, slip, and slide follow the active tool |
| Double-click pool item | Overwrite at the playhead |

Linked selection is on by default so picture and sound move together. Inserting a linked pair is one ripple, not two.

Click or drag the timeline ruler, or the bar under the program monitor, to scrub. The decoded preview follows the playhead. Playback and scrubbing reuse a short frame cache (one ffmpeg job covers a window of frames) instead of spawning a process per pixel. The timeline scrolls horizontally. Scroll pans; Ctrl+scroll or a pinch zooms around the cursor. `+` / `−` zoom, and Shift+Z fits the sequence.

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
| Home / End | Go to start / end |
| I / O | Mark in / out |
| M | Add marker |
| V | Select tool |
| C or Ctrl+K | Razor at the playhead |
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
| Ctrl+scroll or pinch | Zoom timeline |
| Scroll | Pan timeline |
| Shift+Z | Zoom timeline to fit |

Reverse shuttle and rates other than 1× play the picture and stay silent. At 1×, the ffmpeg build decodes interleaved stereo PCM at 48 kHz in about two-second chunks and plays it through the default ALSA device (rodio). The viewer shows a stereo meter and a badge: `Audio`, `Buffering`, `Silent`, `No device`, or `No audio`. A missing device does not stop the picture. The default build (no `ffmpeg` feature) does not link an audio backend; its badge is `No audio` and the status line says to rebuild with `--features ffmpeg`.

Colour and transform parameters are `AnimatedF32`: a constant until you add a keyframe, then linear or hold interpolation in clip-relative frames. The inspector diamond toggles a key at the playhead. The Colour workspace adds an offset pad for temperature and tint.

Transitions (cross dissolve, wipe, push) are centered on a cut and consume head and tail handles. Duration is in sequence frames.

## Workspaces

- **Edit** — media pool, program viewer, inspector, captions, timeline
- **Colour** — viewer plus grade, wheels, and transform
- **Deliver** — codec, container, and in/out. **Write Manifest** saves a JSON export plan. This build does not encode pictures

The program monitor draws mute/solo, grade, transform, dissolve, wipe, push, and caption burn-in as proxy cards. With `--features ffmpeg` it replaces that stack with a decoded frame of the topmost visible video clip under the playhead. Play, keyboard stepping, and mouse scrubbing all follow the sequence timebase. If ffmpeg is missing or the file is offline, the proxy stays up and the viewer says why.

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

`editor_media::probe` returns duration, resolution, codecs, and whether the file is offline. `editor_media::decode_frames` turns a timestamp into RGBA for the program viewer.

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

`cargo run` and `cargo test` without the feature stay on the proxy and do not need ffmpeg installed.

The example sequence *Northline — Opening* points its three picture clips at synthetic files in [`samples/media/`](samples/media/):

| File | What you see | Length |
| --- | --- | --- |
| `interview.mp4` | `testsrc2` card with a running clock | 480 frames, 24 fps |
| `city_broll.mp4` | classic `testsrc` card | 288 frames, 24 fps |
| `aerial.mp4` | the same card with a moving hue | 192 frames, 24 fps |

They are generated, not third-party footage. Regenerate them with [`scripts/generate-sample-media.sh`](scripts/generate-sample-media.sh) if you have ffmpeg. Launching from the repo root (or any subdirectory) resolves those relative paths. The viewer picks the highest unmuted video track that covers the playhead — V2's aerial replaces V1 while that clip is on screen — and caches a short burst of frames so playback and scrubbing do not spawn ffmpeg once per frame.

**File → Import** probes any file `ffprobe` can open and writes the absolute path into the project. Offline media and a missing `ffmpeg` binary leave the shell usable and put the reason on the program monitor and on the clip.

## Captions

Each sequence can carry a caption track of `CaptionCue`s. **Auto Caption** calls a `CaptionTranscriber`. The shipping implementation is `StubTranscriber`: it slices the selected range into roughly two-second cues and cycles a fixed phrase list. No API key, no network.

To attach a real engine, implement the trait and construct the app with your type (the app currently stores `StubTranscriber` directly; widening that field to `Box<dyn CaptionTranscriber>` is the intended seam):

```rust
use editor_core::{CaptionDraft, CaptionError, CaptionTranscriber, TranscribeRequest};

struct WhisperTranscriber { /* model path or client */ }

impl CaptionTranscriber for WhisperTranscriber {
    fn transcribe(&mut self, request: &TranscribeRequest) -> Result<Vec<CaptionDraft>, CaptionError> {
        // Local: whisper.cpp / whisper-rs over the media range.
        // Cloud: send the same range and map word timings back onto `request.timebase`.
        let _ = request;
        Err(CaptionError::Failed("not linked".into()))
    }
}
```

Map word timestamps through the sequence timebase into `Frame` in/out points. `replace_captions` writes the drafts onto the caption track in one undoable edit.

## Tests

`cargo test --workspace` covers timebase conversion and drop-frame timecode, overwrite, insert, razor, lift and ripple delete, move, trim, ripple, roll, slip, slide, transitions, keyframes, undo, templates, the sample project round-trip, imported media paths in JSON, the stub probe, still-image holds, the ffprobe JSON parser, and preview frame-request bounds. A real decode runs only when tests are built with `--features ffmpeg` and `ffmpeg` is on `PATH`; otherwise that test returns without spawning.

## Roadmap

- GPU viewer that composites more than the top decoded video track
- Fairlight-class audio: mixer and clip gain (1× playback and stereo meters are in the ffmpeg build)
- OFX-style plugins for third-party effects
- Deliver codecs that encode the manifest instead of only writing it
- Multi-cam: sync groups and angle switching
- More trim shortcuts, gang, and a command palette

## License

[MIT](LICENSE). Copyright 2026 Meridian Contributors.
