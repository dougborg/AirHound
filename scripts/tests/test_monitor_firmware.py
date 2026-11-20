import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('monitor', Path(__file__).parents[1] / 'monitor-firmware.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class MonitorTests(unittest.TestCase):
    def test_radio_diagnostics_distinguish_reception_from_matches(self):
        monitor = module.Monitor('m5stickc_plus2', '0.1.1')
        self.sample(monitor)
        self.sample(monitor, uptime=60)
        monitor.line(b'INFO - WiFi stats: rx=123 parsed=100 queue_drops=2 hop_errors=0 matches=0')
        summary = monitor.summary(65)
        self.assertTrue(summary['passed'])
        self.assertEqual(summary['wifi_stats'][0],
                         dict(rx=123, parsed=100, queue_drops=2, hop_errors=0, matches=0))
        monitor.line(b'INFO - WiFi stats: rx=123 parsed=100 queue_drops=2 hop_errors=1 matches=0')
        self.assertIn('WiFi channel hopping failed', monitor.summary(65)['failures'])

    def test_observed_addresses_preserve_limit_and_separate_hits(self):
        monitor = module.Monitor('m5stickc_plus2', '0.1.1')
        monitor.line(b'INFO - Observed addresses: aps=12 wifi_other=3 ble=20 wifi_hits=0 ble_hits=2 limited=true')
        sample = monitor.summary(65)['observed_addresses'][0]
        self.assertEqual(sample, dict(aps=12, wifi_other=3, ble=20,
                                      wifi_hits=0, ble_hits=2, limited=True))

    def test_psram_requires_self_test_and_separate_memory_samples(self):
        monitor = module.Monitor('m5stickc_plus2', '0.1.1', require_psram=True)
        self.sample(monitor)
        self.sample(monitor, uptime=60)
        self.assertFalse(monitor.summary(65)['passed'])
        monitor.line(b'INFO - PSRAM ready: bytes=2097152 tested=true observation_capacity=4096')
        for _ in range(2):
            monitor.line(b'INFO - Memory stats: internal_free=7000 external_free=2000000')
        self.assertTrue(monitor.summary(65)['passed'])
        monitor.line(b'INFO - Memory stats: internal_free=7000 external_free=0')
        self.assertFalse(monitor.summary(65)['passed'])

    def sample(self, monitor, **changes):
        status = dict(type='status', board='m5stickc_plus2', version='0.1.1',
                      uptime=30, heap_free=10000, scanning=True)
        status.update(changes)
        monitor.line(('\x1b[32mINFO - ' + json.dumps(status) + '\x1b[0m').encode())

    def test_status_samples_exclude_wireless_identifiers(self):
        monitor = module.Monitor('m5stickc_plus2', '0.1.1')
        monitor.line(b'INFO - AirHound v0.1.1 starting on m5stickc_plus2')
        self.sample(monitor)
        self.sample(monitor, uptime=60, heap_free=9990)
        monitor.line(b'{"type":"wifi","ssid":"secret","mac":"02:01:02:03:04:05"}')
        summary = monitor.summary(65)
        self.assertTrue(summary['passed'])
        self.assertEqual(summary['heap_free_min'], 9990)
        self.assertNotIn('secret', json.dumps(summary))
        self.assertNotIn('02:01', json.dumps(summary))

    def test_old_firmware_and_invalid_metrics_do_not_pass(self):
        for changes in [dict(version='0.1.0'), dict(board='xiao'), dict(heap_free=-1),
                        dict(uptime=True), dict(scanning=False)]:
            monitor = module.Monitor('m5stickc_plus2', '0.1.1')
            self.sample(monitor, **changes)
            self.sample(monitor, **changes)
            self.assertFalse(monitor.summary(65)['passed'])

    def test_panics_restarts_and_uptime_regression_fail(self):
        for failure in [b'Guru Meditation Error', b'panicked at radio',
                        b'INFO - AirHound v0.1.1 starting on m5stickc_plus2']:
            monitor = module.Monitor('m5stickc_plus2', '0.1.1')
            monitor.line(b'INFO - AirHound v0.1.1 starting on m5stickc_plus2')
            self.sample(monitor)
            self.sample(monitor, uptime=60)
            monitor.line(failure)
            self.assertFalse(monitor.summary(65)['passed'])
        self.sample(monitor, uptime=1)
        self.assertIn('device uptime decreased', monitor.summary(65)['failures'])

    def test_noise_and_wireless_payloads_are_not_panic_reports(self):
        monitor = module.Monitor('m5stickc_plus2', '0.1.1')
        for line in [b'{invalid', b'[1,2]', b'INFO - startup',
                     b'{"type":"wifi","ssid":"panicked at"}']:
            monitor.line(line)
        self.sample(monitor)
        self.assertFalse(monitor.summary(35)['passed'])
        self.sample(monitor, uptime=60)
        self.assertTrue(monitor.summary(65)['passed'])

    def test_stream_summary_separates_ordinary_events_without_identifiers(self):
        monitor = module.Monitor('m5stickc_plus2', '0.1.1')
        monitor.line(b'{"type":"wifi","ssid":"private","match":[]}')
        monitor.line(b'{"type":"ble","name":"private","match":[{"detail":"private"}]}')
        monitor.line(b'{"type":"stream_stats","scan_drops":2,"output_drops":3,"ble_output_drops":4,"serialize_errors":0,"extra":"private"}')
        summary = monitor.summary(1)
        self.assertEqual(summary['observation_counts'], {'wifi_ordinary': 1, 'ble_matched': 1})
        self.assertEqual(summary['stream_stats'][0]['output_drops'], 3)
        self.assertNotIn('private', json.dumps(summary))
        monitor.line(b'{"type":"stream_stats","scan_drops":true}')
        self.assertIn('invalid stream loss metrics', monitor.summary(1)['failures'])
