import importlib.util
import json
import os
from pathlib import Path
import sys
import subprocess
import select
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


def cross_group_fixture_code(ready):
    return "\n".join([
        "import os,signal,subprocess,sys,json",
        # EOF still releases the descendant if the fixture parent is killed.
        "child=subprocess.Popen([sys.executable,'-c','import sys; print(\"READY\",flush=True); sys.stdin.buffer.read()'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,start_new_session=True)",
        # Popen completion proves exec, not completion of interpreter startup.
        # The test controller's ReadyPipe watchdog bounds this fixture handshake.
        "assert child.stdout.readline()==b'READY\\n', 'descendant did not initialize'",
        "child.stdout.close()",
        "def stop(*_):",
        " child.wait()",
        " " + ready.write_code("{'state':'reaped'}"),
        " raise SystemExit(0)",
        "signal.signal(signal.SIGTERM,stop)",
        ready.write_code("{'state':'ready','pids':[os.getpid(),child.pid]}"),
        "signal.pause()",
    ])


class TestRunnerTests(unittest.TestCase):
    def require_inventory(self):
        # An unavailable OS ownership API is a missing integration prerequisite,
        # not permission to spawn children that the runner cannot enumerate.
        try:
            rows=runner.snapshot(os.getpid())
        except (OSError, subprocess.SubprocessError) as error:
            raise unittest.SkipTest("Process inventory unavailable: " + str(error))
        if not any(row['pid']==os.getpid() for row in rows):
            raise unittest.SkipTest("Process inventory unavailable in this sandbox")

    def own_fixture(self, process):
        def cleanup():
            if process.poll() is None: process.kill()
            process.wait(timeout=30)  # Outer cleanup guard, not a pass threshold.
        self.addCleanup(cleanup)

    def test_missing_inventory_refuses_before_spawning_any_fixture(self):
        with patch.object(runner, 'snapshot', return_value=[]), patch.object(runner.subprocess, 'Popen') as spawn:
            with self.assertRaises(unittest.SkipTest):
                self.check_cross_group_cleanup()
            spawn.assert_not_called()

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
                def completed(process, timeout):
                    self.own_fixture(process)
                    self.assertEqual(timeout, 2)
                    # Actual exit is causal; this separate bound is only the
                    # owned-child infrastructure guard, not the tested deadline.
                    return process.wait(timeout=30)
                self.assertEqual(runner.run([sys.executable, "-c",
                    f"import os,sys; assert os.environ['NSUnbufferedIO']=='YES'; sys.exit({status})"],
                    2, Path(directory), wait_for_exit=completed), status)

    def test_timeout_kills_and_reaps_owned_uncooperative_child(self):
        self.require_inventory()
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
                self.own_fixture(process)
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
        self.require_inventory()
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            ready=support.ReadyPipe(root)
            self.addCleanup(ready.close)
            pids=[]
            code="\n".join(["import os,signal,json", "signal.signal(signal.SIGTERM,signal.SIG_IGN)",
                ready.write_code("{'pid':os.getpid()}"), "signal.pause()"])
            def failed_wait(process, timeout):
                self.own_fixture(process)
                pids.append(ready.receive()['pid'])
                raise AssertionError('injected readiness failure')
            with self.assertRaisesRegex(AssertionError, 'injected readiness failure'):
                runner.run([sys.executable,'-c',code],15,root,
                    wait_for_exit=failed_wait,cleanup_grace=lambda:None)
            with self.assertRaises(ProcessLookupError): os.kill(pids[0],0)

    def test_descendant_exits_on_parent_pipe_close_even_if_inventory_is_lost(self):
        self.require_inventory()
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            ready=support.ReadyPipe(root)
            self.addCleanup(ready.close)
            events=select.kqueue()
            self.addCleanup(events.close)
            child=[]
            code="\n".join([
                "import os,signal,json,subprocess,sys",
                "child=subprocess.Popen([sys.executable,'-c','import sys; sys.stdin.buffer.read()'],stdin=subprocess.PIPE,start_new_session=True)",
                ready.write_code("{'parent':os.getpid(),'child':child.pid}"),
                "signal.pause()",
            ])
            def deadline(process, timeout):
                self.own_fixture(process)
                message=ready.receive()
                self.assertEqual(message['parent'],process.pid)
                child.append(message['child'])
                events.control([select.kevent(child[0],filter=select.KQ_FILTER_PROC,
                    flags=select.KQ_EV_ADD|select.KQ_EV_ONESHOT,fflags=select.KQ_NOTE_EXIT)],0,0)
                raise subprocess.TimeoutExpired(process.args,timeout)
            with patch.object(runner,'snapshot',return_value=[]):
                self.assertEqual(runner.run([sys.executable,'-c',code],15,root,
                    wait_for_exit=deadline,cleanup_grace=lambda:None),124)
            exited=events.control(None,1,30)  # Outer hang guard on the real exit event.
            self.assertEqual([event.ident for event in exited],child)
            self.assertTrue(exited[0].fflags & select.KQ_NOTE_EXIT)

    def test_grace_hook_failure_still_attempts_descendant_kill(self):
        self.check_cross_group_cleanup(grace_failure=True)

    def test_timeout_cleans_descendant_in_its_own_process_group(self):
        self.check_cross_group_cleanup()

    def test_diagnostic_io_and_later_snapshot_failure_do_not_skip_known_cleanup(self):
        self.check_cross_group_cleanup(diagnostics_failure=True)

    def test_parent_readiness_requires_descendant_startup_event(self):
        # Force the interleaving: Popen returns while the child has not completed
        # startup. Only consuming its actual readiness event may advance it.
        from types import SimpleNamespace
        initialized, published = [], []
        def child_ready():
            self.assertEqual(published, [])
            initialized.append(True)
            return b'READY\n'
        def publish(message):
            self.assertEqual(initialized, [True], 'parent announced ready before descendant')
            published.append(message)
        child = SimpleNamespace(pid=42, stdout=SimpleNamespace(readline=child_ready, close=lambda: None))
        ready = SimpleNamespace(write_code=lambda expression: 'publish(' + expression + ')')
        with patch.object(subprocess, 'Popen', return_value=child), \
             patch.object(runner.signal, 'signal'), patch.object(runner.signal, 'pause'):
            exec(cross_group_fixture_code(ready), {'publish': publish})
        self.assertEqual(published, [{'state':'ready','pids':[os.getpid(),42]}])

    def check_cross_group_cleanup(self, diagnostics_failure=False, grace_failure=False):
        self.require_inventory()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            ready = support.ReadyPipe(root)
            self.addCleanup(ready.close)
            pids = []
            code = cross_group_fixture_code(ready)
            destination = root
            original_snapshot = runner.snapshot
            calls = []
            def checked_snapshot(pid):
                calls.append(pid)
                if diagnostics_failure and len(calls) > 1:
                    raise OSError("controlled later snapshot failure")
                rows = original_snapshot(pid)
                if len(calls) == 1:
                    # A missing process is a failed fixture precondition, not
                    # permission to retry inventory until the test passes.
                    evidence = {'expected_pids':pids,'rows':rows}
                    (root/'initial-inventory.json').write_text(json.dumps(evidence,indent=2))
                    self.assertEqual({row['pid'] for row in rows},set(pids),
                        'Initial inventory incomplete after descendant readiness: '+json.dumps(evidence))
                return rows
            if diagnostics_failure:
                destination = root / "not-a-directory"
                destination.write_text("diagnostic writes must fail")
            context = patch.object(runner, "snapshot", side_effect=checked_snapshot)
            signal_results = []
            original_signal = runner.signal_owned
            def signal_and_record(row, action):
                delivered = original_signal(row, action)
                signal_results.append({'pid':row['pid'],'action':action,'delivered':delivered})
                return delivered
            def deadline(process, timeout):
                self.own_fixture(process)
                self.assertEqual(timeout, 15)
                message = ready.receive()
                self.assertEqual(message['state'], 'ready')
                self.assertEqual(message['pids'][0], process.pid)
                pids.extend(message['pids'])
                raise subprocess.TimeoutExpired(process.args, timeout)
            def grace():
                self.assertEqual({r['pid'] for r in signal_results
                    if r['action']==runner.signal.SIGTERM and r['delivered']},set(pids),
                    'Owned TERM was refused: '+json.dumps(signal_results))
                # The actual parent confirms it reaped the descendant before
                # cleanup proceeds; no grace-period speed assumption.
                self.assertEqual(ready.receive(), {'state':'reaped'})
                if grace_failure: raise AssertionError('injected grace failure')
            with context, patch.object(runner, "signal_owned", side_effect=signal_and_record) as signalled:
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
