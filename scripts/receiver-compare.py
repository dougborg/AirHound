#!/usr/bin/env python3
"""Offline controlled-window comparison: beacon-v1 NDJSON versus tshark TSV.

Raw BSSIDs stay in process memory; the output contains aggregate counts only.
No PCAP decoding dependency is bundled. Export Linux TSV with the documented
four tshark fields before running this tool. Controlled conditions are operator
attestations, not independently verified by this script.
"""
import argparse
import json
import math
import re
import statistics
import sys
from pathlib import Path

MAX_BYTES = 64 * 1024 * 1024
MAX_LINE = 16 * 1024
MAX_BSSIDS = 100000
CONTROLS = ('same_adapter', 'same_antenna', 'same_position', 'same_capture_filter',
            'controlled_ap_set', 'stationary_scene', 'no_channel_hopping')


def number(value):
    return type(value) in (int, float) and math.isfinite(value)


def validate_controls(value, exploratory=False):
    if not isinstance(value, dict) or value.get('schema_version') != 1:
        raise ValueError('controls require schema_version 1')
    required = [key for key in CONTROLS if not (exploratory and key == 'controlled_ap_set')]
    if any(value.get(key) is not True for key in required):
        raise ValueError('every documented controlled-condition attestation must be true')
    if exploratory and type(value.get('controlled_ap_set')) is not bool:
        raise ValueError('exploratory controlled_ap_set must still be an explicit boolean')
    if type(value.get('channel')) is not int or value['channel'] not in (*range(1, 14), *range(36, 65, 4), *range(100, 145, 4), *range(149, 166, 4)):
        raise ValueError('unsupported fixed receive channel')
    if value.get('bandwidth_mhz') != 20:
        raise ValueError('only fixed HT20 comparisons are supported')
    lengths = []
    for key in ('mac_window_unix_s', 'linux_window_unix_s'):
        window = value.get(key)
        if not isinstance(window, list) or len(window) != 2 or not all(number(v) for v in window):
            raise ValueError('each window requires two finite Unix timestamps')
        start, end = window
        if not 0 <= start < end or end - start > 28800:
            raise ValueError('window duration must be positive and at most 8 hours')
        lengths.append(end - start)
    if abs(lengths[0] - lengths[1]) > 0.001:
        raise ValueError('comparison windows must have equal durations within 1ms')
    return value


def lines(path):
    with path.open('rb') as stream:
        total = 0
        while True:
            line = stream.readline(MAX_LINE + 1)
            if not line:
                break
            total += len(line)
            if len(line) > MAX_LINE or total > MAX_BYTES:
                raise ValueError('input exceeds line or file size bound')
            if not line.endswith(b'\n'):
                raise ValueError('truncated input line')
            yield line.decode('utf-8').rstrip('\r\n')


def bssid(value):
    if not isinstance(value, str) or not re.fullmatch(r'(?:[0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}', value):
        raise ValueError('invalid BSSID')
    octets = bytes.fromhex(value.replace(':', ''))
    if octets[0] & 1 or not any(octets):
        raise ValueError('invalid multicast or zero BSSID')
    return octets


def signal_value(value):
    if value is None or value == '':
        return None
    try:
        result = int(value)
    except (TypeError, ValueError):
        raise ValueError('invalid signal field') from None
    if isinstance(value, bool) or (isinstance(value, float) and value != result) or not -128 <= result <= 127:
        raise ValueError('invalid signal range')
    return result


class Capture:
    def __init__(self, channel, window):
        self.channel, self.window = channel, window
        self.by_bssid = {}
        self.count = self.outside_window = self.missing_signal = 0
        self.first = self.last = None

    def add(self, timestamp, address, channel, signal):
        if not number(timestamp) or timestamp < 0:
            raise ValueError('invalid capture timestamp')
        if channel != self.channel:
            raise ValueError('input contains a different receive channel; fixed channel required')
        if not self.window[0] <= timestamp < self.window[1]:
            self.outside_window += 1
            return
        self.count += 1
        self.first = timestamp if self.first is None else min(timestamp, self.first)
        self.last = timestamp if self.last is None else max(timestamp, self.last)
        if address not in self.by_bssid:
            if len(self.by_bssid) == MAX_BSSIDS:
                raise ValueError('too many distinct BSSIDs')
            self.by_bssid[address] = [0, 0, 0]
        item = self.by_bssid[address]
        item[0] += 1
        if signal is not None:
            item[1] += signal
            item[2] += 1
        else:
            self.missing_signal += 1

    def summary(self):
        return {'beacons': self.count, 'distinct_bssids': len(self.by_bssid),
                'beacons_per_second': self.count / (self.window[1] - self.window[0]),
                'outside_window': self.outside_window, 'missing_signal': self.missing_signal,
                'observed_packet_span_seconds': self.last - self.first if self.first is not None else None}


