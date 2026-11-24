"""Offline helper capability inspection is bounded and does not open devices."""
import importlib.util
import json
import os
import select
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

SCRIPT = Path(__file__).parents[1] / 'stage-local-receiver-helpers.py'
SPEC = importlib.util.spec_from_file_location('stage_helpers', SCRIPT)
stage = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(stage)

SUPPORT_SPEC = importlib.util.spec_from_file_location('process_support', SCRIPT.parent / 'test-process-support.py')
support = importlib.util.module_from_spec(SUPPORT_SPEC)
SUPPORT_SPEC.loader.exec_module(support)


class HelperCapabilitiesTests(unittest.TestCase):
    def boundary_query(self, helper, *, phase=None, boundary=0, wait_error=None,
                       cleanup_error=None):
        """Real pipe/exit events drive a clock that never follows scheduler time."""
        now = [0.0]
        children, query_waits, cleanup_waits = [], [], []
        real_read, real_loads = os.read, json.loads

        def advance(event):
            if event == phase:
                now[0] = boundary

        def readable(readers, writers, errors, remaining):
            events = select.select(readers, writers, errors, 30)
            self.assertTrue(events[0], 'Capability fixture pipe did not become ready')
            advance('readiness')
            return events

        def read(fd, limit):
            chunk = real_read(fd, limit)
            if not chunk:
                advance('eof')
            return chunk

        def loads(payload):
            result = real_loads(payload)
            advance('acceptance')
            if phase == 'validation':
                class ValidatedList(list):
                    def __iter__(self):
                        advance('validation')
                        return super().__iter__()
                result['capabilities'] = ValidatedList(result['capabilities'])
            return result

        def spawn(*args, **kwargs):
            child = subprocess.Popen(*args, **kwargs)
            children.append(child)
            real_wait, real_poll = child.wait, child.poll
            cleaning = [False]

            def poll():
                cleaning[0] = True
                return real_poll()

            def wait(timeout):
                (cleanup_waits if cleaning[0] else query_waits).append(timeout)
                result = real_wait(timeout=30)  # Fixture hang guard, not query time.
                if cleaning[0]:
                    if cleanup_error is not None:
                        raise cleanup_error
                else:
                    advance('exit')
                    if wait_error is not None:
                        raise wait_error
                return result

            child.poll, child.wait = poll, wait
            return child

        try:
            with patch.object(stage, 'time', SimpleNamespace(monotonic=lambda: now[0])), \
                 patch.object(stage, 'select', SimpleNamespace(select=readable)), \
                 patch.object(stage, 'os', SimpleNamespace(**{**vars(os), 'read': read})), \
                 patch.object(stage, 'json', SimpleNamespace(**{**vars(json), 'loads': loads})), \
                 patch.object(stage, 'subprocess', SimpleNamespace(**{**vars(subprocess), 'Popen': spawn})):
                return stage.helper_capabilities(Path(sys.executable), helper)
        finally:
            self.assertEqual(len(children), 1)
            child = children[0]
            self.assertIsNotNone(child.returncode)
            if cleanup_error is None:
                self.assertTrue(child.stdout.closed)
            else:
                # Existing cleanup-wait errors propagate before stdout.close.
                # The fixture owns the pipe and has already reaped its child.
                self.assertFalse(child.stdout.closed)
                child.stdout.close()
            self.assertEqual(cleanup_waits, [3], 'Cleanup keeps its separate bound')
            self.assertEqual(len(query_waits), 0 if phase in ('readiness', 'eof') and boundary >= 3 else 1)
            if query_waits:
                self.assertAlmostEqual(query_waits[0], 3 - boundary if phase in ('readiness', 'eof') else 3)
            with self.assertRaises(ChildProcessError):
                os.waitpid(child.pid, os.WNOHANG)

    def test_eof_exit_and_acceptance_obey_exclusive_virtual_deadline(self):
        with tempfile.TemporaryDirectory() as root:
            helper = Path(root) / 'helper.py'
            helper.write_text('print(' + repr(json.dumps({
                'type': 'sniffle_helper_capabilities', 'schema_version': 1,
                'capabilities': ['borrowed_serial_lease_v1', 'structured_errors_v1'],
            })) + ')')
            for phase in ('readiness', 'eof', 'exit', 'acceptance', 'validation'):
                for boundary in (2.999, 3.0, 3.001):
                    with self.subTest(phase=phase, boundary=boundary):
                        if boundary < 3:
                            self.assertIn('borrowed_serial_lease_v1', self.boundary_query(
                                helper, phase=phase, boundary=boundary))
                        else:
                            with self.assertRaisesRegex(ValueError, 'query timed out'):
                                self.boundary_query(helper, phase=phase, boundary=boundary)

    def test_exit_failures_keep_error_provenance_and_reap(self):
        with tempfile.TemporaryDirectory() as root:
            helper = Path(root) / 'helper.py'
            helper.write_text('raise SystemExit(7)')
            with self.assertRaisesRegex(ValueError, 'query failed'):
                self.boundary_query(helper)
            helper.write_text('pass')
            failure = subprocess.TimeoutExpired('owned fixture exit', 3)
            with self.assertRaises(subprocess.TimeoutExpired) as caught:
                self.boundary_query(helper, wait_error=failure)
            self.assertIs(caught.exception, failure)
            for failure in (subprocess.TimeoutExpired('owned fixture cleanup', 3),
                            OSError('injected cleanup failure')):
                with self.subTest(error=type(failure).__name__):
                    with self.assertRaises(type(failure)) as caught:
                        self.boundary_query(helper, cleanup_error=failure)
                    self.assertIs(caught.exception, failure)

    def query_payload(self, helper):
        def await_readable(readers, writers, errors, remaining):
            self.assertEqual(remaining, 3)
            events = select.select(readers, writers, errors, 30)
            self.assertTrue(events[0], 'Capability fixture did not complete its pipe output')
            return events

        def spawn(*args, **kwargs):
            process = subprocess.Popen(*args, **kwargs)
            original_wait = process.wait

            def wait(timeout):
                self.assertEqual(timeout, 3)
                return original_wait(timeout=30)  # Outer owned-child hang guard.

            process.wait = wait
            return process

        with patch.object(stage, 'time', SimpleNamespace(monotonic=lambda: 0)), \
             patch.object(stage, 'select', SimpleNamespace(select=await_readable)), \
             patch.object(stage, 'subprocess', SimpleNamespace(
                 **{**vars(subprocess), 'Popen': spawn})):
            return stage.helper_capabilities(Path(sys.executable), helper)

    def test_current_contract_and_legacy_oversized_reports(self):
        report = self.query_payload(SCRIPT.parent / 'sniffle-source.py')
        self.assertIn('borrowed_serial_lease_v1', report)
        with tempfile.TemporaryDirectory() as root:
            script = Path(root) / 'helper.py'
            for content in ('print("{}")', 'print("x" * 10000)'):
                script.write_text(content)
                with self.assertRaises(ValueError):
                    self.query_payload(script)

    def test_stalled_query_virtual_deadline_boundaries_kill_and_reap(self):
        for now in (2.999, 3.0, 3.001):
            with self.subTest(now=now), tempfile.TemporaryDirectory() as root:
                ready=support.ReadyPipe(root)
                self.addCleanup(ready.close)
                script=Path(root)/'helper.py'
                script.write_text("\n".join([
                    'import os,signal,json',
                    ready.write_code("{'pid':os.getpid(),'state':'ready'}"),
                    'signal.pause()',
                ]))
                calls=[]
                child=[]
                def clock():
                    if not calls:
                        calls.append(0)
                        return 0
                    if not child:
                        message=ready.receive()
                        self.assertEqual(message['state'],'ready')
                        child.append(message['pid'])
                    calls.append(now)
                    return now
                waits=[]
                def wait(readers, writers, errors, remaining):
                    # Explicit virtual expiration of the blocking read. No real
                    # wait or startup timing decides the timeout result.
                    waits.append(remaining)
                    return ([],[],[])
                with patch.object(stage,'time',SimpleNamespace(monotonic=clock)), patch.object(stage,'select',SimpleNamespace(select=wait)):
                    with self.assertRaisesRegex(ValueError,'timed out'):
                        stage.helper_capabilities(Path(sys.executable),script)
                self.assertEqual(len(waits),1 if now<3 else 0)
                if waits: self.assertAlmostEqual(waits[0],0.001)
                with self.assertRaises(ProcessLookupError): os.kill(child[0],0)
