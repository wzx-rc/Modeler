#!/usr/bin/env bash
# Builds FormeNURBS.app for Apple's iPad simulator (arm64), to check the
# iPad version on a real iOS before an .ipa (the workflow ipad-sim.yml).
# OCCT for the simulator first: ios/build_occt_ios.sh with the SDK
# iphonesimulator (the workflow does it; FORME_OCCT_ROOT, default ~/occt-sim).
set -euo pipefail
cd "$(dirname "$0")/.."

export IPHONEOS_DEPLOYMENT_TARGET=16.0
export FORME_OCCT_ROOT="${FORME_OCCT_ROOT:-$HOME/occt-sim}"
rustup target add aarch64-apple-ios-sim
cargo build --release --target aarch64-apple-ios-sim -p nurbs-app --bin forme-nurbs

OUT=target/sim-nurbs
APP="$OUT/FormeNURBS.app"
rm -rf "$OUT"
mkdir -p "$APP"
cp target/aarch64-apple-ios-sim/release/forme-nurbs "$APP/forme-nurbs"
cp ios/nurbs/Info.plist "$APP/Info.plist"
plutil -replace CFBundleSupportedPlatforms -json '["iPhoneSimulator"]' "$APP/Info.plist"
cp ios/nurbs/AppIcon*.png "$APP/"
printf 'APPL????' > "$APP/PkgInfo"
codesign --force --sign - --timestamp=none "$APP"
ls -la "$APP"
