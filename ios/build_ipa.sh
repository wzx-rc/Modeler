#!/usr/bin/env bash
# Builds an unsigned Forme.ipa for iPad (to sideload with AltStore or Sideloadly).
# Needs macOS with Xcode command line tools and the Rust target aarch64-apple-ios.
set -euo pipefail
cd "$(dirname "$0")/.."

export IPHONEOS_DEPLOYMENT_TARGET=16.0
rustup target add aarch64-apple-ios
cargo build --release --target aarch64-apple-ios -p forme-app --bin forme

OUT=target/ipa
APP="$OUT/Payload/Forme.app"
rm -rf "$OUT"
mkdir -p "$APP"
cp target/aarch64-apple-ios/release/forme "$APP/forme"
cp ios/Info.plist "$APP/Info.plist"
cp ios/AppIcon*.png "$APP/"
printf 'APPL????' > "$APP/PkgInfo"
# ad-hoc signature: sideloading tools re-sign with your Apple ID anyway
codesign --force --sign - --timestamp=none "$APP" || true

(cd "$OUT" && zip -qr Forme.ipa Payload)
echo "→ $OUT/Forme.ipa"
