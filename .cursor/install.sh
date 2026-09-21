#!/usr/bin/env bash
#
# Idempotent Cloud Agent bootstrap for the rust-video-editor project.
#
# The base image already provides the Rust toolchain (rustc/cargo/rustup with
# clippy + rustfmt), the FFmpeg CLI, and a C/C++ build toolchain (gcc, clang,
# libclang, pkg-config, make). This script adds the system development
# libraries a "from scratch" Rust video editor needs:
#   * FFmpeg development headers/libs  -> media decode/encode/filtering
#     (used by crates such as ffmpeg-next / ffmpeg-sys-next)
#   * Windowing / GPU / input libs     -> winit + wgpu/eframe/egui style GUIs
#   * ALSA                             -> audio playback/capture (cpal, rodio)
# It then compiles the Cargo workspace if one exists yet.

set -euo pipefail

echo "==> rust-video-editor install: refreshing apt package index"
sudo apt-get update -y

echo "==> Installing system development libraries"
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    pkg-config \
    ca-certificates \
    clang \
    libclang-dev \
    libavcodec-dev \
    libavformat-dev \
    libavutil-dev \
    libavfilter-dev \
    libavdevice-dev \
    libswscale-dev \
    libswresample-dev \
    libx11-dev \
    libxcursor-dev \
    libxrandr-dev \
    libxi-dev \
    libxkbcommon-dev \
    libxkbcommon-x11-dev \
    libwayland-dev \
    libgl1-mesa-dev \
    libegl1-mesa-dev \
    libvulkan-dev \
    mesa-vulkan-drivers \
    libgtk-3-dev \
    libasound2-dev \
    libudev-dev

echo "==> Toolchain versions"
rustc --version
cargo --version
echo -n "ffmpeg: "; ffmpeg -version | head -n 1
echo -n "libavcodec (pkg-config): "; pkg-config --modversion libavcodec

# Build the project only once real Cargo sources exist. Guarded so the script
# stays valid on the current (code-free) revision and on future branches.
if [ -f "Cargo.toml" ]; then
    echo "==> Cargo.toml found: fetching dependencies and building"
    cargo fetch --locked 2>/dev/null || cargo fetch
    cargo build
else
    echo "==> No Cargo.toml yet; skipping cargo build (environment tooling is ready)"
fi

echo "==> install complete"
