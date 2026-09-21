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
sudo dnf install gcc gcc-c++ cmake pkg-config gtk3-devel alsa-lib-devel ffmpeg \
  dejavu-sans-fonts libxkbcommon-x11 mesa-libGL mesa-libEGL pipewire-alsa
cargo run -p editor-app --features ffmpeg
```

`gtk3-devel` is required to compile: the open, save, and import dialogs link GTK 3. `alsa-lib-devel` is required only for `--features ffmpeg`, which is the build that plays audio and encodes. The default `cargo build` / `cargo test` does not link ALSA and does not spawn ffmpeg. Auto Caption with a local model is a separate build: `cargo run -p editor-app --features whisper` (that feature includes ffmpeg). `gcc-c++` and `cmake` are only for building whisper.cpp itself — use `g++`, not Clang, because the Cloud and Fedora Clang packages often cannot find `libstdc++`. `dejavu-sans-fonts` is what Export uses to burn captions into the picture.

### Debian / Ubuntu

```bash
sudo apt install gcc g++ cmake pkg-config libgtk-3-dev libasound2-dev ffmpeg \
  fonts-dejavu-core libxkbcommon-x11-0 libegl1 libgl1
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

Muted audio tracks, and every non-solo track while any audio track is soloed, stay out of the mix. Each clip has a **Clip gain** slider in the inspector (0 to 2 in the UI, stored up to 4). Playback, the meters, caption extraction, and export all use that gain. Meters are the peak of the chunk currently playing, so they move with the mix rather than with a single clip.

Colour and transform parameters are `AnimatedF32`: a constant until you add a keyframe, then linear or hold interpolation in clip-relative frames. The inspector diamond toggles a key at the playhead. The Colour workspace adds an offset pad for temperature and tint.

Transitions (cross dissolve, wipe, push) are centered on a cut and consume head and tail handles. Duration is in sequence frames.

## Workspaces

- **Edit** — media pool, program viewer, inspector, captions, timeline
- **Colour** — viewer plus grade, wheels, and transform
- **Deliver** — codec, container, in/out, and **Export**. With `--features ffmpeg` this encodes a real file. Without that feature, Export still writes the JSON manifest and says the encoder is compiled out.

The program monitor draws mute/solo, grade, transform, dissolve, wipe, push, and caption burn-in as proxy cards when decode is off. With `--features ffmpeg` it decodes every visible video layer under the playhead and composites them on the CPU: the same grade formula as the proxy cards, plus scale, position, rotation, anchor, opacity, a left wipe, and a push. Dissolves update every frame. Play, keyboard stepping, and mouse scrubbing all follow the sequence timebase. If ffmpeg is missing or every layer is offline, the proxy stays up and the viewer says why.

What the preview still does not do: blend modes, motion blur, and a true wipe angle (wipes are always left-to-right). Decoded frames are fit into the preview box before the transform, so a source that is not the sequence aspect is letterboxed by ffmpeg and then scaled again by the clip transform. Export samples a keyed grade or transform about every six frames, and its grade filters are an ffmpeg approximation of the preview formula (exposure, eq, colorbalance). The preview formula and the export filters will not match pixel for pixel.

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

They are generated, not third-party footage. Regenerate them with [`scripts/generate-sample-media.sh`](scripts/generate-sample-media.sh) if you have ffmpeg. Launching from the repo root (or any subdirectory) resolves those relative paths. The viewer stacks every visible video layer, so V2's keyed aerial sits on top of V1 instead of replacing it, and caches a short burst of frames per layer so playback and scrubbing do not spawn ffmpeg once per frame.

**File → Import** probes any file `ffprobe` can open and writes the absolute path into the project. Offline media and a missing `ffmpeg` binary leave the shell usable and put the reason on the program monitor and on the clip.

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

The graph is the sequence, or the marked in/out when that checkbox is on:

- Visible video tracks, bottom to top. Muted video is black. Solo hides the other video tracks.
- A cross dissolve, left wipe, or left push that fills a slice uses ffmpeg `xfade`. A slice that only overlaps part of a transition holds the midpoint opacity.
- Scale, position, rotation, and opacity are applied. Rotation is around the center. Anchor is preview-only.
- Grade is an ffmpeg approximation (exposure, contrast, saturation, shadow/highlight lift, temperature and tint). Neutral grades are omitted.
- Audio is mixed with each clip's gain. Offline audio is skipped and named in the report. Offline video that is actually visible fails the export.
- Captions are burned with `drawtext` when a DejaVu, Liberation, FreeSans, or Arial font is installed, and written as `mov_text` soft subtitles on mp4 and mov. Uncheck **Burn captions into the picture** to keep them soft only. MXF does not get the soft-sub input.

A progress bar follows ffmpeg's `out_time`. **Cancel** sends `SIGTERM`. A failed or cancelled encode deletes the partial file. Without `--features ffmpeg`, Export writes the JSON manifest instead and explains how to rebuild.

## Tests

`cargo test --workspace` covers timebase conversion and drop-frame timecode, overwrite, insert, razor, lift and ripple delete, move, trim, ripple, roll, slip, slide, transitions, keyframes, undo, templates, the sample project round-trip, imported media paths in JSON, the stub probe, still-image holds, the ffprobe JSON parser, preview frame-request bounds, caption JSON parsing, the export filter graph (grade, overlay, dissolve, gain, burned captions), and the preview composite. It does not spawn ffmpeg or whisper.

`cargo test -p editor-media --features ffmpeg` also encodes a short H.264/AAC mp4 when `ffmpeg` is on `PATH`. `cargo test -p editor-app --features whisper` builds the local speech-to-text path; the binary and model are resolved at runtime, not at compile time.

## Roadmap

- GPU viewer. The CPU composite already stacks tracks, grades, transforms, and the three transitions; it is not a full optical-flow or blend-mode engine
- Fairlight-class mixing beyond mute, solo, and clip gain
- OFX-style plugins for third-party effects
- Export that matches the preview grade formula per frame, including anchor and wipe angle
- Multi-cam: sync groups and angle switching
- More trim shortcuts, gang, and a command palette

## License

[MIT](LICENSE). Copyright 2026 Meridian Contributors.
