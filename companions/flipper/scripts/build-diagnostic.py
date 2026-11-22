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
import struct
import zlib
from elftools.elf.elffile import ELFFile

ROOT = Path(__file__).resolve().parents[1]
FLAGS = ['-C', 'target-cpu=cortex-m4', '-C', 'panic=abort', '-C', 'debuginfo=2',
         '-C', 'embed-bitcode=yes', '-C', 'lto=yes',
         '-C', 'link-arg=--script=diagnostic.ld', '-C', 'link-arg=--Bstatic',
         '-C', 'link-arg=--relocatable', '-C', 'link-arg=--lto-O3',
         '-C', 'link-arg=--lto-whole-program-visibility']


def output(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify_pair(debug, fap):
    with debug.open('rb') as source, fap.open('rb') as stripped:
        original = ELFFile(source)
        deployed = ELFFile(stripped)
        for name in ('.debug_info', '.debug_line', '.debug_abbrev'):
            section = original.get_section_by_name(name)
            if section is None or not section.data_size:
                raise ValueError(f'missing nonempty {name}: debug information was stripped')
        def allocated(elf):
            return {s.name: (s['sh_type'], s['sh_flags'], s['sh_size'],
                            None if s['sh_type'] == 'SHT_NOBITS' else s.data())
                    for s in elf.iter_sections() if s['sh_flags'] & 2}
        if allocated(original) != allocated(deployed):
            raise ValueError('debug ELF and FAP allocated code/data differ')
        link = deployed.get_section_by_name('.gnu_debuglink')
        if link is None:
            raise ValueError('missing debug link')
        data = link.data()
        name_end = data.find(b'\0')
        crc_offset = (name_end + 4) & ~3
        if name_end < 0 or len(data) != crc_offset + 4:
            raise ValueError('malformed debug link')
        if data[:name_end] != debug.name.encode():
            raise ValueError('debug link filename mismatch')
        crc = struct.unpack_from('<I', data, crc_offset)[0]
        if crc != zlib.crc32(debug.read_bytes()):
            raise ValueError('debug link CRC mismatch')
        return {'dwarf_verified': True, 'allocated_sections_match': True,
                'debuglink_crc32': f'{crc:08x}'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('sdk_symbols', type=Path)
    parser.add_argument('--binary', choices=['airhound-uart-diagnostic', 'airhound-live-bridge'],
                        default='airhound-uart-diagnostic')
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
    revision = output('git', 'rev-parse', 'HEAD')
    modified = bool(output('git', 'status', '--porcelain'))
    if args.binary == 'airhound-live-bridge':
        env['AIRHOUND_SOURCE_REVISION'] = revision
        env['AIRHOUND_SOURCE_MODIFIED'] = '1' if modified else '0'
    # Isolated target path makes the exact output deterministic despite caller defaults.
    artifact_dir = 'diagnostic' if args.binary == 'airhound-uart-diagnostic' else 'live-bridge'
    env['CARGO_TARGET_DIR'] = str(ROOT / 'target' / (artifact_dir + '-build'))
    subprocess.run(['cargo', 'build', '--locked', '--release', '--features', 'fap',
                    '--bin', args.binary, '--target', 'thumbv7em-none-eabihf'],
                   cwd=ROOT, env=env, check=True)
    folder = ROOT / 'target' / artifact_dir
    folder.mkdir(parents=True, exist_ok=True)
    debug = folder / (args.binary + '.debug.elf')
    fap = folder / (args.binary + '.fap')
    shutil.copyfile(Path(env['CARGO_TARGET_DIR']) / 'thumbv7em-none-eabihf/release' / args.binary, debug)
    subprocess.run([str(objcopy), '--strip-debug', f'--add-gnu-debuglink={debug}', str(debug), str(fap)], check=True)
    result = audit.audit(fap, sdk)
    result.update(verify_pair(debug, fap))
    result.update(debug_elf_sha256=digest(debug), source_revision=revision,
                  source_modified=modified,
                  rustc=output('rustc', '-vV'), rustflags=FLAGS,
                  source_hashes={str(p.relative_to(ROOT)): digest(p) for p in sorted(
                      [*ROOT.glob('src/**/*.rs'), ROOT / 'Cargo.toml', ROOT / 'Cargo.lock',
                       ROOT / 'scripts/build-diagnostic.py', ROOT / 'scripts/check-fap.py',
                       ROOT / 'diagnostic.ld', ROOT / 'diagnostic-linker.LICENSE'])},
                  hardware_tested=False)
    (folder / 'audit.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
