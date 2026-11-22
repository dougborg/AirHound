#!/usr/bin/env python3
"""Regression-check a built diagnostic pair and deliberately damaged copies."""
import importlib.util
from pathlib import Path
import sys
import tempfile
from elftools.elf.elffile import ELFFile

spec = importlib.util.spec_from_file_location('builder', Path(__file__).with_name('build-diagnostic.py'))
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)
folder = Path(sys.argv[1])
debug = folder / 'airhound-uart-diagnostic.debug.elf'
fap = folder / 'airhound-uart-diagnostic.fap'
assert builder.verify_pair(debug, fap)['dwarf_verified']

def rejected(first, second):
    try:
        builder.verify_pair(first, second)
    except ValueError:
        return
    raise AssertionError('damaged artifact pair accepted')

rejected(fap, fap)  # A symbol table alone is not DWARF.
with tempfile.TemporaryDirectory() as temp:
    damaged = Path(temp) / 'altered.fap'
    with fap.open('rb') as source:
        elf = ELFFile(source)
        text_offset = elf.get_section_by_name('.text')['sh_offset']
        crc_offset = elf.get_section_by_name('.gnu_debuglink')['sh_offset'] + elf.get_section_by_name('.gnu_debuglink')['sh_size'] - 1
    for offset in (text_offset, crc_offset):
        data = bytearray(fap.read_bytes())
        data[offset] ^= 1
        damaged.write_bytes(data)
        rejected(debug, damaged)
print('4 artifact checks passed: matching DWARF, stripped-debug rejection, code mismatch, CRC mismatch')
