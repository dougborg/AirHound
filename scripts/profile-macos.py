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


def is_launched_process(pid, executable, observations):
    """Recheck identity before signaling a PID that Launch Services owns."""
    result = subprocess.run(
        ['ps', '-p', str(pid), '-o', 'args='], capture_output=True, text=True,
    )
    args = result.stdout.strip()
    output_argument = f'--output {observations}'
    return (result.returncode == 0
            and args.startswith(str(executable) + ' ')
            and (args.endswith(output_argument) or output_argument + ' ' in args))


class ObservationLog:
    """Read complete NDJSON records, including replies flushed during shutdown."""
    def __init__(self, path):
        self.path = path
        self.position = 0
        self.partial = b''
        self.messages = []

    def read(self, elapsed):
        if not self.path.exists():
            return
        with self.path.open('rb') as stream:
            stream.seek(self.position)
            self.partial += stream.read()
            self.position = stream.tell()
        lines = self.partial.split(b'\n')
        self.partial = lines.pop()
        for line in lines:
            if line:
                self.messages.append((elapsed, json.loads(line)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seconds', type=int, default=45)
    parser.add_argument('--allow-empty', action='store_true', help='Diagnostic-only run: permit no observations; reception remains unverified')
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
    samples, commands, errors = [], [], []
    log = ObservationLog(output)
    schedule = [(0, 'status'), (args.pause_at, 'stop'), (args.resume_at, 'start')]
    expected = [(deadline, cmd) for deadline, cmd in schedule if deadline < args.seconds]
    forced, complete = False, False
    shutdown_ms = None
    started = None
    observed_duration = 0
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
        started = time.monotonic()
        next_sample = 0
        pending = list(expected)
        phase = 'running'
        while True:
            elapsed = time.monotonic() - started
            observed_duration = elapsed
            if not running(pid):
                errors.append('Scanner exited before the requested profiling interval completed')
                break
            if elapsed >= args.seconds:
                complete = True
                break
            for deadline, value in list(pending):
                if elapsed >= deadline:
                    pending.remove((deadline, value))
                    os.write(descriptor, (json.dumps({'cmd': value}) + '\n').encode())
                    commands.append({'cmd': value, 'sent_s': time.monotonic() - started})
                    if value == 'stop': phase = 'paused'
                    if value == 'start': phase = 'resumed'
            if elapsed >= next_sample:
                stat = subprocess.run(['ps', '-p', str(pid), '-o', '%cpu=,rss='], capture_output=True, text=True).stdout.split()
                if len(stat) == 2:
                    samples.append({'elapsed_s': elapsed, 'cpu_percent': float(stat[0]), 'rss_kib': int(stat[1]), 'phase': phase})
                next_sample = elapsed + 1
            log.read(time.monotonic() - started)
            time.sleep(.005)
    finally:
        if pid is not None and running(pid):
            if is_launched_process(pid, executable, observations):
                stopped = time.monotonic()
                try:
                    os.kill(pid, signal.SIGINT)
                except ProcessLookupError:
                    errors.append('Scanner exited before SIGINT; shutdown was not measured')
                else:
                    while running(pid) and time.monotonic() - stopped < 10:
                        time.sleep(.01)
                    if running(pid):
                        errors.append('Scanner did not exit within ten seconds of SIGINT')
                        if is_launched_process(pid, executable, observations):
                            try:
                                os.kill(pid, signal.SIGKILL)
                                forced = True
                            except ProcessLookupError:
                                pass
                        else:
                            errors.append('Process identity changed; refused to force termination')
                    shutdown_ms = (time.monotonic() - stopped) * 1000
            else:
                errors.append('Process identity changed; refused to signal it')
        elif pid is not None and complete:
            errors.append('Scanner exited before SIGINT; shutdown was not measured')
        os.close(descriptor)
        fifo.unlink(missing_ok=True)
    log.read(time.monotonic() - started)
    if log.partial.strip():
        errors.append('Scanner output ended with an incomplete NDJSON record')
    rows = []
    if observations.exists():
        with observations.open() as stream:
            next(stream, None)
            rows = [row for row in csv.DictReader(stream) if row.get('Type') in ('WIFI', 'BLE')]
    else:
        errors.append('Scanner did not create its CSV output')
    records = [record for _, record in log.messages]
    ble = [record for record in records if record.get('type') == 'ble_observation']
    for _, expected_command in expected:
        command = next((cmd for cmd in commands if cmd['cmd'] == expected_command), None)
        if command is None:
            errors.append(f'Scheduled command {expected_command!r} was not sent')
            continue
        replies = [(arrival, record) for arrival, record in log.messages
                   if arrival >= command['sent_s']
                   and record.get('type') == 'command_result'
                   and record.get('cmd') == command['cmd']]
        command['acknowledged'] = False
        if replies:
            arrival, reply = replies[0]
            command['reply_ms'] = (arrival - command['sent_s']) * 1000
            command['acknowledged'] = reply.get('ok') is True
            if not command['acknowledged']:
                errors.append(f'Command {expected_command!r} did not succeed')
        else:
            errors.append(f'Command {expected_command!r} was not acknowledged')
    reception = {'wifi': any(row.get('Type') == 'WIFI' for row in rows), 'ble': bool(ble)}
    required = ('wifi', 'ble') if args.mode == 'all' else (args.mode,)
    if not args.allow_empty:
        for scanner in required:
            if not reception[scanner]:
                errors.append(f'No {scanner.upper()} observations; check scanner health and permissions')
    shutdown_timeout = 'Scanner exceeded shutdown budget' in diagnostics.read_text(errors='replace') if diagnostics.exists() else False
    if shutdown_timeout:
        errors.append('Scanner needed task cancellation at shutdown')
    health = [record for record in records if record.get('type') == 'scanner_health']
    metrics = {
        'reception_verified': all(reception[scanner] for scanner in required),
        'reception': reception, 'allow_empty': args.allow_empty,
        'scanner_health_latest': health[-1] if health else None,
        'scanner_shutdown_timeout': shutdown_timeout,
        'schema_version': 1, 'mode': args.mode, 'directory': str(directory),
        'success': not errors, 'errors': errors,
        'measurement_complete': complete, 'requested_duration_s': args.seconds,
        'observed_duration_s': observed_duration,
        'git_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'git_dirty': bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True).strip()),
        'binary_sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
        'samples': samples, 'commands': commands, 'reply_poll_interval_ms': 5,
        'cpu_percent_median': statistics.median(sample['cpu_percent'] for sample in samples) if samples else None,
        'rss_peak_mib': max(sample['rss_kib'] for sample in samples) / 1024 if samples else None,
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
    return int(bool(errors))


if __name__ == '__main__':
    sys.exit(main())
