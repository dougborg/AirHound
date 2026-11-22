#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Separate Sniffle adapter process. Requires user-installed NCC Group Sniffle 1.11.0.

Writes classic DLT256 PCAP to stdout; diagnostics only to stderr. Never flashes,
selects an arbitrary serial port, actively scans, or follows connections.
"""
import argparse
import json
import logging
import signal
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', required=True)
    parser.add_argument('--module-path', help='Directory containing upstream sniffle package')
    parser.add_argument('--channel', type=int, choices=(37, 38, 39), default=37)
    parser.add_argument('--coded', action='store_true')
    parser.add_argument('--identify-only', action='store_true',
                        help='Query firmware/API and exit without configuring or starting capture')
    args = parser.parse_args()
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
    hw = SniffleHW(args.port, timeout=0.25, baudrate=921600, logger=logger)
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
