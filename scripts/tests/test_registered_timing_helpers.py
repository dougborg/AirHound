"""Imported helper coverage; synthetic timing text is scanned, never executed."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('registered_helper_guard', ROOT / 'scripts/check-test-timing.py')
guard = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = guard
spec.loader.exec_module(guard)


class RegisteredTimingHelpersTests(unittest.TestCase):
    def test_each_imported_helper_is_scanned_through_repository_policy(self):
        paths = (
            'apps/macos/build-metadata.py',
            'scripts/compile-signatures.py',
            'companions/flipper/scripts/build-diagnostic.py',
        )
        policy = guard.read_policy(ROOT / 'docs/test-timing-policy.json')
        helpers = {path: policy['helpers'][path] for path in paths}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            for path in paths:
                self.assertFalse(guard.test_path(path), path)
                target = root / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text('import time\ntime.sleep(1)\n')
            subprocess.run(['git', 'add', '.'], cwd=root, check=True)
            report = guard.report(root, {'helpers': helpers, 'dispositions': {}})
            self.assertEqual(report['scan_errors'], [])
            self.assertEqual(report['new_unclassified'], len(paths))
            self.assertEqual({item['path'] for item in report['findings']}, set(paths))
            for omitted in paths:
                with self.subTest(omitted=omitted):
                    subset = {path: reason for path, reason in helpers.items() if path != omitted}
                    report = guard.report(root, {'helpers': subset, 'dispositions': {}})
                    self.assertEqual(report['scan_errors'], [])
                    self.assertEqual({item['path'] for item in report['findings']}, set(paths) - {omitted})


if __name__ == '__main__':
    unittest.main()
