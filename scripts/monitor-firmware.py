#!/usr/bin/env python3
"""Bounded serial observation of an already-flashed AirHound board; never flashes."""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import re
import time

ANSI = re.compile(r'\x1b\[[0-9;]*[A-Za-z]')
MAX_BYTES = 64 * 1024 * 1024
MAX_LINE = 16 * 1024


class Monitor:
    def __init__(self, board, version, require_psram=False):
        self.board, self.version = board, version
        self.require_psram = require_psram
        self.stream_stats = []
        self.observation_counts = Counter()
        self.memory_stats = []
        self.psram_ready = False
        self.statuses = []
        self.types = Counter()
        self.boots = 0
        self.failures = set()
        self.initialized = set()
        self.wifi_stats = []
        self.observed_addresses = []

    def line(self, raw):
        line = ANSI.sub('', raw.decode('utf-8', 'replace')).strip()
        start = line.find('{')
        if start >= 0:
            try:
                value = json.loads(line[start:])
            except ValueError:
                return
            if not isinstance(value, dict):
                return
            kind = value.get('type')
            if isinstance(kind, str):
                # Only protocol type names, never arbitrary identifiers, in summaries.
                if kind in ('status', 'wifi', 'ble', 'drone', 'error', 'command_result', 'stream_stats'):
                    self.types[kind] += 1
                else:
                    self.types['other'] += 1
            if kind in ('wifi', 'ble') and isinstance(value.get('match'), list):
                suffix = 'matched' if value['match'] else 'ordinary'
                self.observation_counts[f'{kind}_{suffix}'] += 1
            if kind == 'stream_stats':
                keys = ('scan_drops', 'output_drops', 'ble_output_drops', 'serialize_errors')
                if any(type(value.get(k)) is not int or not 0 <= value[k] <= 0xffffffff for k in keys):
                    self.failures.add('invalid stream loss metrics')
                else:
                    self.stream_stats.append({k: value[k] for k in keys})
            if kind != 'status':
                return
            if value.get('board') != self.board or value.get('version') != self.version:
                self.failures.add('unexpected board or firmware version')
                return
            if any(type(value.get(k)) is not int or not 0 <= value[k] <= 0xffffffff
                   for k in ('uptime', 'heap_free')):
                self.failures.add('invalid status metrics')
                return
            if value.get('scanning') is not True:
                self.failures.add('scanning unexpectedly disabled')
            if self.statuses and value['uptime'] < self.statuses[-1]['uptime']:
                self.failures.add('device uptime decreased')
            self.statuses.append({k: value[k] for k in ('uptime', 'heap_free', 'scanning')})
            return
        stats = re.fullmatch(r'INFO - WiFi stats: rx=(\d+) parsed=(\d+) queue_drops=(\d+) hop_errors=(\d+) matches=(\d+)', line)
        if stats:
            sample = dict(zip(('rx', 'parsed', 'queue_drops', 'hop_errors', 'matches'),
                              map(int, stats.groups())))
            self.wifi_stats.append(sample)
            if sample['hop_errors']:
                self.failures.add('WiFi channel hopping failed')
        observed = re.fullmatch(r'INFO - Observed addresses: aps=(\d+) wifi_other=(\d+) ble=(\d+) wifi_hits=(\d+) ble_hits=(\d+) limited=(true|false)', line)
        if observed:
            sample = dict(zip(('aps', 'wifi_other', 'ble', 'wifi_hits', 'ble_hits'),
                              map(int, observed.groups()[:5])))
            sample['limited'] = observed.group(6) == 'true'
            self.observed_addresses.append(sample)
        memory = re.fullmatch(r'INFO - Memory stats: internal_free=(\d+) external_free=(\d+)', line)
        if memory:
            self.memory_stats.append(dict(zip(('internal_free', 'external_free'), map(int, memory.groups()))))
        if line == 'INFO - PSRAM ready: bytes=2097152 tested=true observation_capacity=4096':
            self.psram_ready = True
        if 'AirHound v' in line and 'starting on' in line:
            self.boots += 1
        if re.search(r'panicked at|Guru Meditation|abort\(\)|memory allocation of .* failed|ERROR -', line):
            self.failures.add('firmware error or panic in serial log')
        for marker, name in [('WiFi sniffer initialized', 'wifi'), ('BLE scan started', 'ble_scan'),
                             ('BLE advertising as', 'ble_advertising'),
                             ('Display initialized', 'display'), ('Buzzer ready', 'buzzer')]:
            if marker in line:
                self.initialized.add(name)

    def summary(self, elapsed):
        failures = set(self.failures)
        if self.boots > 1:
            failures.add('multiple firmware startups during observation')
        if len(self.statuses) < 2:
            failures.add('fewer than two valid periodic status samples')
        if self.require_psram:
            if not self.psram_ready:
                failures.add('PSRAM startup self-test not observed')
            if len(self.memory_stats) < 2 or any(s['external_free'] <= 0 for s in self.memory_stats):
                failures.add('missing or exhausted external memory')
        heaps = [s['heap_free'] for s in self.statuses]
        return {'schema_version': 1, 'passed': not failures, 'failures': sorted(failures),
                'board': self.board, 'version': self.version, 'observed_seconds': elapsed,
                'boot_count': self.boots, 'initialized': sorted(self.initialized),
                'message_counts': dict(self.types),
                'observation_counts': dict(self.observation_counts),
                'stream_stats': self.stream_stats, 'status_samples': self.statuses,
                'wifi_stats': self.wifi_stats,
                'observed_addresses': self.observed_addresses,
                'memory_stats': self.memory_stats, 'psram_ready': self.psram_ready,
                'heap_free_min': min(heaps) if heaps else None,
                'heap_free_max': max(heaps) if heaps else None,
                'scope': 'serial observation only; display appearance, sound, RF sensitivity and long-run stability are not validated'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', required=True)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--seconds', type=float, default=120)
    parser.add_argument('--board', default='m5stickc_plus2')
    parser.add_argument('--version', required=True)
    parser.add_argument('--require-psram', action='store_true', help='Require boot PSRAM self-test and external-memory samples')
    args = parser.parse_args()
    if not 35 <= args.seconds <= 3600:
        parser.error('seconds must be between 35 and 3600')
    import serial  # Optional hardware tool: pip install pyserial.
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    monitor = Monitor(args.board, args.version, args.require_psram)
    started = time.monotonic()
    total, pending = 0, bytearray()
    port = serial.Serial(port=None, baudrate=115200, timeout=0.25)
    port.dtr = False
    port.rts = False
    port.port = args.port
    fd = os.open(args.output / 'serial.log', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, 'wb') as log:
            port.open()
            while time.monotonic() - started < args.seconds:
                data = port.read(4096)
                total += len(data)
                if total > MAX_BYTES:
                    raise ValueError('serial capture exceeds 64 MiB')
                log.write(data)
                log.flush()  # Make progress and boot failures inspectable during the run.
                pending.extend(data)
                while b'\n' in pending:
                    line, _, remaining = pending.partition(b'\n')
                    pending = bytearray(remaining)
                    if len(line) > MAX_LINE:
                        raise ValueError('serial line exceeds 16 KiB')
                    monitor.line(line)
                if len(pending) > MAX_LINE:
                    raise ValueError('unterminated serial line exceeds 16 KiB')
    except (OSError, ValueError, serial.SerialException) as error:
        monitor.failures.add(f'serial capture failed: {type(error).__name__}')
    finally:
        port.close()
    summary = monitor.summary(time.monotonic() - started)
    fd = os.open(args.output / 'summary.json', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as output:
        json.dump(summary, output, indent=2)
        output.write('\n')
    print(json.dumps(summary))
    return 0 if summary['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
