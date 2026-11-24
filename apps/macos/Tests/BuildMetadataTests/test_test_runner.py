import importlib.util
from contextlib import nullcontext
import json
import os
from pathlib import Path
import sys
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / "run-tests.py"
spec = importlib.util.spec_from_file_location("native_test_runner", SCRIPT)
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)

SUPPORT = Path(__file__).resolve().parents[4] / "scripts/test-process-support.py"
support_spec = importlib.util.spec_from_file_location("test_process_support", SUPPORT)
support = importlib.util.module_from_spec(support_spec)
support_spec.loader.exec_module(support)


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
            ready = support.ReadyPipe(root)
            self.addCleanup(ready.close)
            code = "\n".join([
                "import os,signal,json",
                "signal.signal(signal.SIGTERM,signal.SIG_IGN)",
                ready.write_code("{'pid':os.getpid(),'state':'ready'}"),
                "signal.pause()",
            ])
            observed = []
            def deadline(process, timeout):
                self.assertEqual(timeout, 15)
                message = ready.receive()
                self.assertEqual(message, {'pid':process.pid,'state':'ready'})
                observed.append(process.pid)
                # Inject expiration after the explicit child barrier, independent
                # of launch speed; production cleanup still kills and reaps it.
                raise subprocess.TimeoutExpired(process.args, timeout)
            self.assertEqual(runner.run([sys.executable, "-c", code], 15, root,
                wait_for_exit=deadline, cleanup_grace=lambda: None), 124)
            with self.assertRaises(ProcessLookupError):
                os.kill(observed[0], 0)
            self.assertTrue((root / "owned-processes.json").exists())

    def test_readiness_hook_failure_still_kills_and_reaps_child(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            ready=support.ReadyPipe(root)
            self.addCleanup(ready.close)
            pids=[]
            code="\n".join(["import os,signal,json", "signal.signal(signal.SIGTERM,signal.SIG_IGN)",
                ready.write_code("{'pid':os.getpid()}"), "signal.pause()"])
            def failed_wait(process, timeout):
                pids.append(ready.receive()['pid'])
                raise AssertionError('injected readiness failure')
            with self.assertRaisesRegex(AssertionError, 'injected readiness failure'):
                runner.run([sys.executable,'-c',code],15,root,
                    wait_for_exit=failed_wait,cleanup_grace=lambda:None)
            with self.assertRaises(ProcessLookupError): os.kill(pids[0],0)

    def test_grace_hook_failure_still_attempts_descendant_kill(self):
        self.check_cross_group_cleanup(grace_failure=True)

    def test_timeout_cleans_descendant_in_its_own_process_group(self):
        self.check_cross_group_cleanup()

    def test_diagnostic_io_and_later_snapshot_failure_do_not_skip_known_cleanup(self):
        self.check_cross_group_cleanup(diagnostics_failure=True)

    def check_cross_group_cleanup(self, diagnostics_failure=False, grace_failure=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            ready = support.ReadyPipe(root)
            self.addCleanup(ready.close)
            pids = []
            code = "\n".join([
                "import os,signal,subprocess,sys,time,json,pathlib",
                "child=subprocess.Popen([sys.executable,'-c','import signal; signal.pause()'],start_new_session=True)",
                "def stop(*_):",
                " child.wait()",
                " " + ready.write_code("{'state':'reaped'}"),
                " raise SystemExit(0)",
                "signal.signal(signal.SIGTERM,stop)",
                ready.write_code("{'state':'ready','pids':[os.getpid(),child.pid]}"),
                "signal.pause()",
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
            def deadline(process, timeout):
                self.assertEqual(timeout, 15)
                message = ready.receive()
                self.assertEqual(message['state'], 'ready')
                self.assertEqual(message['pids'][0], process.pid)
                pids.extend(message['pids'])
                raise subprocess.TimeoutExpired(process.args, timeout)
            def grace():
                # The actual parent confirms it reaped the descendant before
                # cleanup proceeds; no grace-period speed assumption.
                self.assertEqual(ready.receive(), {'state':'reaped'})
                if grace_failure: raise AssertionError('injected grace failure')
            with context, patch.object(runner, "signal_owned", wraps=runner.signal_owned) as signalled:
                if grace_failure:
                    with self.assertRaisesRegex(AssertionError, 'injected grace failure'):
                        runner.run([sys.executable, "-c", code], 15, destination,
                            wait_for_exit=deadline, cleanup_grace=grace)
                    kills={call.args[0]['pid'] for call in signalled.call_args_list
                           if call.args[1] == runner.signal.SIGKILL}
                    self.assertEqual(kills,set(pids))
                else:
                    self.assertEqual(runner.run([sys.executable, "-c", code], 15, destination,
                        wait_for_exit=deadline, cleanup_grace=grace), 124)
            if not diagnostics_failure:
                rows = json.loads((root / "owned-processes.json").read_text())
                self.assertEqual({row["pid"] for row in rows}, set(pids))
                self.assertEqual(len({row["group"] for row in rows}), 2)
            for pid in pids:
                with self.assertRaises(ProcessLookupError):
                    os.kill(pid, 0)


if __name__ == "__main__":
    unittest.main()
