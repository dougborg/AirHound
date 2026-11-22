import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("build_metadata", Path(__file__).parents[2] / "build-metadata.py")
metadata = importlib.util.module_from_spec(spec)
spec.loader.exec_module(metadata)


class BuildMetadataTests(unittest.TestCase):
    def test_source_archive_does_not_invent_revision_or_clean_state(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[package]\nversion = "0.2.0"\n')
            info = metadata.metadata(root)
            self.assertEqual(info["AirHoundPackageVersion"], "0.2.0")
            self.assertEqual(info["CFBundleVersion"], "0")
            self.assertNotIn("AirHoundSourceRevision", info)
            self.assertNotIn("AirHoundTrackedSourceModified", info)

    def test_exact_tag_and_tracked_modifications_are_separate_from_package_version(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[package]\nversion = "0.2.0"\n')
            def git(*args):
                subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True)
            git("init")
            git("add", "Cargo.toml")
            git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-m", "fixture")
            git("tag", "v0.2.0-preview.1")
            info = metadata.metadata(root)
            self.assertEqual(info["AirHoundSourceTag"], "v0.2.0-preview.1")
            self.assertEqual(info["CFBundleVersion"], "1")
            self.assertFalse(info["AirHoundTrackedSourceModified"])
            self.assertEqual(len(info["AirHoundSourceRevision"]), 40)
            (root / "Cargo.toml").write_text('[package]\nversion = "0.3.0"\n')
            changed = metadata.metadata(root)
            self.assertEqual(changed["AirHoundPackageVersion"], "0.3.0")
            self.assertTrue(changed["AirHoundTrackedSourceModified"])
            self.assertEqual(changed["AirHoundSourceRevision"], info["AirHoundSourceRevision"])
