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
LEASE_ROOT = Path('/tmp') / ('airhound-sniffle-' + str(os.getuid()))


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
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != 0o700:
        raise RuntimeError('Sniffle lease directory must be owned and private (0700)')
    name = hashlib.sha256(port_identity(port).encode()).hexdigest() + '.lock'
    fd = os.open(str(LEASE_ROOT / name), os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, 0o600)
    try:
        metadata = os.fstat(fd)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != 0o600:
            raise RuntimeError('Sniffle lease must be an owned private regular file')
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError('Sniffle port is already owned by another AirHound helper') from error
        yield
    finally:
        os.close(fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', required=True)
    parser.add_argument('--module-path', help='Directory containing upstream sniffle package')
    parser.add_argument('--channel', type=int, choices=(37, 38, 39), default=37)
    parser.add_argument('--coded', action='store_true')
    parser.add_argument('--identify-only', action='store_true',
                        help='Query firmware/API and exit without configuring or starting capture')
    args = parser.parse_args()
    # SniffleHW's constructor writes a marker; ownership must precede construction.
    with port_lease(args.port):
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
        if version is None or (version.major, version.minor, version.revision, version.api_level) != (1, 11, 0, 0):
            raise RuntimeError('requires verified Sniffle firmware 1.11.0 / API0')
        print('Sniffle 1.11.0/API0: passive advertisements, fixed channel', args.channel,
              'coded' if args.coded else '1M', file=sys.stderr, flush=True)
        hw.setup_sniffer(mode=SnifferMode.PASSIVE_SCAN, chan=args.channel,
                         ext_adv=True, coded_phy=args.coded, validate_crc=True)
        writer = PcapBleWriter(sys.stdout.buffer)
        sys.stdout.buffer.flush()
        while not stopped:
            if errors + decoder_errors.count >= 10:
                raise RuntimeError('too many Sniffle transport/decode errors')
            try:
                packet = hw.recv_and_decode()
            except Exception as exc:
                # A malformed transport record is visible and does not turn into evidence.
                errors += 1
                print('Sniffle transport/decode error:', type(exc).__name__, file=sys.stderr, flush=True)
                if errors >= 10:
                    raise RuntimeError('too many Sniffle transport/decode errors') from exc
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
        print('Sniffle source failed:', type(error).__name__, str(error), file=sys.stderr)
        sys.exit(1)
