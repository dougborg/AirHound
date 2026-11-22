#!/usr/bin/env python3
"""Build/audit an experimental UART diagnostic; never opens or installs hardware.

Requires Cargo/rustc, llvm-tools-preview and requirements-dev.txt. Retains DWARF
in a matching debug ELF, strips only debug sections from the deployable FAP, and
records source/artifact hashes. API availability is not hardware acceptance.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
FLAGS = ['-C', 'target-cpu=cortex-m4', '-C', 'panic=abort', '-C', 'debuginfo=2',
         '-C', 'embed-bitcode=yes', '-C', 'lto=yes',
         '-C', 'link-arg=--script=flipperzero-rt.ld', '-C', 'link-arg=--Bstatic',
         '-C', 'link-arg=--relocatable', '-C', 'link-arg=--lto-O3',
         '-C', 'link-arg=--lto-whole-program-visibility']


def output(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('sdk_symbols', type=Path)
    args = parser.parse_args()
    sdk = args.sdk_symbols.resolve()
    spec = importlib.util.spec_from_file_location('fap_audit', ROOT / 'scripts/check-fap.py')
    audit = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(audit)
    if digest(sdk) != audit.SDK_SHA256:
        raise ValueError('SDK CSV is not pinned API87.1')
    host = next(line[6:] for line in output('rustc', '-vV').splitlines() if line.startswith('host: '))
    objcopy = Path(output('rustc', '--print', 'sysroot')) / 'lib/rustlib' / host / 'bin/llvm-objcopy'
    if not objcopy.is_file():
        raise ValueError('install llvm-tools-preview for this Rust toolchain first')
    env = dict(os.environ)
    env.pop('RUSTFLAGS', None)
    env['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join(FLAGS)
    # Isolated target path makes the exact output deterministic despite caller defaults.
    env['CARGO_TARGET_DIR'] = str(ROOT / 'target/diagnostic-build')
    subprocess.run(['cargo', 'build', '--locked', '--release', '--features', 'fap',
                    '--bin', 'airhound-uart-diagnostic', '--target', 'thumbv7em-none-eabihf'],
                   cwd=ROOT, env=env, check=True)
    folder = ROOT / 'target/diagnostic'
    folder.mkdir(parents=True, exist_ok=True)
    debug = folder / 'airhound-uart-diagnostic.debug.elf'
    fap = folder / 'airhound-uart-diagnostic.fap'
    shutil.copyfile(Path(env['CARGO_TARGET_DIR']) / 'thumbv7em-none-eabihf/release/airhound-uart-diagnostic', debug)
    subprocess.run([str(objcopy), '--strip-debug', f'--add-gnu-debuglink={debug}', str(debug), str(fap)], check=True)
    result = audit.audit(fap, sdk)
    result.update(debug_elf_sha256=digest(debug), source_revision=output('git', 'rev-parse', 'HEAD'),
                  source_modified=bool(output('git', 'status', '--porcelain')),
                  rustc=output('rustc', '-vV'), rustflags=FLAGS,
                  source_hashes={str(p.relative_to(ROOT)): digest(p) for p in sorted(
                      [*ROOT.glob('src/**/*.rs'), ROOT / 'Cargo.toml', ROOT / 'Cargo.lock',
                       ROOT / 'scripts/build-diagnostic.py', ROOT / 'scripts/check-fap.py'])},
                  hardware_tested=False)
    (folder / 'audit.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
