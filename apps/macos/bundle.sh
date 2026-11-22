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
# Pass a built backend explicitly; never embed a stale binary by guessing a path.
if [ "$#" -gt 0 ]; then
  test -x "$1" || { printf 'Backend is not executable: %s\n' "$1" >&2; exit 1; }
  service="$app/Contents/MacOS/AirHound Service.app"
  mkdir -p "$service/Contents/MacOS"
  cp "$1" "$service/Contents/MacOS/airhound-hostd"
  cp ../../hostd/macos/Info.plist "$service/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c 'Set :CFBundleIdentifier tools.airhound.service' "$service/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c 'Set :CFBundleName AirHound Service' "$service/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c 'Add :CFBundleDisplayName string AirHound Service' "$service/Contents/Info.plist"
  codesign --force --sign - "$service"
  # Preserve the established developer CLI path; service startup resolves it
  # to the enclosing application and uses LaunchServices.
  mkdir -p "$app/Contents/Helpers"
  ln -s '../MacOS/AirHound Service.app/Contents/MacOS/airhound-hostd' "$app/Contents/Helpers/airhound-hostd"
fi
codesign --force --sign - "$app"
printf 'Built %s/%s\n' "$PWD" "$app"
