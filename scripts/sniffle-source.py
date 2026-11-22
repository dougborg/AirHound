#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Separate Sniffle adapter process. Requires user-installed NCC Group Sniffle 1.11.0.

Writes classic DLT256 PCAP to stdout; diagnostics only to stderr. Never flashes,
selects an arbitrary serial port, actively scans, or follows connections.
"""
import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import os
from pathlib import Path
import stat
import json
import logging
import signal
import sys
import time


# Shared across backend sockets, app bundles and TMPDIR environments for this UID.
# Keep lock files after release: unlinking a locked inode permits split ownership.
LEASE_ROOT = Path('/tmp') / ('airhound-serial-' + str(os.geteuid()))


def port_identity(port):
    canonical = os.path.realpath(port)
    if canonical.startswith('/dev/cu.'):
        canonical = '/dev/tty.' + canonical[len('/dev/cu.'):]
    return canonical


@contextmanager
def port_lease(port):
    """Coordinate AirHound helper processes before upstream can open/write UART.

    This cooperative lease covers current AirHound helpers, including cu/tty and
    symlink aliases. It cannot exclude unrelated tools or older helper versions.
    No device descriptor is opened while checking/acquiring the lease.
    """
    try:
        LEASE_ROOT.mkdir(mode=0o700)
    except FileExistsError:
        pass
    metadata = LEASE_ROOT.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) != 0o700:
        raise RuntimeError('Sniffle lease directory must be owned and private (0700)')
    name = hashlib.sha256(port_identity(port).encode()).hexdigest() + '.lock'
    fd = os.open(str(LEASE_ROOT / name), os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, 0o600)
    acquired = False
    try:
        metadata = os.fstat(fd)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) != 0o600:
            raise RuntimeError('Sniffle lease must be an owned private regular file')
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError('Sniffle port is already owned by another AirHound source') from error
        acquired = True
        yield
    finally:
        try:
            if acquired:
                fcntl.flock(fd, fcntl.LOCK_UN)
        finally:
            os.close(fd)


CAPABILITIES = {"type": "sniffle_helper_capabilities", "schema_version": 1,
                "capabilities": ["borrowed_serial_lease_v1", "structured_errors_v1"]}


class HelperError(RuntimeError):
    def __init__(self, code, retryable):
        super().__init__(code)
        self.code, self.retryable = code, retryable


@contextmanager
def borrowed_port_lease(port, fd):
    """Borrow the supervisor's open-file-description lock; never unlock it.

    The exact private inode and endpoint binding must match before upstream can
    open UART. Re-locking the inherited descriptor proves it is not blocked by
    another owner; unlike a separate open, this preserves the parent's flock.
    """
    try:
        if fd < 3:
            raise HelperError('invalid_borrowed_lease', False)
        metadata = LEASE_ROOT.lstat()
        if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) != 0o700:
            raise HelperError('invalid_borrowed_lease', False)
        name = hashlib.sha256(port_identity(port).encode()).hexdigest() + '.lock'
        expected = (LEASE_ROOT / name).lstat()
        actual = os.fstat(fd)
        if (not stat.S_ISREG(expected.st_mode) or not stat.S_ISREG(actual.st_mode)
                or expected.st_uid != os.geteuid() or actual.st_uid != os.geteuid()
                or stat.S_IMODE(expected.st_mode) != 0o600 or stat.S_IMODE(actual.st_mode) != 0o600
                or (expected.st_dev, expected.st_ino) != (actual.st_dev, actual.st_ino)):
            raise HelperError('invalid_borrowed_lease', False)
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        os.set_inheritable(fd, False)
    except (OSError, ValueError) as error:
        raise HelperError('invalid_borrowed_lease', False) from error
    try:
        yield
    finally:
        # Parent owns release after child reaping, including cancellation paths.
        os.close(fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port')
    parser.add_argument('--capabilities', action='store_true')
    parser.add_argument('--lease-fd', type=int, help=argparse.SUPPRESS)
    parser.add_argument('--module-path', help='Directory containing upstream sniffle package')
    parser.add_argument('--channel', type=int, choices=(37, 38, 39), default=37)
    parser.add_argument('--coded', action='store_true')
    parser.add_argument('--identify-only', action='store_true',
                        help='Query firmware/API and exit without configuring or starting capture')
    args = parser.parse_args()
    if args.capabilities:
        print(json.dumps(CAPABILITIES), flush=True)
        return
    if not args.port:
        parser.error('--port is required for identification or capture')
    # SniffleHW's constructor writes a marker; ownership precedes construction.
    lease = port_lease(args.port) if args.lease_fd is None else borrowed_port_lease(args.port, args.lease_fd)
    with lease:
        run(args)


def run(args):
    if args.module_path:
        sys.path.insert(0, args.module_path)
    from sniffle.sniffle_hw import SniffleHW
    from sniffle.constants import SnifferMode
    from sniffle.packet_decoder import PacketMessage
    from sniffle.pcap import PcapBleWriter

    logger = logging.getLogger('airhound-sniffle')
    class DecodeErrors(logging.Handler):
        count = 0

        def emit(self, record):
            if record.levelno >= logging.ERROR:
                self.count += 1
                print('Sniffle decoder rejected a transport record', file=sys.stderr, flush=True)

    decoder_errors = DecodeErrors()
    logger.addHandler(decoder_errors)
    logger.propagate = False
    try:
        hw = SniffleHW(args.port, timeout=0.25, baudrate=921600, logger=logger)
    except BaseException:
        logger.removeHandler(decoder_errors)
        raise
    stopped = False

    def stop(_signal, _frame):
        nonlocal stopped
        stopped = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    errors = 0
    try:
        time.sleep(0.7)
        hw.cmd_marker(b'airhound')
        version = hw.probe_fw_version()
        if version is not None:
            identity = {'type': 'sniffle_identity', 'schema_version': 1,
                        'major': version.major, 'minor': version.minor,
                        'patch': version.revision, 'api_level': version.api_level}
            print(json.dumps(identity), file=sys.stdout if args.identify_only else sys.stderr,
                  flush=True)
            if args.identify_only:
                return
        if version is None:
            raise HelperError('firmware_query_timeout', True)
        if (version.major, version.minor, version.revision, version.api_level) != (1, 11, 0, 0):
            raise HelperError('unsupported_firmware', False)
        print('Sniffle 1.11.0/API0: passive advertisements, fixed channel', args.channel,
              'coded' if args.coded else '1M', file=sys.stderr, flush=True)
        hw.setup_sniffer(mode=SnifferMode.PASSIVE_SCAN, chan=args.channel,
                         ext_adv=True, coded_phy=args.coded, validate_crc=True)
        writer = PcapBleWriter(sys.stdout.buffer)
        sys.stdout.buffer.flush()
        while not stopped:
            if errors + decoder_errors.count >= 10:
                raise HelperError('transport_decode_errors', True)
            try:
                packet = hw.recv_and_decode()
            except Exception as exc:
                # A malformed transport record is visible and does not turn into evidence.
                errors += 1
                print('Sniffle transport/decode error:', type(exc).__name__, file=sys.stderr, flush=True)
                if errors >= 10:
                    raise HelperError('transport_decode_errors', True) from exc
                continue
            if isinstance(packet, PacketMessage):
                writer.write_packet_message(packet)
                sys.stdout.buffer.flush()
    finally:
        hw.ser.close()
        print('Sniffle session ended; decode errors:', errors + decoder_errors.count, file=sys.stderr, flush=True)
        logger.removeHandler(decoder_errors)


if __name__ == '__main__':
    try:
        main()
    except (Exception, KeyboardInterrupt) as error:
        if isinstance(error, HelperError):
            code, retryable = error.code, error.retryable
        elif isinstance(error, PermissionError):
            code, retryable = 'serial_permission_denied', False
        elif isinstance(error, OSError):
            code, retryable = 'serial_transport_unavailable', True
        elif isinstance(error, ImportError):
            code, retryable = 'missing_python_dependency', False
        else:
            code, retryable = 'helper_configuration_or_protocol_error', False
        print(json.dumps({'type': 'sniffle_error', 'schema_version': 1,
                          'code': code, 'retryable': retryable}), file=sys.stderr, flush=True)
        sys.exit(1)
