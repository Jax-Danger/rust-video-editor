#!/usr/bin/env bash
# Build whisper.cpp's whisper-cli and download ggml-tiny.en into ~/.cache/whisper.
# Meridian does not link this library. cargo run -p editor-app --features whisper
# finds the binary on PATH or at ~/.local/bin/whisper-cli.
set -euo pipefail

ROOT="${WHISPER_SRC:-$HOME/.cache/meridian/whisper.cpp}"
MODEL_DIR="${WHISPER_MODEL_DIR:-$HOME/.cache/whisper}"
MODEL_URL="${WHISPER_MODEL_URL:-https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin}"
BIN_DEST="${WHISPER_INSTALL:-$HOME/.local/bin/whisper-cli}"

if ! command -v cmake >/dev/null || ! command -v g++ >/dev/null; then
  echo "cmake and g++ are required. On Fedora: sudo dnf install cmake gcc gcc-c++ git" >&2
  echo "On Debian/Ubuntu: sudo apt install cmake g++ git" >&2
  exit 1
fi

if [[ ! -d "$ROOT/.git" ]]; then
  mkdir -p "$(dirname "$ROOT")"
  git clone --depth 1 https://github.com/ggml-org/whisper.cpp.git "$ROOT"
fi

cmake -S "$ROOT" -B "$ROOT/build" -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_C_COMPILER=gcc -DCMAKE_CXX_COMPILER=g++
cmake --build "$ROOT/build" -j --target whisper-cli

install -D "$ROOT/build/bin/whisper-cli" "$BIN_DEST"
mkdir -p "$MODEL_DIR"
if [[ ! -f "$MODEL_DIR/ggml-tiny.en.bin" ]]; then
  curl -L --fail -o "$MODEL_DIR/ggml-tiny.en.bin" "$MODEL_URL"
fi

echo "whisper-cli: $BIN_DEST"
echo "model:       $MODEL_DIR/ggml-tiny.en.bin"
echo "Run: WHISPER_BIN=$BIN_DEST cargo run -p editor-app --features whisper"
