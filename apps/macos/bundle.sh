#!/bin/sh
set -eu
cd "$(dirname "$0")"
swift build -c release
bin_path=$(swift build -c release --show-bin-path)
app=dist/AirHound.app
# Only replace this script's generated bundle; avoid stale resources/signatures.
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin_path/AirHound" "$app/Contents/MacOS/AirHound"
# The app uses its own standard Resources directory; swift run uses Bundle.module.
cp Sources/AirHoundApp/Resources/example.ndjson "$app/Contents/Resources/example.ndjson"
cp Info.plist "$app/Contents/Info.plist"
codesign --force --sign - "$app"
printf 'Built %s/%s\n' "$PWD" "$app"
