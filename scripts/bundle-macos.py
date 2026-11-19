#!/usr/bin/env python3
"""Assemble the already-built macOS executable into an identifiable app bundle."""
from pathlib import Path
import plistlib
import shutil
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    if sys.platform != 'darwin':
        raise SystemExit('The AirHound app bundle is only supported on macOS')
    binary = ROOT / 'hostd/target/release/airhound-hostd'
    if not binary.is_file():
        raise SystemExit('Build first: just build-hostd (or just build-hostd-public-api)')
    app = ROOT / 'hostd/AirHound.app'
    executable = app / 'Contents/MacOS/airhound-hostd'
    if app.exists():
        shutil.rmtree(app)
    executable.parent.mkdir(parents=True)
    shutil.copy2(ROOT / 'hostd/macos/Info.plist', app / 'Contents/Info.plist')
    shutil.copy2(binary, executable)
    with (app / 'Contents/Info.plist').open('rb') as stream:
        assert plistlib.load(stream)['CFBundleExecutable'] == executable.name
    assert not executable.is_symlink()
    assert executable.read_bytes() == binary.read_bytes()
    print(app)


if __name__ == '__main__':
    main()
