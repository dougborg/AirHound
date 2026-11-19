#!/usr/bin/env python3
"""Profile the built macOS app via Launch Services; keep radio data local."""
import argparse
import collections
import csv
import datetime
import json
import hashlib
import os
from pathlib import Path
import signal
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]


def running(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seconds', type=int, default=45)
    parser.add_argument('--mode', choices=['all', 'ble', 'wifi'], default='all')
    parser.add_argument('--pause-at', type=int, default=15)
    parser.add_argument('--resume-at', type=int, default=25)
    args = parser.parse_args()
    if sys.platform != 'darwin' or args.seconds < 1:
        parser.error('Requires macOS and a positive duration')
    if args.pause_at < 0 or args.resume_at <= args.pause_at:
        parser.error('Require 0 <= pause-at < resume-at')
    app = ROOT / 'hostd/AirHound.app'
    executable = app / 'Contents/MacOS/airhound-hostd'
    if not executable.is_file() or executable.is_symlink():
        parser.error('Run just bundle-hostd first; a real bundled executable is required')
    stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S-%f')
    directory = ROOT / f'hostd/target/performance/live-{stamp}-{args.mode}'
    directory.mkdir(parents=True, mode=0o700)
    fifo = directory / 'commands.fifo'
    os.mkfifo(fifo, 0o600)
    descriptor = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)
    output = directory / 'events.ndjson'
    diagnostics = directory / 'stderr.log'
    observations = directory / 'observations.csv'
    command = ['open', '-n', str(app), '--stdin', str(fifo), '--stdout', str(output), '--stderr', str(diagnostics), '--env', 'RUST_LOG=airhound_hostd=debug,btleplug=warn', '--args', '--output', str(observations)]
    command += {'ble': ['--no-wifi'], 'wifi': ['--no-ble']}.get(args.mode, [])
    pid = None
    samples, commands, messages = [], [], []
    forced = False
    started = time.monotonic()
    try:
        subprocess.run(command, check=True)
        for _ in range(100):
            processes = subprocess.check_output(['ps', '-axo', 'pid=,args='], text=True)
            matches = [int(line.split(None, 1)[0]) for line in processes.splitlines() if str(executable) in line and str(observations) in line]
            if len(matches) == 1:
                pid = matches[0]
                break
            time.sleep(.05)
        if pid is None:
            raise RuntimeError(f'Could not identify launched process; inspect {diagnostics}')
        next_sample = 0
        pending = {0: 'status', args.pause_at: 'stop', args.resume_at: 'start'}
        phase = 'running'
        position, partial = 0, ''
        while time.monotonic() - started < args.seconds and running(pid):
            elapsed = time.monotonic() - started
            for deadline in sorted(list(pending)):
                if elapsed >= deadline:
                    value = pending.pop(deadline)
                    os.write(descriptor, (json.dumps({'cmd': value}) + '\n').encode())
                    commands.append({'cmd': value, 'sent_s': time.monotonic() - started})
                    if value == 'stop': phase = 'paused'
                    if value == 'start': phase = 'resumed'
            if elapsed >= next_sample:
                stat = subprocess.run(['ps', '-p', str(pid), '-o', '%cpu=,rss='], capture_output=True, text=True).stdout.split()
                if len(stat) == 2:
                    samples.append({'elapsed_s': elapsed, 'cpu_percent': float(stat[0]), 'rss_kib': int(stat[1]), 'phase': phase})
                next_sample = elapsed + 1
            if output.exists():
                with output.open() as stream:
                    stream.seek(position)
                    chunk = stream.read()
                    position = stream.tell()
                partial += chunk
                lines = partial.split('\n')
                partial = lines.pop()
                for line in lines:
                    if not line: continue
                    message = json.loads(line)
                    messages.append((time.monotonic() - started, message))
            time.sleep(.005)
    finally:
        stopped = time.monotonic()
        if pid is not None and running(pid):
            os.kill(pid, signal.SIGINT)
            while running(pid) and time.monotonic() - stopped < 10:
                time.sleep(.01)
            if running(pid):
                forced = True
                os.kill(pid, signal.SIGKILL)
        shutdown_ms = (time.monotonic() - stopped) * 1000
        os.close(descriptor)
        fifo.unlink(missing_ok=True)
    with observations.open() as stream:
        next(stream, None)
        rows = [row for row in csv.DictReader(stream) if row.get('Type') in ('WIFI', 'BLE')]
    records = [json.loads(line) for line in output.read_text().splitlines() if line]
    ble = [record for record in records if record.get('type') == 'ble_observation']
    for command in commands:
        replies = [arrival for arrival, record in messages if arrival >= command['sent_s'] and record.get('type') == 'command_result' and record.get('cmd') == command['cmd']]
        if replies: command['reply_ms'] = (replies[0] - command['sent_s']) * 1000
    metrics = {
        'schema_version': 1, 'mode': args.mode, 'directory': str(directory),
        'git_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'git_dirty': bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True).strip()),
        'binary_sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
        'samples': samples, 'commands': commands, 'reply_poll_interval_ms': 5,
        'cpu_percent_median': statistics.median(sample['cpu_percent'] for sample in samples) if samples else None,
        'rss_peak_mib': max((sample['rss_kib'] for sample in samples), default=0) / 1024,
        'shutdown_ms': shutdown_ms, 'forced_kill': forced,
        'csv_types': dict(collections.Counter(row['Type'] for row in rows)),
        'located_rows': sum(bool(row.get('CurrentLatitude')) for row in rows),
        'zero_mac_rows': sum(row['MAC'] == '00:00:00:00:00:00' for row in rows),
        'ble_observations': len(ble), 'ble_with_address': sum(bool(row.get('address')) for row in ble),
        'distinct_peripheral_ids': len({row['peripheral_id'] for row in ble}),
        'message_types': dict(collections.Counter(record['type'] for record in records)),
    }
    (directory / 'metrics.json').write_text(json.dumps(metrics, indent=2) + '\n')
    print(json.dumps({key: value for key, value in metrics.items() if key != 'samples'}, indent=2))
    return int(forced)


if __name__ == '__main__':
    sys.exit(main())