def read_mac(path, controls):
    capture = Capture(controls['channel'], controls['mac_window_unix_s'])
    session, next_sequence = None, 0
    for line in lines(path):
        record = json.loads(line)
        if not isinstance(record, dict) or record.get('schema_version') != 1 or record.get('type') != 'beacon':
            raise ValueError('expected beacon-v1 observation file (not mixed control stream)')
        current = record.get('session_id')
        if not isinstance(current, str) or not current:
            raise ValueError('missing capture session')
        if session is None:
            session = current
        if current != session or type(record.get('sequence')) is not int or record['sequence'] != next_sequence:
            raise ValueError('expected one complete, consecutive capture session')
        next_sequence += 1
        timestamp = record.get('observed_at_unix_ms')
        if type(timestamp) is not int:
            raise ValueError('invalid observation timestamp')
        if record.get('rssi_validated') is not False or record.get('rssi_estimate_basis') != 'mt76_eeprom_formula':
            raise ValueError('expected explicit unvalidated EEPROM signal estimate')
        capture.add(timestamp / 1000, bssid(record.get('bssid')), record.get('receive_channel'),
                    signal_value(record.get('estimated_rssi_dbm')))
    return capture


def frequency_channel(value):
    try:
        frequency = int(value)
    except ValueError:
        raise ValueError('Linux radiotap receive frequency is missing or invalid') from None
    if 2412 <= frequency <= 2462 and (frequency - 2407) % 5 == 0:
        return (frequency - 2407) // 5
    if frequency in (5180, 5200, 5220, 5240):
        return (frequency - 5000) // 5
    raise ValueError('unsupported Linux receive frequency')


def read_linux(path, controls):
    capture = Capture(controls['channel'], controls['linux_window_unix_s'])
    for line in lines(path):
        fields = line.split('\t')
        if len(fields) != 4:
            raise ValueError('expected exactly four documented tshark TSV fields')
        try:
            timestamp = float(fields[0])
        except ValueError:
            raise ValueError('invalid Linux packet timestamp') from None
        capture.add(timestamp, bssid(fields[1]), frequency_channel(fields[2]), signal_value(fields[3]))
    return capture


def compare(mac, linux, controls, exploratory=False):
    matched = mac.by_bssid.keys() & linux.by_bssid.keys()
    deltas = []
    for address in matched:
        _, mac_total, mac_samples = mac.by_bssid[address]
        _, linux_total, linux_samples = linux.by_bssid[address]
        if mac_samples and linux_samples:
            deltas.append(mac_total / mac_samples - linux_total / linux_samples)
    return {
        'schema_version': 1, 'controlled_conditions_operator_attested': not exploratory,
        'comparison_kind': 'exploratory' if exploratory else 'controlled',
        'validation_passed': False,
        'limitations': (['Ambient AP scene is not controlled; time variation confounds recall and RSSI.'] if exploratory else []) +
                       ['No release acceptance thresholds or calibrated RSSI validation are applied.'],
        'channel': controls['channel'], 'bandwidth_mhz': controls['bandwidth_mhz'],
        'window_seconds': mac.window[1] - mac.window[0],
        'mac': mac.summary(), 'linux': linux.summary(), 'matched_bssids': len(matched),
        'mac_bssid_recall_relative_to_linux': len(matched) / len(linux.by_bssid) if linux.by_bssid else None,
        'mac_beacon_count_ratio_to_linux': mac.count / linux.count if linux.count else None,
        'signal_delta': {
            'basis': 'mean mac estimated RSSI minus mean Linux radiotap RSSI per matched BSSID',
            'matched_bssids_with_signal': len(deltas),
            'mean_db': statistics.mean(deltas) if deltas else None,
            'median_db': statistics.median(deltas) if deltas else None,
            'min_db': min(deltas) if deltas else None, 'max_db': max(deltas) if deltas else None,
            'mac_rssi_validated': False},
        'interpretation': (
            'Exploratory sequential ambient-scene comparison; AP conditions are uncontrolled and time variation confounds recall and RSSI. '
            if exploratory else 'Descriptive sequential controlled-window comparison; '
        ) + 'Linux detections are a reference, not ground truth. No sensitivity or calibrated RSSI claim.',
        'conclusive': bool(mac.count and linux.count) and not exploratory}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mac', required=True, type=Path)
    parser.add_argument('--linux-tsv', required=True, type=Path)
    parser.add_argument('--controls', required=True, type=Path)
    parser.add_argument('--exploratory', action='store_true', help='Allow an uncontrolled AP set; never a release-validation pass')
    args = parser.parse_args(argv)
    try:
        if args.controls.stat().st_size > MAX_LINE:
            raise ValueError('controls file too large')
        controls = validate_controls(json.loads(args.controls.read_text()), args.exploratory)
        report = compare(read_mac(args.mac, controls), read_linux(args.linux_tsv, controls), controls, args.exploratory)
    except (OSError, ValueError, UnicodeError):
        # Do not echo raw input or exception text containing captured identifiers.
        print('Comparison input invalid: verify bounded files, beacon-v1 schema, TSV fields, and controlled-window manifest.', file=sys.stderr)
        return 2
    print(json.dumps(report, indent=2, allow_nan=False))
    return 0 if report['conclusive'] else 1


if __name__ == '__main__':
    sys.exit(main())
