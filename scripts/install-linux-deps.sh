#!/bin/bash
# Ubuntu 24.04 build dependencies shared by CI and both release architectures.
set -euo pipefail
sudo apt-get update
sudo apt-get install -y build-essential clang libclang-dev cmake ninja-build pkg-config dpkg-dev \
	libasound2-dev libpipewire-0.3-dev libdbus-1-dev libssl-dev \
	libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-dev libx11-xcb-dev \
	libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-randr0-dev libxcb-render0-dev \
	libxcb-xkb-dev libxrandr-dev libxi-dev libxcursor-dev libxinerama-dev libxrender-dev \
	libxfixes-dev libegl1-mesa-dev libfontconfig1-dev libfreetype6-dev libvulkan-dev
