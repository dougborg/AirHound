"""Offline helper capability inspection is bounded and does not open devices."""
import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).parents[1] / 'stage-local-receiver-helpers.py'
SPEC = importlib.util.spec_from_file_location('stage_helpers', SCRIPT)
stage = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(stage)


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

    def test_stalled_query_is_killed_and_reaped(self):
        with tempfile.TemporaryDirectory() as root:
            script = Path(root) / 'helper.py'
            pidfile = Path(root) / 'pid'
            script.write_text(f'import os,time\nopen({str(pidfile)!r},"w").write(str(os.getpid()))\ntime.sleep(30)\n')
            with self.assertRaisesRegex(ValueError, 'timed out'):
                stage.helper_capabilities(Path(sys.executable), script)
            with self.assertRaises(ProcessLookupError):
                os.kill(int(pidfile.read_text()), 0)
