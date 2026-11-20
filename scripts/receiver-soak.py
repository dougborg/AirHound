#!/usr/bin/env python3
"""Measure a receiver stream child for the entire requested wall-clock interval.

No hardware is accessed by this module itself. The explicit command after -- is
launched once and may claim hardware. Raw logs stay in a new private directory;
summary.json contains only aggregate measurements, never captured identifiers.
"""
import argparse
from collections import deque
import datetime
import json
import math
import os
from pathlib import Path
import selectors
import shutil
import signal
import statistics
import subprocess
import sys
import time

COUNTERS = ('dropped_observations', 'transfers', 'frames', 'beacons',
            'malformed_transfers', 'reconnect_attempts')
MAX_LINE = 16 * 1024


class RotatingLog:
    """Bound raw logs by bytes, including a possibly huge incoming chunk."""
    def __init__(self, path, limit, backups):
        self.path, self.limit, self.backups = path, limit, backups
        self.size = 0
        self.file = self.open()

    def open(self):
        return os.fdopen(os.open(self.path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), 'wb')

    def write(self, data):
        while data:
            if self.size == self.limit:
                self.file.close()
                for index in range(self.backups, 0, -1):
                    src = self.path if index == 1 else Path(f'{self.path}.{index - 1}')
                    dst = Path(f'{self.path}.{index}')
                    if src.exists():
                        if index == self.backups and dst.exists():
                            dst.unlink()
                        src.rename(dst)
                if not self.backups:
                    self.path.unlink()
                self.file = self.open()
                self.size = 0
            part, data = data[:self.limit - self.size], data[self.limit - self.size:]
            self.file.write(part)
            self.size += len(part)

    def close(self):
        self.file.close()


def cpu_seconds(value):
    """Parse ps cumulative CPU time: [[days-]hours:]minutes:seconds."""
    days, clock = value.split('-', 1) if '-' in value else ('0', value)
    parts = [float(part) for part in clock.split(':')]
    if len(parts) not in (2, 3):
        raise ValueError('unsupported ps time')
    return int(days) * 86400 + sum(part * 60 ** power for power, part in enumerate(reversed(parts)))


def sample_resources(pid):
    """Sample only our direct child; no privileged system-wide process scan."""
    result = subprocess.run(['ps', '-p', str(pid), '-o', 'rss=,time='],
                            capture_output=True, text=True, timeout=2, check=False)
    values = result.stdout.split()
    if result.returncode or len(values) != 2:
        raise ValueError('process metrics unavailable')
    rss, cpu = int(values[0]) * 1024, cpu_seconds(values[1])
    proc = Path(f'/proc/{pid}/fd')
    if proc.is_dir():
        fds = len(list(proc.iterdir()))
    else:
        lsof = shutil.which('lsof') or ('/usr/sbin/lsof' if Path('/usr/sbin/lsof').exists() else None)
        if not lsof:
            raise ValueError('FD measurement needs procfs or lsof')
        result = subprocess.run([lsof, '-a', '-p', str(pid), '-F', 'f'],
                                capture_output=True, text=True, timeout=2, check=False)
        if result.returncode:
            raise ValueError('FD measurement unavailable')
        fds = sum(line[1:].isdigit() for line in result.stdout.splitlines() if line.startswith('f'))
    return {'rss_bytes': rss, 'cpu_seconds': cpu, 'fds': fds}


class Measurements:
    def __init__(self, window_samples):
        self.first = []
        self.last = deque(maxlen=window_samples)
        self.limit = window_samples
        self.count = 0
        self.peak_rss = self.peak_fds = 0

    def add(self, elapsed, sample):
        sample = dict(sample, elapsed_seconds=elapsed)
        if len(self.first) < self.limit:
            self.first.append(sample)
        self.last.append(sample)
        self.count += 1
        self.peak_rss = max(self.peak_rss, sample['rss_bytes'])
        self.peak_fds = max(self.peak_fds, sample['fds'])

    def summary(self):
        if not self.count:
            return {'samples': 0}
        first, last = self.first[0], self.last[-1]
        interval = last['elapsed_seconds'] - first['elapsed_seconds']
        baseline = statistics.mean(s['rss_bytes'] for s in self.first)
        final = statistics.mean(s['rss_bytes'] for s in self.last)
        return {'samples': self.count, 'rss_peak_bytes': self.peak_rss,
                'rss_initial_window_mean_bytes': baseline, 'rss_final_window_mean_bytes': final,
                'rss_window_growth_bytes': final - baseline, 'fds_peak': self.peak_fds,
                'fds_initial': first['fds'], 'fds_final': last['fds'],
                'fds_growth': last['fds'] - first['fds'],
                'cpu_observed_seconds': last['cpu_seconds'] - first['cpu_seconds'],
                'cpu_mean_percent_one_core': 100 * (last['cpu_seconds'] - first['cpu_seconds']) / interval if interval > 0 else None,
                'measurement_span_seconds': interval}


