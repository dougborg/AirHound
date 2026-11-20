"""Boundary tests for the separate GPL Sniffle helper; no upstream code imported."""
import importlib.util
import io
from pathlib import Path
import sys
import types
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('sniffle_source', Path(__file__).parents[1] / 'sniffle-source.py')
helper = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(helper)


class SniffleHelperTests(unittest.TestCase):
    def run_helper(self, version=(1, 11, 0, 0), errors=False):
        state = {'writes': 0, 'closed': False, 'signals': {}}

        class Packet:
            pass

        class Hardware:
            def __init__(self, port, **kwargs):
                state['port'], state['serial_options'] = port, kwargs
                self.ser = types.SimpleNamespace(close=lambda: state.update(closed=True))

            def cmd_marker(self, marker):
                pass

            def probe_fw_version(self):
                if version is None:
                    return None
                return types.SimpleNamespace(**dict(zip(('major', 'minor', 'revision', 'api_level'), version)))

            def setup_sniffer(self, **kwargs):
                state['setup'] = kwargs

            def recv_and_decode(self):
                if errors:
                    raise ValueError('synthetic malformed packet')
                if state['writes']:
                    state['signals'][helper.signal.SIGTERM](None, None)
                    return None
                return Packet()

        class Writer:
            def __init__(self, output):
                state['header_written'] = True

            def write_packet_message(self, packet):
                state['writes'] += 1

        modules = {}
        for name, values in {
            'sniffle': {}, 'sniffle.sniffle_hw': {'SniffleHW': Hardware},
            'sniffle.constants': {'SnifferMode': types.SimpleNamespace(PASSIVE_SCAN='passive')},
            'sniffle.packet_decoder': {'PacketMessage': Packet},
            'sniffle.pcap': {'PcapBleWriter': Writer},
        }.items():
            module = types.ModuleType(name)
            module.__dict__.update(values)
            modules[name] = module
        stdout = types.SimpleNamespace(buffer=io.BytesIO())
        with patch.dict(sys.modules, modules), patch.object(sys, 'argv', ['helper', '--port', 'fixture', '--channel', '39', '--coded']), patch.object(sys, 'stdout', stdout), patch.object(sys, 'stderr', io.StringIO()), patch.object(helper.time, 'sleep'), patch.object(helper.signal, 'signal', side_effect=lambda signal, callback: state['signals'].update({signal: callback})):
            try:
                helper.main()
            except Exception as exc:
                state['error'] = exc
        return state

    def test_passive_mode_explicit_port_and_cleanup(self):
        state = self.run_helper()
        self.assertEqual(state['setup']['mode'], 'passive')
        self.assertTrue(state['setup']['coded_phy'])
        self.assertTrue(state['setup']['ext_adv'])
        self.assertTrue(state['setup']['validate_crc'])
        self.assertEqual(state['port'], 'fixture')
        self.assertEqual(state['serial_options']['baudrate'], 921600)
        self.assertEqual(state['writes'], 1)
        self.assertTrue(state['closed'])
        self.assertNotIn('error', state)

    def test_unverified_firmware_never_emits_capture_header(self):
        for version in (None, (1, 10, 0, 0), (1, 11, 0, 1)):
            state = self.run_helper(version)
            self.assertIsInstance(state['error'], RuntimeError)
            self.assertNotIn('header_written', state)
            self.assertNotIn('setup', state)
            self.assertTrue(state['closed'])

    def test_decode_errors_are_bounded_and_close_serial(self):
        state = self.run_helper(errors=True)
        self.assertIsInstance(state['error'], RuntimeError)
        self.assertEqual(state['writes'], 0)
        self.assertTrue(state['closed'])
