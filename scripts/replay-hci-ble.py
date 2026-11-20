#!/usr/bin/env python3
"""Offline H4 event replay only. No USB library, device access, HCI commands or scan.

Input: one contiguous hexadecimal H4 Event packet per line. Output is an
experimental report stream, NOT the hostd device/receiver v1 protocol.
"""
import argparse
import json
import re
import sys

MAX_PACKET = 258  # H4 type + HCI event code + u8 parameter length + parameters
MAX_LINE = MAX_PACKET * 2 + 2  # optional CRLF; no arbitrary whitespace
ADDRESS_TYPES = {0: 'public', 1: 'random', 2: 'public_identity', 3: 'random_identity', 255: 'anonymous'}


class InvalidEvent(ValueError):
    pass


def signal(value):
    if value == 127:
        return None
    result = value if value < 128 else value - 256
    if not -127 <= result <= 20:
        raise InvalidEvent('reserved signal-strength value')
    return result


def identity(kind, address, extended):
    if kind not in ADDRESS_TYPES or (kind == 255 and not extended):
        raise InvalidEvent('reserved address type')
    return {'address_type': ADDRESS_TYPES[kind], 'hci_address_type': kind,
            'address': None if kind == 255 else ':'.join(f'{b:02X}' for b in reversed(address))}


def parse_event(packet):
    """Return reports atomically; a malformed report rejects the entire event.

    Extended records remain unassembled fragments, even if their controller
    data-status marks the final fragment complete. No cross-record state exists.
    """
    if len(packet) < 3 or len(packet) > MAX_PACKET or packet[0] != 4:
        raise InvalidEvent('expected a bounded H4 event packet')
    if len(packet) != 3 + packet[2]:
        raise InvalidEvent('HCI parameter-length mismatch')
    if packet[1] != 0x3e:
        return []
    data = packet[3:]
    if not data:
        raise InvalidEvent('missing LE subevent')
    if data[0] not in (2, 13):
        return []
    extended = data[0] == 13
    if len(data) < 2 or not 1 <= data[1] <= (10 if extended else 25):
        raise InvalidEvent('invalid report count')
    reports, pos = [], 2
    for _ in range(data[1]):
        header_size = 24 if extended else 9
        if len(data) - pos < header_size:
            raise InvalidEvent('truncated report header')
        header = data[pos:pos + header_size]
        pos += header_size
        length = header[-1]
        tail_size = 0 if extended else 1  # legacy RSSI follows its payload
        if (not extended and length > 31) or len(data) - pos < length + tail_size:
            raise InvalidEvent('invalid or truncated report payload')
        payload = data[pos:pos + length]
        pos += length
        if extended:
            event_type = int.from_bytes(header[:2], 'little')
            status = (event_type >> 5) & 3
            if event_type & ~0x7f or status == 3:
                raise InvalidEvent('reserved extended event flags')
            address = identity(header[2], header[3:9], True)
            sid = header[11]
            if sid not in (*range(16), 255):
                raise InvalidEvent('reserved advertising SID')
            report = {**address, 'event_type': event_type,
                      'scan_response': bool(event_type & 8),
                      'legacy_pdu': bool(event_type & 16),
                      'primary_phy_code': header[9], 'secondary_phy_code': header[10],
                      'advertising_sid': None if sid == 255 else sid,
                      'tx_power': signal(header[12]), 'rssi': signal(header[13]),
                      'periodic_interval_units': int.from_bytes(header[14:16], 'little'),
                      'controller_data_status': ('complete', 'more', 'truncated')[status]}
        else:
            event_type = header[0]
            if event_type > 4:
                raise InvalidEvent('reserved legacy event type')
            report = {**identity(header[1], header[2:8], False),
                      'event_type': event_type, 'scan_response': event_type == 4,
                      'legacy_pdu': True, 'primary_phy_code': 1, 'secondary_phy_code': 0,
                      'advertising_sid': None, 'tx_power': None,
                      'rssi': signal(data[pos]), 'controller_data_status': 'complete'}
            pos += 1
        reports.append({**report, 'report_kind': 'extended' if extended else 'legacy',
                        'data_hex': payload.hex(), 'data_bytes': len(payload),
                        'reassembled': False})
    if pos != len(data):
        raise InvalidEvent('trailing data after reports')
    return reports


def replay(source, output, receiver_id, session_id):
    counts = {'events': 0, 'reports': 0, 'ignored_events': 0}
    while True:
        line = source.readline(MAX_LINE + 1)
        if not line:
            return counts
        if len(line) > MAX_LINE:
            raise InvalidEvent('input line exceeds HCI event limit')
        line = line.rstrip(b'\r\n')
        if not line or len(line) % 2 or not re.fullmatch(rb'[0-9a-fA-F]+', line):
            raise InvalidEvent('expected contiguous hexadecimal event line')
        reports = parse_event(bytes.fromhex(line.decode('ascii')))
        counts['events'] += 1
        counts['ignored_events'] += not reports
        for index, report in enumerate(reports):
            value = {'type': 'experimental_ble_hci_report', 'schema_version': 0,
                     'source': 'hci_replay', 'receiver_id': receiver_id,
                     'session_id': session_id, 'event_index': counts['events'] - 1,
                     'report_index': index, 'provenance': 'controller_report_replay',
                     'live_reception_validated': False, **report}
            output.write(json.dumps(value, separators=(',', ':')) + '\n')
            counts['reports'] += 1
        output.flush()


def identifier(value):
    if not 1 <= len(value) <= 128 or not re.fullmatch(r'[A-Za-z0-9_.:/-]+', value):
        raise argparse.ArgumentTypeError('use 1..128 ASCII identifier characters')
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--receiver-id', required=True, type=identifier)
    parser.add_argument('--session-id', required=True, type=identifier)
    args = parser.parse_args()
    try:
        counts = replay(sys.stdin.buffer, sys.stdout, args.receiver_id, args.session_id)
    except (InvalidEvent, OSError) as error:
        # Never echo raw packets or identifiers in failure diagnostics.
        print(f'HCI replay failed: {error}', file=sys.stderr)
        return 1
    print(json.dumps(counts), file=sys.stderr)
    return 0


if __name__ == '__main__':
    sys.exit(main())
