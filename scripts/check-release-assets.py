#!/usr/bin/env python3
"""Require the complete preview asset set before publication and hash every asset."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import tarfile

EXPECTED = {f'airhound-{board}{flavor}.bin'
            for board in ('m5stickc', 'xiao') for flavor in ('', '-std')}
EXPECTED |= {f'airhound-desktop-{target}.tar.gz'
             for target in ('macos-arm64', 'linux-x86_64')}
EXPECTED |= {f'mt7610u-lab-{target}{suffix}.tar.gz'
             for target in ('macos-arm64', 'linux-x86_64') for suffix in ('', '-source')}
EXPECTED.add('airhound-source.tar.gz')


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check(directory):
    actual = {p.name for p in directory.iterdir() if p.suffix == '.bin' or p.name.endswith('.tar.gz')}
    if actual != EXPECTED:
        raise ValueError(f'asset set mismatch: missing={sorted(EXPECTED - actual)}, unexpected={sorted(actual - EXPECTED)}')
    for name in sorted(EXPECTED):
        path = directory / name
        if path.is_symlink() or path.stat().st_size == 0:
            raise ValueError(f'empty or symlink asset: {name}')
        if not name.endswith('.tar.gz'):
            continue
        with tarfile.open(path) as archive:
            members = archive.getmembers()
            for member in members:
                parts = PurePosixPath(member.name)
                if parts.is_absolute() or '..' in parts.parts or not (member.isfile() or member.isdir()):
                    raise ValueError(f'unsafe archive member: {member.name}')
            names = {m.name for m in members}
            prefix = name.removesuffix('.tar.gz')
            if name == 'airhound-source.tar.gz':
                required = ['LICENSE', 'signatures/builtin.v2.json', 'schemas/signatures.v2.schema.json',
                            'scripts/sniffle-source.py', 'scripts/licenses/GPL-3.0.txt',
                            'scripts/replay-hci-ble.py', 'docs/release-status.md']
            elif name.endswith('-source.tar.gz'):
                required = ['Cargo.lock', 'Cargo.toml', '.cargo/config.toml', 'BUILD-SOURCE.txt',
                            'LICENSE', 'THIRD_PARTY_NOTICES']
                if not any(n.startswith(prefix + '/vendor/') for n in names):
                    raise ValueError('missing corresponding dependency sources')
            elif name.startswith('mt7610u-lab-'):
                required = ['bin/mt7610u-lab', 'LICENSE', 'THIRD_PARTY_NOTICES', 'LIBUSB-LICENSE', 'manifest.json']
                manifest = json.load(archive.extractfile(prefix + '/manifest.json'))
                source = manifest['source_archive']
                if source not in EXPECTED or manifest['source_sha256'] != digest(directory / source):
                    raise ValueError('receiver corresponding-source mismatch')
                executable = archive.extractfile(prefix + '/bin/mt7610u-lab').read()
                if hashlib.sha256(executable).hexdigest() != manifest['binary_sha256']:
                    raise ValueError('receiver binary provenance mismatch')
            else:
                required = ['manifest.json', 'dependencies.json', 'LICENSE', 'README.txt', 'runtime-libraries.txt']
                required += ['AirHound.app/Contents/MacOS/airhound-hostd', 'AirHound.app/Contents/Info.plist'] if 'macos' in name else ['bin/airhound-hostd']
            for required_name in required:
                if prefix + '/' + required_name not in names:
                    raise ValueError(f'{name} missing {required_name}')
    (directory / 'SHA256SUMS').write_text(''.join(f'{digest(directory / name)}  {name}\n' for name in sorted(EXPECTED)))
    print(f'Validated {len(EXPECTED)} release assets; wrote SHA256SUMS')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    check(parser.parse_args().directory)