class Protocol:
    def __init__(self):
        self.hello = False
        self.ready_at = None
        self.state = None
        self.pending = None
        self.acks = 0
        self.beacon_records = 0
        self.counters = {key: 0 for key in COUNTERS}
        self.stopped = False
        self.cleanup_verified = False
        self.session = None
        self.next_sequence = 0
        self.errors = []

    def fail(self, code):
        if code not in self.errors:
            self.errors.append(code)

    def receive(self, record, now):
        if not isinstance(record, dict):
            self.fail('non_object_record')
            return
        kind = record.get('type')
        if kind == 'hello':
            if self.hello or type(record.get('protocol_version')) is not int or record['protocol_version'] != 1:
                self.fail('invalid_hello')
            self.hello = True
            return
        if not self.hello:
            self.fail('record_before_hello')
        if self.stopped:
            self.fail('record_after_stopped')
        if kind == 'status':
            state = record.get('state')
            if state not in ('running', 'paused', 'recovering'):
                self.fail('invalid_state')
                return
            self.state = state
            if state == 'running' and self.ready_at is None:
                self.ready_at = now
            if state == 'paused' and record.get('cleanup_verified') is not True:
                self.fail('pause_cleanup_unverified')
            for key in COUNTERS:
                value = record.get(key)
                if type(value) is not int or value < self.counters[key]:
                    self.fail('invalid_or_decreasing_counter')
                else:
                    self.counters[key] = value
            request = record.get('request_id')
            if request is not None:
                if not self.pending or request != self.pending['id']:
                    self.fail('unexpected_acknowledgment')
                elif state != self.pending['state']:
                    self.fail('wrong_acknowledgment_state')
                else:
                    self.acks += 1
                    self.pending = None
        elif kind == 'beacon':
            if self.state != 'running':
                self.fail('observation_while_not_running')
            session, sequence = record.get('session_id'), record.get('sequence')
            if not isinstance(session, str) or not session:
                self.fail('invalid_beacon_session')
            if session != self.session:
                self.session, self.next_sequence = session, 0
            if type(sequence) is not int or sequence != self.next_sequence:
                self.fail('nonconsecutive_beacon_sequence')
            self.next_sequence += 1
            self.beacon_records += 1
        elif kind == 'stopped':
            for key in COUNTERS:
                value = record.get(key)
                if type(value) is not int or value < self.counters[key]:
                    self.fail('invalid_or_decreasing_counter')
                else:
                    self.counters[key] = value
            self.stopped = True
            self.cleanup_verified = record.get('cleanup_verified') is True
            if not self.cleanup_verified:
                self.fail('shutdown_cleanup_unverified')
        elif kind != 'status':
            self.fail('unexpected_record_type')

    def command(self, process, cmd, now, timeout):
        request = f'soak-{self.acks + 1}'
        expected = {'pause': 'paused', 'resume': 'running', 'status': self.state}[cmd]
        payload = json.dumps({'cmd': cmd, 'request_id': request}).encode() + b'\n'
        process.stdin.write(payload)
        process.stdin.flush()
        self.pending = {'id': request, 'cmd': cmd, 'state': expected, 'deadline': now + timeout}


