"""Actual child/pipe lifecycle tests; synthetic receiver only, no USB access."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('receiver_soak', Path(__file__).parents[1] / 'receiver-soak.py')
soak = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(soak)

CHILD = r'''
import json, os, select, signal, sys, time
mode = sys.argv[1]
state = 'running'
running = True
counters = {key: 0 for key in ('dropped_observations', 'transfers', 'frames', 'beacons', 'malformed_transfers', 'reconnect_attempts')}
def emit(value):
    print(json.dumps(value), flush=True)
def stop(*_):
    global running
    running = False
signal.signal(signal.SIGTERM, signal.SIG_IGN if mode == 'ignore_term' else stop)
emit({'type': 'hello', 'protocol_version': 1})
emit(dict(counters, type='status', state=state))
if mode == 'early':
    sys.exit(0)
if mode == 'noisy':
    os.write(2, b'x' * 20000)
while running:
    if select.select([sys.stdin], [], [], .01)[0]:
        line = sys.stdin.readline()
        if not line:
            break
        cmd = json.loads(line)
        if mode == 'missing_ack':
            continue
        if cmd['cmd'] == 'pause':
            state = 'paused'
        elif cmd['cmd'] == 'resume':
            state = 'running'
        emit(dict(counters, type='status', state=state, request_id=cmd['request_id'], cleanup_verified=True))
if mode == 'mcu_timeout':
    counters['mcu_timeouts'] = 1
if mode == 'final_drop':
    counters['dropped_observations'] = 1
emit(dict(counters, type='stopped', cleanup_verified=mode != 'bad_cleanup'))
'''


class SoakTests(unittest.TestCase):
    def args(self, output):
        return SimpleNamespace(output=output, seconds=.35, sample_seconds=.04,
                               status_seconds=.08, startup_timeout=2, ack_timeout=.08,
                               shutdown_timeout=1, recovery_timeout=.2, pause_every=0,
                               pause_seconds=.01, max_rss_growth_mib=32, max_fd_growth=0,
                               max_drops=0, log_bytes=1024, log_backups=2)

    def invoke(self, mode='normal', **changes):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            args = self.args(root / 'run')
            for key, value in changes.items():
                setattr(args, key, value)
            with patch.object(soak, 'sample_resources', return_value={'rss_bytes': 4096, 'cpu_seconds': 0, 'fds': 3}):
                report = soak.run(args, [sys.executable, '-u', '-c', CHILD, mode])
            stored = json.loads((args.output / 'summary.json').read_text())
            self.assertEqual(report, stored)
            if os.name == 'posix':
                self.assertEqual(args.output.stat().st_mode & 0o777, 0o700)
                for path in args.output.iterdir():
                    self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            raw_size = sum(p.stat().st_size for p in args.output.glob('*.log*'))
            self.assertLessEqual(raw_size, 2 * args.log_bytes * (args.log_backups + 1))
            return report

    def test_quiet_radio_completes_full_interval_and_sigterm_cleanup(self):
        report = self.invoke()
        self.assertTrue(report['passed'], report['failures'])
        self.assertGreaterEqual(report['measured_seconds'], report['requested_seconds'])
        self.assertGreater(report['commands_acknowledged'], 0)
        self.assertEqual(report['beacon_records'], 0)
        self.assertTrue(report['cleanup_verified'])

    def test_early_success_exit_cannot_pass_or_measure_shutdown(self):
        report = self.invoke('early')
        self.assertFalse(report['passed'])
        self.assertIn('receiver_exited_early', report['failures'])
        self.assertIsNone(report['shutdown_seconds'])

    def test_ignored_command_and_false_cleanup_fail(self):
        self.assertIn('command_acknowledgment_timeout', self.invoke('missing_ack')['failures'])
        self.assertIn('shutdown_cleanup_unverified', self.invoke('bad_cleanup')['failures'])

    def test_pauses_resume_before_completion(self):
        report = self.invoke(seconds=.7, pause_every=.15)
        self.assertTrue(report['passed'], report['failures'])
        self.assertGreaterEqual(report['commands_acknowledged'], 5)

    def test_final_counter_snapshot_and_forced_kill_are_failures(self):
        report = self.invoke('final_drop')
        self.assertEqual(report['counters']['dropped_observations'], 1)
        self.assertIn('observation_drop_limit_exceeded', report['failures'])
        self.assertIn('forced_kill', self.invoke('ignore_term', shutdown_timeout=.1)['failures'])

    def test_recovered_mcu_timeout_still_fails_acceptance(self):
        report = self.invoke('mcu_timeout')
        self.assertIn('mcu_command_timeouts_observed', report['failures'])
        self.assertFalse(report['passed'])

    def test_large_stderr_rotates_without_blocking(self):
        self.assertTrue(self.invoke('noisy')['passed'])

    def test_protocol_rejects_periodic_status_as_command_ack(self):
        protocol = soak.Protocol()
        protocol.hello = True
        protocol.pending = {'id': 'request-1', 'state': 'paused'}
        record = dict.fromkeys(soak.COUNTERS, 0)
        protocol.receive(dict(record, type='status', state='paused', cleanup_verified=True), 1)
        self.assertIsNotNone(protocol.pending)
        protocol.receive(dict(record, type='status', state='running', request_id='request-1'), 2)
        self.assertIn('wrong_acknowledgment_state', protocol.errors)

    def test_monotonic_counters_and_sequence_validation(self):
        protocol = soak.Protocol()
        protocol.hello = True
        record = dict.fromkeys(soak.COUNTERS, 2)
        protocol.receive(dict(record, type='status', state='running'), 1)
        record['frames'] = 1
        protocol.receive(dict(record, type='status', state='running'), 2)
        self.assertIn('invalid_or_decreasing_counter', protocol.errors)
        protocol.receive({'type': 'beacon', 'session_id': 'synthetic', 'sequence': 1}, 3)
        self.assertIn('nonconsecutive_beacon_sequence', protocol.errors)

    def test_metrics_and_cpu_time(self):
        self.assertEqual(soak.cpu_seconds('1-02:03:04.50'), 93784.5)
        self.assertEqual(soak.cpu_seconds('00:01.25'), 1.25)
        measurements = soak.Measurements(1)
        measurements.add(0, {'rss_bytes': 100, 'cpu_seconds': 2, 'fds': 3})
        measurements.add(10, {'rss_bytes': 150, 'cpu_seconds': 3, 'fds': 4})
        result = measurements.summary()
        self.assertEqual(result['cpu_mean_percent_one_core'], 10)
        self.assertEqual(result['rss_window_growth_bytes'], 50)
        self.assertEqual(result['fds_growth'], 1)


if __name__ == '__main__':
    unittest.main()
