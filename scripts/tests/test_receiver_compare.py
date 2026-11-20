"""Synthetic, identifier-free fixture comparison tests (no captures/hardware)."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('receiver_compare', Path(__file__).parents[1] / 'receiver-compare.py')
comparison = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(comparison)


def controls():
    return dict.fromkeys(comparison.CONTROLS, True) | {
        'schema_version': 1, 'channel': 36, 'bandwidth_mhz': 20,
        'mac_window_unix_s': [1000, 1010], 'linux_window_unix_s': [2000, 2010]}


def beacon(sequence, bssid, signal=-60, channel=36, timestamp=1001000):
    return {'schema_version': 1, 'type': 'beacon', 'sequence': sequence, 'session_id': 'synthetic',
            'bssid': bssid, 'estimated_rssi_dbm': signal, 'receive_channel': channel,
            'observed_at_unix_ms': timestamp, 'rssi_validated': False,
            'rssi_estimate_basis': 'mt76_eeprom_formula'}


class CompareTests(unittest.TestCase):
    def test_expanded_ht20_channels_and_wide_center_rejection(self):
        for channel in [12, 13, 52, 64, 100, 128, 132, 144, 149, 153, 165]:
            comparison.validate_controls(controls() | {'channel': channel})
        for channel in [14, 42, 50, 68, 106, 155, 163, 169]:
            with self.assertRaises(ValueError):
                comparison.validate_controls(controls() | {'channel': channel})

    def test_counts_relative_recall_and_signal_pairing(self):
        with tempfile.TemporaryDirectory() as directory:
            mac, linux = Path(directory) / 'mac', Path(directory) / 'linux'
            records = [beacon(0, '02:00:00:00:00:01', -60), beacon(1, '02:00:00:00:00:01', -64),
                       beacon(2, '02:00:00:00:00:02', None),
                       beacon(3, '02:00:00:00:00:03', timestamp=1010000)]
            mac.write_text(''.join(json.dumps(record) + '\n' for record in records))
            linux.write_text('2001\t02:00:00:00:00:01\t5180\t-65\n2002\t02:00:00:00:00:03\t5180\t-70\n')
            manifest = comparison.validate_controls(controls())
            report = comparison.compare(comparison.read_mac(mac, manifest), comparison.read_linux(linux, manifest), manifest)
            self.assertEqual(report['mac']['beacons'], 3)
            self.assertEqual(report['mac']['outside_window'], 1)
            self.assertEqual(report['matched_bssids'], 1)
            self.assertEqual(report['mac_bssid_recall_relative_to_linux'], .5)
            self.assertEqual(report['signal_delta']['mean_db'], 3)
            self.assertNotIn('02:00:', json.dumps(report))
            self.assertNotIn('synthetic', json.dumps(report))

    def test_controls_required_and_equal_windows(self):
        for change in ({'same_antenna': False}, {'linux_window_unix_s': [2000, 2020]},
                       {'bandwidth_mhz': 40}, {'channel': 169}, {'mac_window_unix_s': [float('nan'), 1010]}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                comparison.validate_controls(controls() | change)

    def test_exploratory_only_relaxes_controlled_ap_attestation(self):
        manifest = controls() | {'controlled_ap_set': False}
        with self.assertRaises(ValueError):
            comparison.validate_controls(manifest)
        comparison.validate_controls(manifest, exploratory=True)
        with self.assertRaises(ValueError):
            comparison.validate_controls(manifest | {'same_antenna': False}, exploratory=True)
        report = comparison.compare(comparison.Capture(36, [1000, 1010]),
                                    comparison.Capture(36, [2000, 2010]), manifest, exploratory=True)
        self.assertFalse(report['validation_passed'])
        self.assertEqual(report['comparison_kind'], 'exploratory')
        self.assertTrue(report['limitations'])

    def test_nonempty_exploratory_measurements_are_never_conclusive(self):
        manifest = controls() | {'controlled_ap_set': False}
        mac = comparison.Capture(36, [1000, 1010])
        linux = comparison.Capture(36, [2000, 2010])
        address = bytes([2, 0, 0, 0, 0, 1])
        mac.add(1001, address, 36, -61)
        linux.add(2001, address, 36, -60)
        report = comparison.compare(mac, linux, manifest, exploratory=True)
        self.assertEqual(report['matched_bssids'], 1)
        self.assertEqual(report['mac_bssid_recall_relative_to_linux'], 1)
        self.assertEqual(report['signal_delta']['mean_db'], -1)
        self.assertFalse(report['conclusive'])
        self.assertFalse(report['validation_passed'])
        self.assertIn('Exploratory', report['interpretation'])
        self.assertIn('uncontrolled', report['interpretation'])
        self.assertNotIn('controlled-window', report['interpretation'])

    def test_different_channel_and_missing_radiotap_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'linux'
            for line in ('2001\t02:00:00:00:00:01\t5200\t-65\n',
                         '2001\t02:00:00:00:00:01\t\t-65\n',
                         '2001\t02:00:00:00:00:01\t5180\t-65',
                         '2001\t03:00:00:00:00:01\t5180\t-65\n'):
                path.write_text(line)
                with self.assertRaises(ValueError):
                    comparison.read_linux(path, controls())

    def test_empty_reference_is_inconclusive_not_perfect_recall(self):
        manifest = controls()
        report = comparison.compare(comparison.Capture(36, [1000, 1010]),
                                    comparison.Capture(36, [2000, 2010]), manifest)
        self.assertFalse(report['conclusive'])
        self.assertIsNone(report['mac_bssid_recall_relative_to_linux'])
        self.assertIsNone(report['signal_delta']['mean_db'])

    def test_missing_signal_is_not_zero(self):
        capture = comparison.Capture(36, [0, 10])
        capture.add(1, b'\x02\0\0\0\0\1', 36, None)
        self.assertEqual(capture.missing_signal, 1)
        self.assertEqual(capture.by_bssid[b'\x02\0\0\0\0\1'][2], 0)

    def test_bounds_and_sequence_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'mac'
            path.write_text(json.dumps(beacon(1, '02:00:00:00:00:01')) + '\n')
            with self.assertRaises(ValueError):
                comparison.read_mac(path, controls())
            path.write_text('x' * comparison.MAX_LINE + '\n')
            with self.assertRaises(ValueError):
                list(comparison.lines(path))


if __name__ == '__main__':
    unittest.main()
