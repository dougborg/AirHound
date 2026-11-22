"""Cross-process UART ownership tests, using PTYs and synthetic upstream modules."""
import fcntl
import hashlib
import importlib.util
import os
from pathlib import Path
import pty
import select
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HELPER = Path(__file__).parents[1] / 'sniffle-source.py'
SPEC = importlib.util.spec_from_file_location('sniffle_source_ownership', HELPER)
helper = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(helper)
RUNNER = '''import importlib.util,sys
from pathlib import Path
s=importlib.util.spec_from_file_location('helper',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
m.LEASE_ROOT=Path(sys.argv[2]);sys.argv=['helper']+sys.argv[3:]
try: m.main()
except Exception as e: print(str(e),file=sys.stderr);sys.exit(1)
'''
HARDWARE = '''import os,time,types
class SniffleHW:
 def __init__(self,port,**kwargs):
  fd=os.open(port,os.O_RDWR|os.O_NOCTTY);os.write(fd,b'OPEN')
  self.ser=types.SimpleNamespace(close=lambda:os.close(fd))
 def cmd_marker(self,marker): pass
 def probe_fw_version(self):
  if os.environ.get('HOLD_QUERY'):
   print('QUERY_READY',flush=True);time.sleep(20)
  return types.SimpleNamespace(major=1,minor=11,revision=0,api_level=0)
 def setup_sniffer(self,**kwargs): pass
 def recv_and_decode(self): time.sleep(.02)
'''


class SniffleOwnershipTests(unittest.TestCase):
    def test_normalizes_macos_aliases_and_real_symlinks(self):
        with patch.object(helper.os.path, 'realpath', side_effect=lambda x: x):
            self.assertEqual(helper.port_identity('/dev/cu.usbmodem1'),
                             helper.port_identity('/dev/tty.usbmodem1'))
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'real'
            path.touch()
            alias = Path(root) / 'alias'
            alias.symlink_to(path)
            self.assertEqual(helper.port_identity(str(alias)), str(path.resolve()))

    def test_unsafe_lock_nodes_fail_without_device_access(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root) / 'locks'
            directory.mkdir(mode=0o700)
            name = hashlib.sha256(helper.port_identity('/dev/fixture').encode()).hexdigest()+'.lock'
            lock = directory / name
            lock.symlink_to(Path(root) / 'missing')
            with patch.object(helper, 'LEASE_ROOT', directory):
                with self.assertRaises(OSError):
                    with helper.port_lease('/dev/fixture'): self.fail('acquired symlink lock')
                lock.unlink()
                os.mkfifo(lock, 0o600)
                with self.assertRaises(RuntimeError):
                    with helper.port_lease('/dev/fixture'): self.fail('acquired FIFO lock')

    def test_borrowed_descriptor_preserves_parent_lock_and_rejects_wrong_inode(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root) / 'locks'
            directory.mkdir(mode=0o700)
            port = '/dev/borrowed-fixture'
            lock = directory / (hashlib.sha256(helper.port_identity(port).encode()).hexdigest() + '.lock')
            fd = os.open(lock, os.O_RDWR | os.O_CREAT, 0o600)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                with patch.object(helper, 'LEASE_ROOT', directory):
                    with helper.borrowed_port_lease(port, os.dup(fd)):
                        with self.assertRaisesRegex(RuntimeError, 'already owned'):
                            with helper.port_lease(port): self.fail('second owner')
                    # Child close must never LOCK_UN the parent's shared description.
                    with self.assertRaisesRegex(RuntimeError, 'already owned'):
                        with helper.port_lease(port): self.fail('premature release')
                    wrong = os.open(directory / 'wrong', os.O_RDWR | os.O_CREAT, 0o600)
                    try:
                        with self.assertRaises(helper.HelperError):
                            with helper.borrowed_port_lease(port, wrong): self.fail('wrong inode')
                    finally:
                        os.close(wrong)
            finally:
                os.close(fd)
            with patch.object(helper, 'LEASE_ROOT', directory):
                with helper.port_lease(port): pass

    def test_invalid_borrowed_fd_is_terminal_before_device_access(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root) / 'locks'
            directory.mkdir(mode=0o700)
            with patch.object(helper, 'LEASE_ROOT', directory):
                for fd in (-1, 0, 999999):
                    with self.assertRaises(helper.HelperError) as raised:
                        with helper.borrowed_port_lease('/dev/missing-fixture', fd):
                            self.fail('invalid inherited FD accepted')
                    self.assertEqual(raised.exception.code, 'invalid_borrowed_lease')
                    self.assertFalse(raised.exception.retryable)

    def _check_two_processes(self, first_identify):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            package = directory / 'sniffle'
            package.mkdir()
            (package / '__init__.py').write_text('')
            (package / 'sniffle_hw.py').write_text(HARDWARE)
            (package / 'constants.py').write_text('class SnifferMode: PASSIVE_SCAN=0\n')
            (package / 'packet_decoder.py').write_text('class PacketMessage: pass\n')
            (package / 'pcap.py').write_text("class PcapBleWriter:\n def __init__(self,output): output.write(b'CAPTURE_READY\\n');output.flush()\n")
            master, slave = pty.openpty()
            alias = directory / 'port-alias'
            alias.symlink_to(os.ttyname(slave))
            args = [sys.executable, '-c', RUNNER, str(HELPER), str(directory/'locks'),
                    '--module-path', str(directory), '--port', os.ttyname(slave)]
            env = dict(os.environ)
            if first_identify: env['HOLD_QUERY'] = '1'
            first = subprocess.Popen(args + (['--identify-only'] if first_identify else []),
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
            try:
                self.assertTrue(select.select([first.stdout], [], [], 5)[0], 'first helper never ready')
                self.assertIn(b'READY', first.stdout.readline())
                self.assertTrue(select.select([master], [], [], 2)[0])
                self.assertEqual(os.read(master, 4096), b'OPEN')
                # Symlink spelling must still conflict. Inverse modes cover both paths.
                second_args = args[:-1] + [str(alias)] + ([] if first_identify else ['--identify-only'])
                second = subprocess.run(second_args, capture_output=True, timeout=3)
                self.assertEqual(second.returncode, 1)
                self.assertIn(b'already owned', second.stderr)
                self.assertFalse(select.select([master], [], [], .2)[0], 'denied owner wrote UART')
                self.assertIsNone(first.poll(), 'denial disrupted first owner')
            finally:
                first.send_signal(signal.SIGTERM)
                try: first.communicate(timeout=2)
                except subprocess.TimeoutExpired: first.kill();first.communicate(timeout=2)
            # Exit, including termination during probe, releases the lease.
            final = subprocess.run(args + ['--identify-only'], capture_output=True, timeout=3)
            self.assertEqual(final.returncode, 0, final.stderr)
            os.close(master)
            os.close(slave)

    def test_capture_owner_excludes_identification_before_constructor(self):
        self._check_two_processes(False)

    def test_identification_owner_excludes_capture_before_constructor(self):
        self._check_two_processes(True)
