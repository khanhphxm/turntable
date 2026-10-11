#!/bin/sh
# Deterministic stereo music-like test PCM, encoded by real codecs; no network or playback.
set -eu
out=${1:?usage: audio-fixtures.sh OUTPUT_DIRECTORY [SECONDS]}
seconds=${2:-30}
mkdir -p "$out"
ffmpeg -hide_banner -loglevel error -y -f lavfi \
  -i "aevalsrc=(0.17+0.13*sin(2*PI*0.7*t))*sin(2*PI*440*t)+0.12*sin(2*PI*93*t)+0.06*sin(2*PI*17003*t)| (0.16+0.12*cos(2*PI*0.9*t))*sin(2*PI*661*t)+0.11*sin(2*PI*117*t)+0.06*sin(2*PI*15997*t):s=44100:d=$seconds" \
  -c:a pcm_s24le "$out/source.wav"
ffmpeg -hide_banner -loglevel error -y -i "$out/source.wav" -c:a libvorbis -q:a 8 "$out/vorbis.ogg"
ffmpeg -hide_banner -loglevel error -y -i "$out/source.wav" -c:a libmp3lame -b:a 320k "$out/mp3.mp3"
ffmpeg -hide_banner -loglevel error -y -i "$out/source.wav" -c:a flac "$out/flac.flac"
