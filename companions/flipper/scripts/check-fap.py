#!/usr/bin/env python3
"""Audit a feasibility FAP against the exact installed Momentum API export list.

Requires pyelftools==0.32. This verifies static metadata/import availability, not
loadability, relocation support, ABI behavior, stack headroom or hardware safety.
"""
import argparse
import csv
import hashlib
import json
import struct
from pathlib import Path
from elftools.elf.elffile import ELFFile

SDK_COMMIT = 'e1784e7418d8b074e971983ceb6fef0f37e52ae4'
SDK_SHA256 = '0353a374fe778fc0a28d69f4e6b8f650d20ad796460a57f9c0e97e359f15d3a9'


def audit(binary, symbols):
    if hashlib.sha256(symbols.read_bytes()).hexdigest() != SDK_SHA256:
        raise ValueError('API CSV does not match pinned Momentum mntm-012')
    with symbols.open(newline='') as source:
        rows = list(csv.DictReader(source))
    if not any(r['entry'] == 'Version' and r['name'] == '87.1' for r in rows):
        raise ValueError('expected API 87.1')
    exports = {r['name'] for r in rows if r['entry'] in ('Function', 'Variable') and r['status'] == '+'}
    with binary.open('rb') as source:
        elf = ELFFile(source)
        if elf.elfclass != 32 or not elf.little_endian or elf['e_machine'] != 'EM_ARM' or elf['e_type'] != 'ET_REL':
            raise ValueError('expected 32-bit little-endian ARM relocatable ELF')
        meta = elf.get_section_by_name('.fapmeta')
        if meta is None or meta.data_size < 20:
            raise ValueError('missing FAP manifest')
        magic, version, api, target, stack, app_version = struct.unpack_from('<IIIHHI', meta.data())
        if (magic, version, api, target) != (0x52474448, 1, (87 << 16) | 1, 7):
            raise ValueError('FAP manifest is not hardware 7 / API 87.1')
        table = elf.get_section_by_name('.symtab')
        if table is None:
            raise ValueError('missing ELF symbol table')
        imports = sorted({s.name for s in table.iter_symbols() if s.name and s['st_shndx'] == 'SHN_UNDEF'})
        missing = sorted(set(imports) - exports)
        if missing:
            raise ValueError('imports absent from installed SDK: ' + ', '.join(missing))
        if not any(s.name == '_start' and s['st_shndx'] != 'SHN_UNDEF' for s in table.iter_symbols()):
            raise ValueError('missing application entry point')
        allocated = sum(s['sh_size'] for s in elf.iter_sections() if s['sh_flags'] & 2)
    return {'api': '87.1', 'hardware_target': target, 'stack_bytes': stack, 'app_version': app_version,
            'file_bytes': binary.stat().st_size, 'allocated_section_bytes': allocated,
            'sdk_commit': SDK_COMMIT, 'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
            'imports': imports, 'hardware_tested': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('symbols', type=Path)
    args = parser.parse_args()
    print(json.dumps(audit(args.binary, args.symbols), indent=2))


if __name__ == '__main__':
    main()
