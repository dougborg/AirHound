#!/usr/bin/env python3
"""Package an already-built desktop executable, notices and build provenance."""
import argparse
import json
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def run(args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True)


def build(output):
    system = platform.system()
    architecture = platform.machine()
    supported = {('Darwin', 'arm64'): 'macos-arm64',
                 ('Linux', 'x86_64'): 'linux-x86_64'}
    target = supported.get((system, architecture))
    if target is None:
        raise ValueError('unsupported release build platform')
    binary = ROOT / 'hostd/target/release/airhound-hostd'
    run([str(binary), '--help'])  # Does not initialize radios.
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
    output.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(prefix='airhound-desktop-') as tmp:
        payload = Path(tmp) / f'airhound-desktop-{target}'
        payload.mkdir()
        if system == 'Darwin':
            run(['python3', 'scripts/bundle-macos.py'])
            app = payload / 'AirHound.app'
            shutil.copytree(ROOT / 'hostd/AirHound.app', app)
            plist = app / 'Contents/Info.plist'
            info = plistlib.loads(plist.read_bytes())
            info['CFBundleVersion'] = version
            info['CFBundleShortVersionString'] = version
            plist.write_bytes(plistlib.dumps(info))
            run(['codesign', '--force', '--deep', '--sign', '-', str(app)])
            run(['codesign', '--verify', '--deep', '--strict', str(app)])
            links = run(['otool', '-L', str(binary)])
            for line in links.splitlines()[1:]:
                lib = line.strip().split(' (', 1)[0]
                if not lib.startswith(('/usr/lib/', '/System/Library/')):
                    raise ValueError(f'non-system macOS dependency: {lib}')
            launch = './AirHound.app/Contents/MacOS/airhound-hostd'
        else:
            (payload / 'bin').mkdir()
            shutil.copy2(binary, payload / 'bin/airhound-hostd')
            links = run(['ldd', str(binary)])
            if 'not found' in links:
                raise ValueError('unresolved Linux runtime dependency')
            launch = './bin/airhound-hostd'
        (payload / 'runtime-libraries.txt').write_text(links)
        metadata = json.loads(run(['cargo', 'metadata', '--locked', '--format-version', '1'], ROOT / 'hostd'))
        dependencies = []
        for package in metadata['packages']:
            dependencies.append({key: package.get(key) for key in ('name', 'version', 'license', 'repository')})
            directory = Path(package['manifest_path']).parent
            notices = [p for p in directory.rglob('*') if p.is_file()
                       and p.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE'))] if package['source'] else [ROOT / 'LICENSE']
            for notice in notices:
                relative = notice.relative_to(directory) if package['source'] else Path('LICENSE')
                destination = payload / 'dependency-notices' / f"{package['name']}-{package['version']}" / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(notice, destination)
        (payload / 'dependencies.json').write_text(json.dumps(dependencies, indent=2) + '\n')
        shutil.copy2(ROOT / 'LICENSE', payload / 'LICENSE')
        (payload / 'README.txt').write_text(
            f'AirHound {version} engineering preview ({target})\n\n'
            f'Run from a terminal: {launch} --help\n'
            'This is a CLI, not a graphical desktop interface.\n'
            'macOS: Apple Silicon, macOS 15 or newer; grant Bluetooth/Location permissions.\n'
            'The app is ad-hoc signed, not Developer ID signed or notarized.\n'
            'Use the OS approval flow for downloaded software; do not disable Gatekeeper globally.\n'
            'Default macOS build includes private Bluetooth address lookup.\n'
            'Linux: x86_64, built on Ubuntu 22.04; requires glibc 2.35 or newer,\n'
            'libdbus-1.so.3, libpcap.so.0.8, and BlueZ for native Bluetooth.\n'
            'Install runtime packages libdbus-1-3, libpcap0.8 and bluez on Ubuntu.\n'
            'Monitor-mode WiFi and USB receivers need appropriate device permissions.\n'
            'GPS uses separately installed gpsd. CatSniffer uses separately installed Sniffle\n'
            'and the GPL helper in the source/tools archive. No external firmware is bundled.\n'
            'Full setup, experimental limits and schemas are in the accompanying source archive.\n'
            'https://github.com/dougborg/AirHound/tree/main/hostd\n')
        (payload / 'manifest.json').write_text(json.dumps({
            'schema_version': 1, 'release_version': version,
            'component_version': tomllib.loads((ROOT / 'hostd/Cargo.toml').read_text())['package']['version'],
            'git_commit': run(['git', 'rev-parse', 'HEAD']).strip(),
            'platform': target, 'build_os': platform.platform(),
            'rustc': run(['rustc', '--version']).strip(),
            'status': 'experimental_preview', 'developer_id_signed': False,
            'notarized': False, 'macos_private_addresses': system == 'Darwin',
        }, indent=2) + '\n')
        destination = output / f'{payload.name}.tar.gz'
        with tarfile.open(destination, 'w:gz') as archive:
            archive.add(payload, arcname=payload.name)
        # Exercise the shipped executable after extraction, without opening radios.
        extract = Path(tmp) / 'verify'
        with tarfile.open(destination) as archive:
            archive.extractall(extract, filter='data')
        run([str(extract / payload.name / launch.removeprefix('./')), '--help'])
        print(destination)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, required=True)
    build(parser.parse_args().output_dir.resolve())
