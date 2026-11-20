import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest

PATH = Path(__file__).parents[1] / 'replay-hci-ble.py'
spec = importlib.util.spec_from_file_location('hci_replay', PATH)
hci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hci)


def event(reports, extended=False):
    data = bytes([13 if extended else 2, len(reports)]) + b''.join(reports)
    return bytes([4, 0x3e, len(data)]) + data


def legacy(payload=b'\x02\x01\x06', kind=0, address_type=1, rssi=211):
    return bytes([kind, address_type]) + bytes.fromhex('665544332211') + bytes([len(payload)]) + payload + bytes([rssi])


def extended(payload=b'abc', status=0, address_type=0, sid=3, tx_power=127,
             rssi=127, primary_phy=3, secondary_phy=3):
    return ((status << 5).to_bytes(2, 'little') + bytes([address_type])
            + bytes.fromhex('665544332211') + bytes([primary_phy, secondary_phy, sid, tx_power, rssi])
            + bytes(2) + bytes([255]) + bytes(6) + bytes([len(payload)]) + payload)


class HciReplayTests(unittest.TestCase):
    def test_legacy_multiple_and_scan_response_stay_separate(self):
        reports = hci.parse_event(event([legacy(), legacy(b'name', kind=4, address_type=2, rssi=127)]))
        self.assertEqual(len(reports), 2)
        self.assertEqual(reports[0]['address'], '11:22:33:44:55:66')
        self.assertEqual(reports[0]['address_type'], 'random')
        self.assertEqual(reports[0]['rssi'], -45)
        self.assertEqual(reports[1]['address_type'], 'public_identity')
        self.assertTrue(reports[1]['scan_response'])
        self.assertIsNone(reports[1]['rssi'])
        self.assertEqual(reports[1]['data_hex'], b'name'.hex())

    def test_extended_fragments_do_not_become_completed_advertisements(self):
        for status, label in enumerate(('complete', 'more', 'truncated')):
            report = hci.parse_event(event([extended(status=status)], True))[0]
            self.assertEqual(report['controller_data_status'], label)
            self.assertFalse(report['reassembled'])
            self.assertEqual(report['primary_phy_code'], 3)
            self.assertEqual(report['advertising_sid'], 3)
            self.assertIsNone(report['tx_power'])
        report = hci.parse_event(event([extended(address_type=255, sid=255)], True))[0]
        self.assertIsNone(report['address'])
        self.assertEqual(report['address_type'], 'anonymous')
        self.assertIsNone(report['advertising_sid'])

    def test_report_dbm_boundaries_match_hci_spec_for_each_field(self):
        for field in ('tx_power', 'rssi'):
            for raw, expected in ((129, -127), (255, -1), (0, 0), (20, 20), (127, None)):
                with self.subTest(field=field, raw=raw):
                    report = hci.parse_event(event([extended(**{field: raw})], True))[0]
                    self.assertEqual(report[field], expected)
            # +21 and +126 are outside the HCI report table, as is -128.
            for raw in (21, 126, 128):
                with self.subTest(field=field, raw=raw), self.assertRaises(hci.InvalidEvent):
                    hci.parse_event(event([extended(**{field: raw})], True))
        for raw in (21, 126, 128):
            with self.subTest(legacy_rssi=raw), self.assertRaises(hci.InvalidEvent):
                hci.parse_event(event([legacy(rssi=raw)]))

    def test_extended_phy_codes_are_explicitly_unvalidated(self):
        # Even reserved/future codes survive replay without being called valid.
        for primary, secondary in ((3, 4), (0, 255), (255, 255)):
            report = hci.parse_event(event([extended(primary_phy=primary,
                                                    secondary_phy=secondary)], True))[0]
            self.assertEqual(report['primary_phy_code'], primary)
            self.assertEqual(report['secondary_phy_code'], secondary)
            self.assertEqual(report['phy_interpretation'], 'unvalidated_raw_codes')
        report = hci.parse_event(event([legacy()]))[0]
        self.assertEqual(report['phy_interpretation'], 'legacy_1m')

    def test_malformed_event_is_atomic_and_never_emits_partial_reports(self):
        good = event([legacy(), legacy()])
        for end in range(len(good)):
            with self.subTest(end=end), self.assertRaises(hci.InvalidEvent):
                hci.parse_event(good[:end])
        bad = event([legacy(), b'\x00'])
        out = io.StringIO()
        with self.assertRaises(hci.InvalidEvent):
            hci.replay(io.BytesIO(bad.hex().encode() + b'\n'), out, 'rx', 'session')
        self.assertEqual(out.getvalue(), '')
        with self.assertRaises(hci.InvalidEvent):
            hci.parse_event(good + b'\x00')

    def test_reserved_fields_and_oversized_legacy_data_reject(self):
        for packet in [event([legacy(bytes(32))]), event([legacy(kind=5)]),
                       event([legacy(address_type=255)]), event([legacy(rssi=128)]),
                       event([extended(status=3)], True), event([extended(sid=16)], True),
                       bytes([4, 0x3e, 2, 2, 0])]:
            with self.subTest(packet_length=len(packet)), self.assertRaises(hci.InvalidEvent):
                hci.parse_event(packet)

    def test_bounded_stream_and_explicit_replay_provenance(self):
        data = (event([legacy()]).hex() + '\n').encode() * 3
        out = io.StringIO()
        counts = hci.replay(io.BytesIO(data), out, 'usb:0a12:0001', 'replay-1')
        self.assertEqual(counts, {'events': 3, 'reports': 3, 'ignored_events': 0})
        records = [json.loads(line) for line in out.getvalue().splitlines()]
        self.assertEqual([r['event_index'] for r in records], [0, 1, 2])
        self.assertEqual(records[0]['session_id'], 'replay-1')
        self.assertFalse(records[0]['live_reception_validated'])
        self.assertNotIn('timestamp', records[0])
        for bad in [b'00' * 1000, b'not hex\n', b'0\n', b'\n', b'04 00\n']:
            with self.assertRaises(hci.InvalidEvent):
                hci.replay(io.BytesIO(bad), io.StringIO(), 'rx', 'session')

    def test_unrelated_events_are_counted_without_fake_observations(self):
        data = b'040e00\n043e0101\n'
        out = io.StringIO()
        counts = hci.replay(io.BytesIO(data), out, 'rx', 'session')
        self.assertEqual(counts['ignored_events'], 2)
        self.assertEqual(counts['reports'], 0)
        self.assertEqual(out.getvalue(), '')

    def test_cli_has_only_replay_and_sanitized_error_path(self):
        cmd = [sys.executable, str(PATH), '--receiver-id', 'rx', '--session-id', 'session']
        result = subprocess.run(cmd, input=event([legacy()]).hex() + '\n', text=True, capture_output=True)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout)['source'], 'hci_replay')
        result = subprocess.run(cmd, input='private-payload\n', text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('private-payload', result.stderr)
        result = subprocess.run(cmd + ['--usb', '0a12:0001'], text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
