#!/usr/bin/env bash
# Runs FormeNURBS.app on an iPad simulator with its scripted steps
# (FORME_SIM=1, crates/nurbs-app/src/sim.rs) and takes a screenshot after
# each one, into target/sim-shots (with the app's log).
# Usage: ios/sim_run.sh [device name]   (default: the first "iPad Pro 11")
set -uo pipefail
cd "$(dirname "$0")/.."
APP=target/sim-nurbs/FormeNURBS.app
OUT=target/sim-shots
BUNDLE=com.forme.nurbs
rm -rf "$OUT"; mkdir -p "$OUT"
xcrun simctl list devices available > "$OUT/devices.txt"
NAME="${1:-}"
if [ -z "$NAME" ]; then
  NAME="$(grep -o 'iPad Pro 11-inch ([^)]*)' "$OUT/devices.txt" | head -1)"
fi
[ -z "$NAME" ] && NAME="$(grep -o 'iPad Pro[^(]*([^)]*)' "$OUT/devices.txt" | head -1)"
UDID="$(grep -F "$NAME (" "$OUT/devices.txt" | head -1 | grep -oE '[0-9A-F-]{36}')"
echo "device: $NAME $UDID"
xcrun simctl boot "$UDID" || true
xcrun simctl bootstatus "$UDID" -b
# landscape (the app is landscape only)
xcrun simctl ui "$UDID" appearance dark || true
xcrun simctl install "$UDID" "$APP"
SIMCTL_CHILD_FORME_SIM=1 SIMCTL_CHILD_RUST_BACKTRACE=1 xcrun simctl launch --console-pty "$UDID" "$BUNDLE" > "$OUT/console.txt" 2>&1 &
sleep 3
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
echo "data: $DATA"
last=""
for i in $(seq 1 900); do
  cur="$(cat "$DATA/Documents/sim-step.txt" 2>/dev/null || true)"
  if [ -n "$cur" ] && [ "$cur" != "$last" ]; then
    last="$cur"
    n="$(echo "$cur" | tr ' ' '-' | tr -d '\n')"
    xcrun simctl io "$UDID" screenshot "$OUT/$n.png" >/dev/null 2>&1 && echo "shot $n"
    case "$cur" in *end*) break ;; esac
  fi
  sleep 0.2
done
cp "$DATA/Documents/sim-log.txt" "$OUT/" 2>/dev/null || true
cp "$DATA/Documents/crash.log" "$OUT/" 2>/dev/null || true
cp "$DATA/Documents/journal.jsonl" "$OUT/" 2>/dev/null || true
xcrun simctl io "$UDID" screenshot "$OUT/final.png" >/dev/null 2>&1 || true
ls -la "$OUT"
