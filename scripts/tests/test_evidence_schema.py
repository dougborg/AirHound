"""Keep standalone offline schemas aligned on the shared evidence contract."""
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class EvidenceSchemaTests(unittest.TestCase):
    def test_device_and_desktop_evidence_contracts_are_identical(self):
        device = json.loads((ROOT / 'schemas/device-message.v1.schema.json').read_text())
        desktop = json.loads((ROOT / 'schemas/desktop-ble.v1.schema.json').read_text())
        self.assertEqual(
            device['$defs']['detection_evidence'],
            desktop['properties']['detection'],
        )
