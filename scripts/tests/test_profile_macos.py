"""Exercise profiler outcomes without requiring macOS, radios, or real processes."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import signal
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / 'profile-macos.py'
spec = importlib.util.spec_from_file_location('profile_macos', SCRIPT)
profiler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profiler)


class FakeScanner:
    def __init__(self, root, *, exit_at=None, missing_reply=None, rejected_reply=None,
                 reply_on_shutdown=False, ignore_sigint=False, replaced_pid=False,
                 exit_on_sigint=False, launch_delay=0):
        self.root = root
        self.executable = root / 'hostd/AirHound.app/Contents/MacOS/airhound-hostd'
        self.executable.parent.mkdir(parents=True)
        self.executable.write_bytes(b'fake executable')
        self.now = 0
        self.exit_at = exit_at
        self.alive = True
        self.missing_reply = missing_reply
        self.rejected_reply = rejected_reply
        self.reply_on_shutdown = reply_on_shutdown
        self.ignore_sigint = ignore_sigint
        self.replaced_pid = replaced_pid
        self.exit_on_sigint = exit_on_sigint
        self.launch_delay = launch_delay
        self.signals = []
        self.deferred_replies = []

    def sleep(self, duration):
        self.now += duration

    def running(self):
        return self.alive and (self.exit_at is None or self.now < self.exit_at)

    def run(self, args, **kwargs):
        if args[0] == 'open':
            self.output = Path(args[args.index('--stdout') + 1])
            self.csv = Path(args[args.index('--output') + 1])
            self.output.write_bytes(b'')
            self.csv.write_text('WiGLE header\nMAC,Type,CurrentLatitude\n')
            self.now += self.launch_delay
        elif args == ['ps', '-p', '12345', '-o', 'args=']:
            text = 'unrelated process' if self.replaced_pid else f'{self.executable} --output {self.csv}'
            return SimpleNamespace(stdout=text, returncode=0)
        elif args == ['ps', '-p', '12345', '-o', '%cpu=,rss=']:
            return SimpleNamespace(stdout='0.1 1024', returncode=0)
        else:
            raise AssertionError(f'Unexpected subprocess: {args!r}')
        return SimpleNamespace(returncode=0)

    def check_output(self, args, **kwargs):
        if args == ['ps', '-axo', 'pid=,args=']:
            return f'12345 {self.executable} --output {self.csv}\n'
        if args == ['git', 'rev-parse', 'HEAD']:
            return 'test-commit\n'
        if args == ['git', 'status', '--porcelain']:
            return ''
        raise AssertionError(f'Unexpected subprocess: {args!r}')

    def write(self, _descriptor, data):
        command = json.loads(data)['cmd']
        if command != self.missing_reply:
            reply = {'type': 'command_result', 'cmd': command,
                     'ok': command != self.rejected_reply}
            if self.reply_on_shutdown:
                self.deferred_replies.append(reply)
            else:
                self.append(reply)
        return len(data)

    def append(self, reply):
        with self.output.open('a') as stream:
            stream.write(json.dumps(reply) + '\n')

    def kill(self, pid, sig):
        if pid != 12345:
            raise AssertionError(f'Unexpected PID: {pid}')
        if not self.running():
            raise ProcessLookupError
        if not sig:
            return
        if sig == signal.SIGINT and self.exit_on_sigint:
            self.alive = False
            raise ProcessLookupError
        self.signals.append(sig)
        if sig == signal.SIGINT:
            for reply in self.deferred_replies:
                self.append(reply)
            if not self.ignore_sigint:
                self.alive = False
        elif sig == signal.SIGKILL:
            self.alive = False
        else:
            raise AssertionError(f'Unexpected signal: {sig}')


class ProfileTests(unittest.TestCase):
    def profile(self, arguments=None, **behavior):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake = FakeScanner(root, **behavior)
            with contextlib.ExitStack() as stack:
                for target, attribute, value in [
                    (profiler, 'ROOT', root),
                    (profiler.sys, 'argv', ['profile-macos.py'] + (arguments or ['--seconds', '1'])),
                    (profiler.sys, 'platform', 'darwin'),
                    (profiler.time, 'monotonic', lambda: fake.now),
                    (profiler.time, 'sleep', fake.sleep),
                    (profiler.subprocess, 'run', fake.run),
                    (profiler.subprocess, 'check_output', fake.check_output),
                    (profiler.os, 'write', fake.write),
                    (profiler.os, 'kill', fake.kill),
                ]:
                    stack.enter_context(patch.object(target, attribute, value))
                stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
                result = profiler.main()
            paths = list(root.glob('hostd/target/performance/*/metrics.json'))
            self.assertEqual(len(paths), 1)
            metrics = json.loads(paths[0].read_text())
            self.assertFalse(paths[0].with_name('commands.fifo').exists())
            return result, metrics, fake.signals

    def test_early_exit_is_failure_with_unmeasured_shutdown(self):
        for when in (0, .5):
            with self.subTest(exit_at=when):
                result, metrics, signals = self.profile(exit_at=when)
                self.assertEqual(result, 1)
                self.assertFalse(metrics['success'])
                self.assertFalse(metrics['measurement_complete'])
                self.assertLess(metrics['observed_duration_s'], 1)
                self.assertIsNone(metrics['shutdown_ms'])
                self.assertEqual(signals, [])

    def test_short_quiet_run_requires_only_status_and_full_interval(self):
        result, metrics, signals = self.profile(launch_delay=2)
        self.assertEqual(result, 0)
        self.assertTrue(metrics['measurement_complete'])
        self.assertGreaterEqual(metrics['observed_duration_s'], 1)
        self.assertEqual(metrics['ble_observations'], 0)
        self.assertEqual([cmd['cmd'] for cmd in metrics['commands']], ['status'])
        self.assertTrue(metrics['commands'][0]['acknowledged'])
        self.assertIsNotNone(metrics['shutdown_ms'])
        self.assertEqual(signals, [signal.SIGINT])

    def test_missing_or_rejected_scheduled_reply_fails(self):
        arguments = ['--seconds', '3', '--pause-at', '1', '--resume-at', '2']
        for behavior in ({'missing_reply': 'stop'}, {'rejected_reply': 'start'}):
            with self.subTest(behavior=behavior):
                result, metrics, _ = self.profile(arguments, **behavior)
                self.assertEqual(result, 1)
                self.assertTrue(metrics['measurement_complete'])
                self.assertEqual(sum(cmd['acknowledged'] for cmd in metrics['commands']), 2)
                self.assertTrue(metrics['errors'])

    def test_final_replies_are_read_after_shutdown(self):
        result, metrics, _ = self.profile(reply_on_shutdown=True)
        self.assertEqual(result, 0)
        self.assertTrue(metrics['commands'][0]['acknowledged'])

    def test_status_and_stop_can_both_be_scheduled_at_zero(self):
        result, metrics, _ = self.profile(['--seconds', '2', '--pause-at', '0', '--resume-at', '1'])
        self.assertEqual(result, 0)
        self.assertEqual([cmd['cmd'] for cmd in metrics['commands']], ['status', 'stop', 'start'])

    def test_forced_shutdown_fails(self):
        result, metrics, signals = self.profile(ignore_sigint=True)
        self.assertEqual(result, 1)
        self.assertTrue(metrics['forced_kill'])
        self.assertGreaterEqual(metrics['shutdown_ms'], 10000)
        self.assertEqual(signals, [signal.SIGINT, signal.SIGKILL])

    def test_reused_pid_is_not_signaled(self):
        result, metrics, signals = self.profile(replaced_pid=True)
        self.assertEqual(result, 1)
        self.assertIsNone(metrics['shutdown_ms'])
        self.assertEqual(signals, [])

    def test_process_exit_racing_sigint_does_not_claim_shutdown_measurement(self):
        result, metrics, _ = self.profile(exit_on_sigint=True)
        self.assertEqual(result, 1)
        self.assertIsNone(metrics['shutdown_ms'])

    def test_partial_utf8_record_waits_for_newline(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'events.ndjson'
            record = {'type': 'ble_observation', 'name': 'caf\u00e9'}
            encoded = (json.dumps(record, ensure_ascii=False) + '\n').encode()
            cut = encoded.index(b'\xc3') + 1
            path.write_bytes(encoded[:cut])
            log = profiler.ObservationLog(path)
            log.read(0)
            self.assertEqual(log.messages, [])
            with path.open('ab') as stream:
                stream.write(encoded[cut:])
            log.read(1)
            self.assertEqual(log.messages, [(1, record)])
            self.assertEqual(log.partial, b'')


if __name__ == '__main__':
    unittest.main()
