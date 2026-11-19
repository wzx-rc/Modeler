#!/usr/bin/env bash
# OpenCascade 7.8.1 for iPad (arm64, iOS 16+), static libraries, only the
# modules Forme NURBS links (FoundationClasses, ModelingData,
# ModelingAlgorithms). Same sources as the PC build (the occt-sys 7.8.1
# crate: OCCT 7.8.1 and its module list patch).
# Usage: ios/build_occt_ios.sh [install dir]   (default ~/occt-ios)
# Needs macOS with Xcode and CMake. ~15 to 25 min on a GitHub Mac, cached
# by the workflow afterwards.
set -euo pipefail
OUT="${1:-$HOME/occt-ios}"
if [ -f "$OUT/lib/libTKOffset.a" ]; then
  echo "OCCT for iOS already in $OUT"
  exit 0
fi
WORK="${RUNNER_TEMP:-/tmp}/occt-ios-work"
SRC="$WORK/occt-sys"
if [ ! -d "$SRC/OCCT" ]; then
  mkdir -p "$SRC"
  curl -fsSL https://static.crates.io/crates/occt-sys/occt-sys-7.8.1.crate | tar xz -C "$SRC" --strip-components=1
fi
SDK="$(xcrun --sdk iphoneos --show-sdk-path)"
JOBS="$(sysctl -n hw.ncpu 2>/dev/null || echo 4)"
cmake -S "$SRC/OCCT" -B "$WORK/build" -G "Unix Makefiles" \
  -D CMAKE_SYSTEM_NAME=iOS \
  -D CMAKE_OSX_ARCHITECTURES=arm64 \
  -D CMAKE_OSX_DEPLOYMENT_TARGET=16.0 \
  -D CMAKE_OSX_SYSROOT="$SDK" \
  -D CMAKE_BUILD_TYPE=Release \
  -D CMAKE_C_USE_RESPONSE_FILE_FOR_OBJECTS=OFF \
  -D CMAKE_CXX_USE_RESPONSE_FILE_FOR_OBJECTS=OFF \
  -D BUILD_LIBRARY_TYPE=Static \
  -D BUILD_PATCH="$SRC/patch" \
  -D INSTALL_DIR="$OUT" \
  -D INSTALL_DIR_LIB=lib \
  -D INSTALL_DIR_INCLUDE=include \
  -D BUILD_MODULE_FoundationClasses=ON \
  -D BUILD_MODULE_ModelingData=ON \
  -D BUILD_MODULE_ModelingAlgorithms=ON \
  -D BUILD_MODULE_Visualization=OFF \
  -D BUILD_MODULE_ApplicationFramework=OFF \
  -D BUILD_MODULE_DataExchange=OFF \
  -D BUILD_MODULE_DETools=OFF \
  -D BUILD_MODULE_Draw=OFF \
  -D BUILD_DOC_Overview=OFF \
  -D USE_D3D=OFF -D USE_DRACO=OFF -D USE_EIGEN=OFF -D USE_FFMPEG=OFF -D USE_FREEIMAGE=OFF \
  -D USE_FREETYPE=OFF -D USE_GLES2=OFF -D USE_OPENGL=OFF -D USE_OPENVR=OFF -D USE_RAPIDJSON=OFF \
  -D USE_TBB=OFF -D USE_TCL=OFF -D USE_TK=OFF -D USE_VTK=OFF -D USE_XLIB=OFF
cmake --build "$WORK/build" --config Release -j "$JOBS"
cmake --install "$WORK/build" --config Release
ls "$OUT/lib"
