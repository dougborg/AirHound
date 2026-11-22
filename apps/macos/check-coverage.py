#!/usr/bin/env python3
"""Gate Foundation presentation logic separately from SwiftUI rendering coverage."""
import json
import sys
from pathlib import Path

report = json.loads(Path(sys.argv[1]).read_text())
files = {
    entry["filename"]: entry["summary"]["lines"]
    for data in report["data"]
    for entry in data["files"]
    if "/Sources/AirHoundCore/" in entry["filename"]
}
if not files:
    raise SystemExit("No AirHoundCore production coverage found")
covered = sum(entry["covered"] for entry in files.values())
count = sum(entry["count"] for entry in files.values())
percent = 100 * covered / count if count else 0
print(f"AirHoundCore production line coverage: {covered}/{count} ({percent:.2f}%)")
print("This excludes SwiftUI rendering and does not establish UI or hardware acceptance.")
if percent < 80:
    raise SystemExit("AirHoundCore line coverage must be at least 80%")
