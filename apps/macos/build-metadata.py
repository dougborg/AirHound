#!/usr/bin/env python3
"""Stamp GUI source provenance without implying a published release."""
from pathlib import Path
import plistlib
import subprocess
import sys
import tomllib


def git(root: Path, *arguments: str):
    try:
        result = subprocess.run(
            ["git", "-C", str(root), *arguments], capture_output=True, text=True, check=True
        )
        return result.stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None


def metadata(root: Path):
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    result = {
        "CFBundleShortVersionString": version,
        "AirHoundPackageVersion": version,
        "AirHoundBuildComponent": "macos_gui",
        "AirHoundBuildSchemaVersion": 1,
    }
    revision = git(root, "rev-parse", "HEAD")
    count = git(root, "rev-list", "--count", "HEAD")
    result["CFBundleVersion"] = count if count and count.isdigit() else "0"
    if revision:
        result["AirHoundSourceRevision"] = revision
        tag = git(root, "describe", "--tags", "--exact-match", "HEAD")
        if tag:
            result["AirHoundSourceTag"] = tag
        status = git(root, "status", "--porcelain", "--untracked-files=no")
        if status is not None:
            result["AirHoundTrackedSourceModified"] = bool(status)
    return result


def main():
    destination = Path(sys.argv[1])
    info = plistlib.loads(destination.read_bytes())
    info.update(metadata(Path(__file__).resolve().parents[2]))
    destination.write_bytes(plistlib.dumps(info, sort_keys=False))


if __name__ == "__main__":
    main()