def run(args, command):
    args.output.mkdir(mode=0o700, parents=False, exist_ok=False)
    os.chmod(args.output, 0o700)
    logs = {name: RotatingLog(args.output / f'{name}.log', args.log_bytes, args.log_backups)
            for name in ('stdout', 'stderr')}
    protocol = Protocol()
    measurements = Measurements(max(1, math.ceil(min(300, args.seconds / 4) / args.sample_seconds)))
    process = None
    selector = selectors.DefaultSelector()
    buffer = bytearray()
    launch = time.monotonic()
    shutdown_at = None
    end = None
    interrupted = False
    metric_errors = 0
    next_sample = next_status = launch
    next_pause = None
    resume_at = None
    recovering_at = None
    previous_handlers = {}

    def interrupt(_signum, _frame):
        nonlocal interrupted
        interrupted = True

    for sig in (signal.SIGINT, signal.SIGTERM):
        previous_handlers[sig] = signal.signal(sig, interrupt)
    try:
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, start_new_session=True, bufsize=0)
        for name in logs:
            stream = getattr(process, name)
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ, name)
        while True:
            now = time.monotonic()
            alive = process.poll() is None
            if interrupted:
                protocol.fail('harness_interrupted')
            if not alive and shutdown_at is None:
                protocol.fail('receiver_exited_early')
            if protocol.ready_at is not None:
                if end is None:
                    end = protocol.ready_at + args.seconds
                    next_pause = protocol.ready_at + args.pause_every if args.pause_every else None
                if protocol.state == 'recovering':
                    recovering_at = recovering_at or now
                    if now - recovering_at > args.recovery_timeout:
                        protocol.fail('recovery_timeout')
                else:
                    recovering_at = None
            elif now - launch > args.startup_timeout:
                protocol.fail('startup_timeout')
            if protocol.pending and now > protocol.pending['deadline']:
                protocol.fail('command_acknowledgment_timeout')
            if shutdown_at is None and alive and (protocol.errors or (end is not None and now >= end)):
                shutdown_at = now
                process.send_signal(signal.SIGTERM)
            if shutdown_at is not None and alive and now - shutdown_at > args.shutdown_timeout:
                protocol.fail('forced_kill')
                process.kill()
                process.wait(timeout=5)
                alive = False
            if shutdown_at is None and alive and protocol.ready_at is not None and not protocol.errors:
                if now >= next_sample:
                    try:
                        measurements.add(now - protocol.ready_at, sample_resources(process.pid))
                    except (OSError, ValueError, subprocess.TimeoutExpired):
                        metric_errors += 1
                    next_sample = time.monotonic() + args.sample_seconds
                if not protocol.pending:
                    cmd = None
                    if resume_at is not None and now >= resume_at:
                        cmd, resume_at = 'resume', None
                    elif next_pause is not None and now >= next_pause and now + args.pause_seconds + 2 * args.ack_timeout < end:
                        cmd = 'pause'
                        next_pause = now + args.pause_every
                    elif now >= next_status:
                        cmd = 'status'
                        next_status = now + args.status_seconds
                    if cmd:
                        try:
                            protocol.command(process, cmd, now, args.ack_timeout)
                        except (BrokenPipeError, OSError):
                            protocol.fail('command_write_failed')
            events = selector.select(timeout=0.05)
            for key, _ in events:
                data = os.read(key.fileobj.fileno(), 65536)
                if not data:
                    selector.unregister(key.fileobj)
                    continue
                logs[key.data].write(data)
                if key.data != 'stdout':
                    continue
                buffer.extend(data)
                while b'\n' in buffer:
                    line, _, tail = buffer.partition(b'\n')
                    buffer = bytearray(tail)
                    if len(line) + 1 > MAX_LINE:
                        protocol.fail('oversized_record')
                        continue
                    try:
                        record = json.loads(line)
                    except (ValueError, UnicodeDecodeError):
                        protocol.fail('malformed_record')
                        continue
                    pending = protocol.pending
                    protocol.receive(record, time.monotonic())
                    if pending and pending['cmd'] == 'pause' and protocol.pending is None:
                        resume_at = time.monotonic() + args.pause_seconds
                if len(buffer) >= MAX_LINE:
                    protocol.fail('oversized_record')
                    buffer.clear()
            if not alive and not selector.get_map():
                break
            # A child which exits but leaves pipe-owning descendants cannot hang us.
            if not alive and time.monotonic() - (shutdown_at or launch) > args.shutdown_timeout:
                protocol.fail('pipes_not_closed')
                break
    except (OSError, ValueError, subprocess.TimeoutExpired):
        protocol.fail('harness_io_error')
    finally:
        if process is not None and process.poll() is None:
            if shutdown_at is None:
                shutdown_at = time.monotonic()
            process.terminate()
            try:
                process.wait(timeout=args.shutdown_timeout)
            except subprocess.TimeoutExpired:
                protocol.fail('forced_kill')
                process.kill()
                process.wait(timeout=5)
        finished = time.monotonic()
        selector.close()
        for log in logs.values():
            log.close()
        if process:
            for stream in (process.stdin, process.stdout, process.stderr):
                stream.close()
        for sig, handler in previous_handlers.items():
            signal.signal(sig, handler)
    if buffer:
        protocol.fail('truncated_record')
    if not protocol.hello or protocol.ready_at is None:
        protocol.fail('readiness_not_verified')
    if not protocol.stopped or not protocol.cleanup_verified:
        protocol.fail('shutdown_not_verified')
    if protocol.pending:
        protocol.fail('missing_command_acknowledgment')
    if process is None or process.returncode != 0:
        protocol.fail('receiver_exit_not_success')
    measured = max(0, (shutdown_at or finished) - protocol.ready_at) if protocol.ready_at is not None else 0
    if measured < args.seconds:
        protocol.fail('duration_not_completed')
    resources = measurements.summary()
    if measurements.count < 2:
        protocol.fail('insufficient_resource_samples')
    if metric_errors:
        protocol.fail('resource_measurement_errors')
    if resources.get('rss_window_growth_bytes', 0) > args.max_rss_growth_mib * 1024 * 1024:
        protocol.fail('rss_growth_limit_exceeded')
    if resources.get('fds_growth', 0) > args.max_fd_growth:
        protocol.fail('fd_growth_limit_exceeded')
    if protocol.counters['dropped_observations'] > args.max_drops:
        protocol.fail('observation_drop_limit_exceeded')
    report = {
        'schema_version': 1, 'passed': not protocol.errors, 'failures': protocol.errors,
        'created_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'requested_seconds': args.seconds, 'measured_seconds': measured,
        'startup_seconds': protocol.ready_at - launch if protocol.ready_at is not None else None,
        'shutdown_seconds': finished - shutdown_at if shutdown_at is not None else None,
        'exit_code': process.returncode if process else None, 'commands_acknowledged': protocol.acks,
        'cleanup_verified': protocol.cleanup_verified, 'beacon_records': protocol.beacon_records,
        'counters': protocol.counters, 'resource_measurement_errors': metric_errors,
        'resources': resources, 'limits': {'rss_growth_mib': args.max_rss_growth_mib,
                                         'fd_growth': args.max_fd_growth, 'drops': args.max_drops},
        'method': {'cpu': 'ps cumulative child CPU delta divided by observed monotonic interval',
                   'rss': 'ps resident bytes; initial/final windows of at most 300 seconds',
                   'fds': 'procfs or lsof numeric descriptors; direct child only',
                   'sample_seconds': args.sample_seconds, 'pause_every_seconds': args.pause_every,
                   'raw_log_max_bytes': 2 * args.log_bytes * (args.log_backups + 1)}}
    with os.fdopen(os.open(args.output / 'summary.json', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), 'w') as output:
        json.dump(report, output, indent=2, allow_nan=False)
        output.write('\n')
    return report


