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
cargo test
cargo run
```

`editor-app` is the default workspace member, so `cargo run` from the repository root launches Meridian. The window title is `Meridian — <project>`.

On Linux the window needs an X11 or Wayland session plus `libxkbcommon-x11`, `libEGL`, and `libGL`. On Debian or Ubuntu:

```bash
sudo apt install libxkbcommon-x11-0 libegl1 libgl1
```

The first launch loads an in-memory example: three picture tracks, linked interview audio, a cross dissolve, a wipe, keyframed picture-in-picture, captions, and markers. **File → Open Example** returns to it. **File → Save** writes pretty JSON. A copy of that example lives at [`samples/northline-opening.json`](samples/northline-opening.json).

## Editing

The toolbar exposes Select, Razor, Ripple, Roll, Slip, and Slide, plus Overwrite and Insert. Track headers have lock, mute, and solo. Snapping is on by default.

| Gesture | Result |
| --- | --- |
| Overwrite | Places the selected pool item at the playhead, trimming or splitting what it covers |
| Insert | Same, then ripples later clips on sync-locked tracks |
| Razor (`C` or Ctrl+K) | Splits the clip under the pointer and its linked partner; transitions follow the right half |
| Delete | Lift (leaves a gap). Shift+Delete ripple-deletes |
| Drag body | Move (lift, then overwrite) |
| Drag edge | Trim. Ripple, roll, slip, and slide follow the active tool |
| Double-click pool item | Overwrite at the playhead |

Linked selection is on by default so picture and sound move together. Inserting a linked pair is one ripple, not two.

Colour and transform parameters are `AnimatedF32`: a constant until you add a keyframe, then linear or hold interpolation in clip-relative frames. The inspector diamond toggles a key at the playhead. The Colour workspace adds an offset pad for temperature and tint.

Transitions (cross dissolve, wipe, push) are centered on a cut and consume head and tail handles. Duration is in sequence frames.

## Workspaces

- **Edit** — media pool, program viewer, inspector, captions, timeline
- **Colour** — viewer plus grade, wheels, and transform
- **Deliver** — codec, container, and in/out. **Write Manifest** saves a JSON export plan. This build does not encode pictures

The program monitor draws mute/solo, grade, transform, dissolve, wipe, push, and caption burn-in as proxy cards. With `--features ffmpeg` it replaces that stack with a decoded frame of the topmost visible video clip under the playhead. Play and scrub both follow the sequence timebase. If ffmpeg is missing or the file is offline, the proxy stays up and the viewer says why.

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

They are generated, not third-party footage. Regenerate them with [`scripts/generate-sample-media.sh`](scripts/generate-sample-media.sh) if you have ffmpeg. Launching from the repo root (or any subdirectory) resolves those relative paths. The viewer picks the highest unmuted video track that covers the playhead — V2's aerial replaces V1 while that clip is on screen — and caches a short burst of frames so playback does not spawn ffmpeg once per frame.

Import still accepts any path `ffprobe` can open. Offline media and a missing `ffmpeg` binary leave the shell usable and put the reason on the program monitor.

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

`cargo test` covers timebase conversion and drop-frame timecode, overwrite, insert, razor, lift and ripple delete, move, trim, ripple, roll, slip, slide, transitions, keyframes, undo, templates, the sample project round-trip, the stub probe, the ffprobe JSON parser, and preview frame-request bounds. A real decode runs only when tests are built with `--features ffmpeg` and `ffmpeg` is on `PATH`; otherwise that test returns without spawning.

## Roadmap

- GPU viewer that composites more than the top decoded video track
- Fairlight-class audio: mixer, metering, and clip gain
- OFX-style plugins for third-party effects
- Deliver codecs that encode the manifest instead of only writing it
- Multi-cam: sync groups and angle switching
- Keyboard-first editing: more trim shortcuts, gang, and a command palette

## License

[MIT](LICENSE). Copyright 2026 Meridian Contributors.
