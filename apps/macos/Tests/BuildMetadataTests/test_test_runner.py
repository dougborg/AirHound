import importlib.util
from contextlib import nullcontext
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / "run-tests.py"
spec = importlib.util.spec_from_file_location("native_test_runner", SCRIPT)
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class TestRunnerTests(unittest.TestCase):
    def test_owned_snapshot_excludes_unrelated_processes(self):
        rows = runner.owned_rows("10 1 10 /bin/swift\n11 10 11 /usr/bin/xctest\n12 1 12 /usr/bin/xctest\n", 10)
        self.assertEqual([row["pid"] for row in rows], [10, 11])

    def test_reused_pid_is_not_signalled(self):
        with patch.object(runner, "identity", return_value={"birth": [2, 1]}), patch.object(runner.os, "kill") as kill:
            self.assertFalse(runner.signal_owned({"pid": 42, "birth": [1, 1]}, 15))
            kill.assert_not_called()

    def test_failure_and_unbuffered_environment_are_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            for status in (0, 7):
                self.assertEqual(runner.run([sys.executable, "-c",
                    f"import os,sys; assert os.environ['NSUnbufferedIO']=='YES'; sys.exit({status})"],
                    2, Path(directory)), status)

    def test_timeout_kills_and_reaps_owned_uncooperative_child(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pidfile = root / "pid"
            code = ("import os,signal,time,pathlib; signal.signal(signal.SIGTERM,signal.SIG_IGN); "
                    f"pathlib.Path({str(pidfile)!r}).write_text(str(os.getpid())); time.sleep(30)")
            self.assertEqual(runner.run([sys.executable, "-c", code], .25, root), 124)
            pid = int(pidfile.read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
            self.assertTrue((root / "owned-processes.json").exists())

    def test_timeout_cleans_descendant_in_its_own_process_group(self):
        self.check_cross_group_cleanup()

    def test_diagnostic_io_and_later_snapshot_failure_do_not_skip_known_cleanup(self):
        self.check_cross_group_cleanup(diagnostics_failure=True)

    def check_cross_group_cleanup(self, diagnostics_failure=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pidfile = root / "pids"
            code = "\n".join([
                "import os,signal,subprocess,sys,time,json,pathlib",
                "child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)'],start_new_session=True)",
                "signal.signal(signal.SIGTERM,lambda *_: child.wait(timeout=1))",
                f"pathlib.Path({str(pidfile)!r}).write_text(json.dumps([os.getpid(),child.pid]))",
                "time.sleep(30)",
            ])
            destination = root
            original_snapshot = runner.snapshot
            calls = []
            def fail_second_snapshot(pid):
                calls.append(pid)
                if len(calls) > 1:
                    raise OSError("controlled later snapshot failure")
                return original_snapshot(pid)
            if diagnostics_failure:
                destination = root / "not-a-directory"
                destination.write_text("diagnostic writes must fail")
            context = (patch.object(runner, "snapshot", side_effect=fail_second_snapshot)
                       if diagnostics_failure else nullcontext())
            with context:
                self.assertEqual(runner.run([sys.executable, "-c", code], .5, destination), 124)
            pids = json.loads(pidfile.read_text())
            if not diagnostics_failure:
                rows = json.loads((root / "owned-processes.json").read_text())
                self.assertEqual({row["pid"] for row in rows}, set(pids))
                self.assertEqual(len({row["group"] for row in rows}), 2)
            for pid in pids:
                with self.assertRaises(ProcessLookupError):
                    os.kill(pid, 0)


if __name__ == "__main__":
    unittest.main()
