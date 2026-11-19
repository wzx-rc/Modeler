#!/usr/bin/env bash
# Builds an unsigned "Forme NURBS.ipa" for iPad Pro (to sideload with
# Sideloadly or AltStore and an Apple ID). The OpenCascade kernel is linked
# in the app (iOS has no child processes): OCCT for iOS must be built first
# (ios/build_occt_ios.sh, FORME_OCCT_ROOT, default ~/occt-ios).
# Needs macOS with Xcode and the Rust target aarch64-apple-ios.
set -euo pipefail
cd "$(dirname "$0")/.."

export IPHONEOS_DEPLOYMENT_TARGET=16.0
export FORME_OCCT_ROOT="${FORME_OCCT_ROOT:-$HOME/occt-ios}"
rustup target add aarch64-apple-ios
cargo build --release --target aarch64-apple-ios -p nurbs-app --bin forme-nurbs

OUT=target/ipa-nurbs
APP="$OUT/Payload/FormeNURBS.app"
rm -rf "$OUT"
mkdir -p "$APP"
cp target/aarch64-apple-ios/release/forme-nurbs "$APP/forme-nurbs"
cp ios/nurbs/Info.plist "$APP/Info.plist"
cp ios/nurbs/AppIcon*.png "$APP/"
printf 'APPL????' > "$APP/PkgInfo"
# ad-hoc signature: Sideloadly / AltStore re-sign with the Apple ID anyway
codesign --force --sign - --timestamp=none "$APP" || true

(cd "$OUT" && zip -qr "FormeNURBS.ipa" Payload)
ls -la "$OUT/FormeNURBS.ipa"
