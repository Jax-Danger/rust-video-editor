#!/bin/sh
# Regenerate the synthetic H.264 sample clips under samples/media.
# Requires ffmpeg (with libx264 and aac) on PATH. The pictures are lavfi
# test sources — no third-party footage.
set -eu
cd "$(dirname "$0")/.."
mkdir -p samples/media

ffmpeg -y -hide_banner -loglevel error \
  -f lavfi -i "testsrc2=size=960x540:rate=24" \
  -f lavfi -i "sine=frequency=220:sample_rate=48000" \
  -frames:v 480 -c:v libx264 -pix_fmt yuv420p -preset veryfast -crf 26 -g 12 \
  -c:a aac -b:a 64k -shortest \
  samples/media/interview.mp4

ffmpeg -y -hide_banner -loglevel error \
  -f lavfi -i "testsrc=size=960x540:rate=24" \
  -f lavfi -i "sine=frequency=330:sample_rate=48000" \
  -frames:v 288 -c:v libx264 -pix_fmt yuv420p -preset veryfast -crf 26 -g 12 \
  -c:a aac -b:a 64k -shortest \
  samples/media/city_broll.mp4

ffmpeg -y -hide_banner -loglevel error \
  -f lavfi -i "testsrc2=size=960x540:rate=24,hue=H=2*PI*t" \
  -f lavfi -i "sine=frequency=440:sample_rate=48000" \
  -frames:v 192 -c:v libx264 -pix_fmt yuv420p -preset veryfast -crf 26 -g 12 \
  -c:a aac -b:a 64k -shortest \
  samples/media/aerial.mp4

echo "wrote samples/media/{interview,city_broll,aerial}.mp4"
