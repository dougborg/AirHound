#!/usr/bin/env python3
"""Reject ESP32 firmware with less than the reviewed startup-stack reservation.

This inspects linker symbols, not runtime stack high-water marks. The 7 KiB
floor rejects a 6,556-byte scheduling trial that hit the CPU0 stack guard during
BLE startup. The validated streaming image reserves 7,212 bytes. Passing this
static gate is necessary, but cannot replace boot and runtime testing.
"""
import argparse
import json
from pathlib import Path
import subprocess


def check_symbols(text, minimum):
    if minimum <= 0:
        raise ValueError('minimum stack reservation must be positive')
    symbols = {}
    required = {'_stack_start_cpu0', '_stack_end_cpu0'}
    for line in text.splitlines():
        fields = line.split()
        if len(fields) == 3 and fields[-1] in required:
            name = fields[-1]
            if name in symbols:
                raise ValueError('duplicate stack boundary symbol')
            symbols[name] = int(fields[0], 16)
    if symbols.keys() != required:
        raise ValueError('missing ESP32 CPU0 stack boundary symbols')
    size = symbols['_stack_start_cpu0'] - symbols['_stack_end_cpu0']
    if size < minimum:
        raise ValueError(f'startup stack reservation {size} bytes is below {minimum} bytes')
    return {'startup_stack_reserved_bytes': size, 'minimum_bytes': minimum,
            'scope': 'linked reservation; runtime high-water is not measured'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('elf', type=Path)
    parser.add_argument('--nm', default='xtensa-esp32-elf-nm')
    parser.add_argument('--minimum-bytes', type=int, default=7 * 1024)
    args = parser.parse_args()
    try:
        result = subprocess.run([args.nm, '--defined-only', str(args.elf)],
                                check=True, capture_output=True, text=True)
        report = check_symbols(result.stdout, args.minimum_bytes)
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        parser.exit(1, f'Firmware memory check failed: {error}\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
