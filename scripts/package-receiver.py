#!/usr/bin/env python3
"""Build an unsigned receiver candidate with vendored corresponding source.

No firmware is acquired or bundled. Public release/signing is a separate gate.
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
CRATE = ROOT / "drivers/mt7610u"


def run(args, *, cwd=ROOT, capture=False, env=None):
    return subprocess.run(args, cwd=cwd, check=True, text=True,
                          stdout=subprocess.PIPE if capture else None,
                          env=env).stdout


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def check_macos_dependencies(output):
    """Only macOS-provided libraries may remain external to a candidate."""
    libraries = [line.strip().split(" (", 1)[0] for line in output.splitlines()[1:]
                 if line.strip()]
    unexpected = [lib for lib in libraries
                  if not lib.startswith(("/usr/lib/", "/System/Library/"))]
    if unexpected:
        raise ValueError("candidate depends on non-system libraries: " + ", ".join(unexpected))


def vendor_config(text):
    parsed = tomllib.loads(text)
    sources = parsed.get("source", {})
    directories = [name for name, spec in sources.items() if "directory" in spec]
    if len(directories) != 1:
        raise ValueError("expected one vendored dependency source")
    # Cargo's generated config can contain an absolute staging-directory path.
    lines = text.splitlines()
    replaced = 0
    for i, line in enumerate(lines):
        if line.strip().startswith("directory ="):
            lines[i] = 'directory = "vendor"'
            replaced += 1
    if replaced != 1:
        raise ValueError("ambiguous vendored dependency directory")
    result = "\n".join(lines) + "\n"
    tomllib.loads(result)
    return result


def source_paths():
    # Explicit tracked-source list prevents local firmware/capture/target leaks.
    output = run(["git", "ls-files", "-z", "drivers/mt7610u"], capture=True)
    files = [Path(name) for name in output.split("\0") if name]
    for path in files:
        relative = path.relative_to("drivers/mt7610u")
        if any(part in {"target", "captures", "firmware", ".git"} for part in relative.parts):
            raise ValueError("unexpected generated or firmware path in source list")
        if (ROOT / path).is_symlink():
            raise ValueError("source symlinks are not supported")
    return files


def archive(directory, destination):
    with tarfile.open(destination, "w:gz") as tar:
        tar.add(directory, arcname=directory.name)


def build(output_dir):
    if platform.system() != "Darwin":
        raise ValueError("initial release candidates target macOS only")
    if run(["git", "status", "--porcelain", "--untracked-files=normal"], capture=True).strip():
        raise ValueError("commit the source tree before building a release candidate")
    # Never overwrite an earlier candidate or mingle provenance.
    output_dir.mkdir(parents=True, exist_ok=False)
    commit = run(["git", "rev-parse", "HEAD"], capture=True).strip()
    version = tomllib.loads((CRATE / "Cargo.toml").read_text())["package"]["version"]
    name = f"mt7610u-lab-{version}-{commit[:12]}-macos-{platform.machine()}"
    with tempfile.TemporaryDirectory(prefix="receiver-package-") as temp:
        staging = Path(temp)
        source = staging / f"{name}-source"
        source.mkdir()
        for path in source_paths():
            destination = source / path.relative_to("drivers/mt7610u")
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / path, destination)
        config = run(["cargo", "vendor", "--locked", "--versioned-dirs",
                      str(source / "vendor")], cwd=CRATE, capture=True)
        (source / ".cargo").mkdir(exist_ok=True)
        (source / ".cargo/config.toml").write_text(vendor_config(config))
        (source / "BUILD-SOURCE.txt").write_text(
            "This is the source used for the accompanying experimental binary.\n"
            "With stable Rust and macOS Command Line Tools installed, run:\n"
            "  cargo build --release --locked --offline\n"
            "Dependency sources and their license files are in vendor/.\n"
            "Firmware is not included. See FIRMWARE_LICENSE and README.md.\n")
        target = staging / "build"
        # Build the executable from the exact source bundle, fully offline.
        run(["cargo", "build", "--release", "--locked", "--offline",
             "--target-dir", str(target)], cwd=source)
        binary = target / "release/mt7610u-lab"
        links = run(["otool", "-L", str(binary)], capture=True)
        check_macos_dependencies(links)
        run([str(binary), "--help"], capture=True)
        payload = staging / name
        (payload / "bin").mkdir(parents=True)
        shutil.copy2(binary, payload / "bin/mt7610u-lab")
        for notice in ["LICENSE", "THIRD_PARTY_NOTICES", "FIRMWARE_LICENSE", "README.md"]:
            shutil.copy2(source / notice, payload / notice)
        # Retain libusb's LGPL notice alongside the executable, not only in source.
        libusb_notice = list((source / "vendor").glob("libusb1-sys-*/libusb/COPYING"))
        if len(libusb_notice) != 1:
            raise ValueError("cannot identify bundled libusb license")
        shutil.copy2(libusb_notice[0], payload / "LIBUSB-LICENSE")
        for notice in (source / "vendor").rglob("*"):
            if notice.is_file() and notice.name.upper().startswith(("LICENSE", "COPYING", "NOTICE")):
                destination = payload / "dependency-notices" / notice.relative_to(source / "vendor")
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(notice, destination)
        source_archive = output_dir / f"{name}-source.tar.gz"
        archive(source, source_archive)
        manifest = {
            "schema_version": 1, "status": "experimental_unsigned_candidate",
            "git_commit": commit, "version": version, "architecture": platform.machine(),
            "rustc": run(["rustc", "--version"], capture=True).strip(),
            "binary_sha256": sha256(binary), "source_archive": source_archive.name,
            "source_sha256": sha256(source_archive), "source_built_offline": True,
            "developer_id_signed": False, "notarized": False,
            "firmware_bundled": False,
            "release_gates": "See repository docs/receiver-release-gates.md; not approved for production",
        }
        (payload / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        binary_archive = output_dir / f"{name}.tar.gz"
        archive(payload, binary_archive)
        checksums = {path.name: sha256(path) for path in (binary_archive, source_archive)}
        (output_dir / "SHA256SUMS").write_text(
            "".join(f"{digest}  {name}\n" for name, digest in sorted(checksums.items())))
    print(json.dumps({"output_dir": str(output_dir), "artifacts": checksums,
                      "production_ready": False}, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    build(args.output_dir.resolve())


if __name__ == "__main__":
    main()
