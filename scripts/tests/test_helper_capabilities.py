"""Offline helper capability inspection is bounded and does not open devices."""
import importlib.util
import os
from pathlib import Path
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
    def test_current_contract_and_legacy_oversized_reports(self):
        report = stage.helper_capabilities(Path(sys.executable), SCRIPT.parent / 'sniffle-source.py')
        self.assertIn('borrowed_serial_lease_v1', report)
        with tempfile.TemporaryDirectory() as root:
            script = Path(root) / 'helper.py'
            for content in ('print("{}")', 'print("x" * 10000)'):
                script.write_text(content)
                with self.assertRaises(ValueError):
                    stage.helper_capabilities(Path(sys.executable), script)

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
