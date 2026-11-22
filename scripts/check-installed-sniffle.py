#!/usr/bin/env python3
"""Explicit offline runtime check for an installed helper bundle; no device opens.

Runs user-installed code intentionally. Inventory never invokes this command.
The Python framework/stdlib remains an external, versioned dependency.
"""
import argparse
import json
from pathlib import Path
import subprocess


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--bundle', type=Path, required=True)
    a = p.parse_args()
    root = a.bundle.resolve(strict=True)
    manifest = json.loads((root / 'manifest.json').read_text())
    s = manifest['sniffle']
    modules = root / s['module_path']
    program = '''import sys, pathlib, json
root = pathlib.Path(sys.argv[1]).resolve()
sys.path.insert(0, str(root))
import serial, sniffle.sniffle_hw, sniffle.packet_decoder, sniffle.pcap
selected = [serial, sniffle.sniffle_hw, sniffle.packet_decoder, sniffle.pcap]
paths = [str(pathlib.Path(m.__file__).resolve()) for m in selected]
assert all(pathlib.Path(p).is_relative_to(root) for p in paths), paths
assert serial.VERSION == "3.5"
print(json.dumps({"imports":paths, "python":sys.executable, "version":sys.version, "base_prefix":sys.base_prefix}))
'''
    result = subprocess.run([str(root / s['python']), '-c', program, str(modules)],
                            check=True, capture_output=True, text=True, timeout=10)
    subprocess.run([str(root / s['python']), str(root / s['helper']), '--help'],
                   check=True, capture_output=True, text=True, timeout=10)
    print(result.stdout.strip())


if __name__ == '__main__':
    main()