def positive(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError('must be finite and positive')
    return number


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path, help='New private directory; parent must exist')
    parser.add_argument('--seconds', type=positive, default=28800)
    parser.add_argument('--sample-seconds', type=positive, default=5)
    parser.add_argument('--status-seconds', type=positive, default=30)
    parser.add_argument('--startup-timeout', type=positive, default=30)
    parser.add_argument('--ack-timeout', type=positive, default=10)
    parser.add_argument('--shutdown-timeout', type=positive, default=10)
    parser.add_argument('--recovery-timeout', type=positive, default=30)
    parser.add_argument('--pause-every', type=float, default=300, help='0 disables pause/resume exercises')
    parser.add_argument('--pause-seconds', type=positive, default=2)
    parser.add_argument('--max-rss-growth-mib', type=float, default=32)
    parser.add_argument('--max-fd-growth', type=int, default=0)
    parser.add_argument('--max-drops', type=int, default=0)
    parser.add_argument('--log-bytes', type=int, default=1024 * 1024)
    parser.add_argument('--log-backups', type=int, default=3)
    parser.add_argument('command', nargs=argparse.REMAINDER, help='-- driver stream ...')
    args = parser.parse_args(argv)
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('explicit receiver command required after --')
    if args.seconds > 28800:
        parser.error('--seconds maximum is 28800 (8 hours)')
    if args.sample_seconds >= args.seconds:
        parser.error('--sample-seconds must be smaller than --seconds')
    for value in (args.pause_every, args.max_rss_growth_mib, args.max_fd_growth, args.max_drops, args.log_backups):
        if not math.isfinite(value) or value < 0:
            parser.error('budgets and pause interval must be finite and nonnegative')
    if not 1024 <= args.log_bytes <= 64 * 1024 * 1024 or args.log_backups > 16:
        parser.error('log limits: 1024..67108864 bytes, 0..16 backups')
    try:
        report = run(args, command)
    except OSError:
        print('Could not create private output directory/artifacts; use a new writable directory.', file=sys.stderr)
        return 2
    print(json.dumps(report, allow_nan=False))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
