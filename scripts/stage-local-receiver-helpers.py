#!/usr/bin/env python3
"""Stage explicitly selected LOCAL receiver assets for airhound-helper-setup.

No downloads, hardware access, or firmware updates. The selected Python runtime
reports its version/base-prefix and runs this repository's helper capability query
(which never imports upstream modules or opens a device).
This local-use bundle is not a redistributable release package (see #268).
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import os
import select
import time
import shlex
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def sha(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for block in iter(lambda: f.read(65536), b''):
            h.update(block)
    return h.hexdigest()


def copy(source, target):
    if source.is_symlink() or not source.is_file():
        raise ValueError('expected nonsymlink regular input: ' + str(source))
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, target)


def tree(source, target):
    for file in sorted(source.rglob('*')):
        if any(p in ('target', '.git', '__pycache__') for p in file.relative_to(source).parts):
            continue
        if file.is_symlink():
            raise ValueError('source symlink is not supported: ' + str(file))
        if file.is_file() and file.suffix != '.pyc':
            copy(file, target / file.relative_to(source))


def helper_capabilities(python, helper):
    """Explicit setup inspection, bounded independently of untrusted output."""
    process = subprocess.Popen([str(python), '-I', '-B', str(helper), '--capabilities'],
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    output = bytearray()
    deadline = time.monotonic() + 3
    try:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise ValueError('helper capability query timed out')
            if not select.select([process.stdout], [], [], remaining)[0]:
                raise ValueError('helper capability query timed out')
            chunk = os.read(process.stdout.fileno(), 4097 - len(output))
            if not chunk:
                break
            output.extend(chunk)
            if len(output) > 4096:
                raise ValueError('helper capability report exceeds limit')
        if process.wait(timeout=max(.01, deadline-time.monotonic())) != 0:
            raise ValueError('helper capability query failed')
        report = json.loads(output)
        required = {'borrowed_serial_lease_v1', 'structured_errors_v1'}
        capabilities = report.get('capabilities')
        if (report.get('type') != 'sniffle_helper_capabilities' or report.get('schema_version') != 1
                or not isinstance(capabilities, list) or len(capabilities) > 16
                or any(not isinstance(v, str) or not v or len(v) > 64 or not v.isascii()
                       or any(not (c.isalnum() or c in '_-') for c in v) for v in capabilities)
                or not required.issubset(capabilities)):
            raise ValueError('helper lacks supervised recovery capabilities')
        return capabilities
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=3)
        process.stdout.close()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--sniffle-source', type=Path, required=True)
    p.add_argument('--python', type=Path, required=True)
    p.add_argument('--site-packages', type=Path, required=True)
    p.add_argument('--pyserial-license', type=Path, required=True)
    p.add_argument('--alfa-executable', type=Path, required=True)
    p.add_argument('--alfa-source', type=Path, required=True)
    p.add_argument('--firmware', type=Path, required=True)
    p.add_argument('--libusb-license', type=Path, required=True)
    args = p.parse_args()
    out = args.output.resolve()
    if out.exists():
        raise ValueError('output already exists')
    python = args.python.resolve(strict=True)
    if any(python.is_relative_to(Path(x)) for x in ('/tmp', '/private/tmp', '/var/folders', '/private/var/folders')):
        raise ValueError('use a stable base interpreter, not a temporary venv')
    runtime = json.loads(subprocess.run([str(python), '-I', '-B', '-c',
        'import sys,json;print(json.dumps({"version":sys.version,"base_prefix":sys.base_prefix}))'],
        check=True, capture_output=True, text=True, timeout=10).stdout)
    def revision(path):
        if subprocess.run(['git', '-C', str(path), 'status', '--porcelain', '--untracked-files=no'],
                          check=True, capture_output=True, text=True).stdout.strip():
            raise ValueError('source has tracked modifications: ' + str(path))
        return subprocess.run(['git', '-C', str(path), 'rev-parse', 'HEAD'],
                              check=True, capture_output=True, text=True).stdout.strip()
    sniffle_revision = revision(args.sniffle_source)
    if sniffle_revision != '18252f2c5a05f9ad906865b776652e31c61471c4':
        raise ValueError('expected pinned Sniffle v1.11.0 source')
    alfa_revision = revision(args.alfa_source)
    metadata = (args.site_packages / 'pyserial-3.5.dist-info/METADATA').read_text()
    if '\nVersion: 3.5\n' not in metadata:
        raise ValueError('expected pyserial 3.5')
    # Hardware Sniffle uses pure Python + pyserial. Do not silently omit native dependencies.
    packages = [args.sniffle_source / 'python_cli/sniffle', args.site_packages / 'serial']
    for package in packages:
        if any(f.suffix in ('.so', '.dylib', '.pyd') for f in package.rglob('*')):
            raise ValueError('native extensions require a separate runtime packaging review')
    out.mkdir(parents=True)
    tree(packages[0], out / 'sniffle/modules/sniffle')
    tree(packages[1], out / 'sniffle/modules/serial')
    tree(args.site_packages / 'pyserial-3.5.dist-info', out / 'sniffle/modules/pyserial-3.5.dist-info')
    copy(ROOT / 'scripts/sniffle-source.py', out / 'sniffle/sniffle-source.py')
    capability_helper_hash = sha(out / 'sniffle/sniffle-source.py')
    capabilities = helper_capabilities(python, out / 'sniffle/sniffle-source.py')
    if sha(out / 'sniffle/sniffle-source.py') != capability_helper_hash:
        raise ValueError('staged helper changed during capability query')
    copy(args.sniffle_source / 'LICENSE', out / 'sniffle/licenses/Sniffle-GPL-3.0.txt')
    copy(args.pyserial_license, out / 'sniffle/licenses/pyserial-BSD-3-Clause.txt')
    (out / 'sniffle/python').write_text('#!/bin/sh\nexec ' + shlex.quote(str(python)) + ' -I -B "$@"\n')
    tree(args.alfa_source, out / 'alfa/source')
    copy(args.alfa_executable, out / 'alfa/mt7610u-lab')
    copy(args.firmware, out / 'alfa/mt7610e.bin')
    copy(args.libusb_license, out / 'alfa/licenses/libusb-LGPL-2.1.txt')
    manifest = {
        'schema_version': 1, 'platform': {'Darwin': 'macos', 'Linux': 'linux'}[platform.system()],
        'architecture': {'arm64': 'aarch64', 'aarch64': 'aarch64', 'x86_64': 'x86_64'}[platform.machine()],
        'sniffle': {'helper_capabilities': capabilities, 'python': 'sniffle/python', 'helper': 'sniffle/sniffle-source.py',
            'module_path': 'sniffle/modules', 'runtime_path': str(python),
            'runtime_sha256': sha(python), 'runtime_version': runtime['version'],
            'runtime_base_prefix': runtime['base_prefix'], 'source_revision': sniffle_revision,
            'version': '1.11.0', 'pyserial_version': '3.5',
            'licenses': ['sniffle/licenses/Sniffle-GPL-3.0.txt', 'sniffle/licenses/pyserial-BSD-3-Clause.txt']},
        'alfa': {'executable': 'alfa/mt7610u-lab', 'firmware': 'alfa/mt7610e.bin',
            'source_revision': alfa_revision, 'source_match': 'local_snapshot_not_reproducibly_verified',
            'licenses': ['alfa/source/LICENSE', 'alfa/source/FIRMWARE_LICENSE',
                         'alfa/source/THIRD_PARTY_NOTICES', 'alfa/licenses/libusb-LGPL-2.1.txt']}}
    executable = {'sniffle/python', 'alfa/mt7610u-lab'}
    if sha(out / 'sniffle/sniffle-source.py') != capability_helper_hash:
        raise ValueError('staged helper changed after capability query')
    manifest['files'] = [{'path': str(f.relative_to(out)), 'sha256': sha(f), 'bytes': f.stat().st_size,
                          'executable': str(f.relative_to(out)) in executable}
                         for f in sorted(out.rglob('*')) if f.is_file()]
    (out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(json.dumps({'bundle': str(out), 'files': len(manifest['files']),
                      'bytes': sum(f['bytes'] for f in manifest['files'])}))


if __name__ == '__main__':
    main()
