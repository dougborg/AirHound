import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('release_assets', Path(__file__).parents[1] / 'check-release-assets.py')
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def tar(path, files):
    with tarfile.open(path, 'w:gz') as archive:
        for name, data in files.items():
            member = tarfile.TarInfo(name)
            member.size = len(data)
            archive.addfile(member, io.BytesIO(data))


def fixture(root):
    for name in MODULE.EXPECTED:
        prefix = name.removesuffix('.tar.gz')
        if name.endswith('.bin'):
            (root / name).write_bytes(b'firmware')
        elif name == 'airhound-source.tar.gz':
            files = ['LICENSE', 'signatures/builtin.v2.json', 'schemas/signatures.v2.schema.json',
                     'scripts/sniffle-source.py', 'scripts/licenses/GPL-3.0.txt',
                     'scripts/replay-hci-ble.py', 'docs/release-status.md']
            tar(root / name, {prefix + '/' + f: b'source' for f in files})
        elif name.endswith('-source.tar.gz'):
            files = ['Cargo.lock', 'Cargo.toml', '.cargo/config.toml', 'BUILD-SOURCE.txt',
                     'LICENSE', 'THIRD_PARTY_NOTICES', 'vendor/dependency/LICENSE']
            tar(root / name, {prefix + '/' + f: b'source' for f in files})
        elif name.startswith('airhound-desktop'):
            files = ['manifest.json', 'dependencies.json', 'LICENSE', 'README.txt', 'runtime-libraries.txt']
            files += ['AirHound.app/Contents/MacOS/airhound-hostd', 'AirHound.app/Contents/Info.plist'] if 'macos' in name else ['bin/airhound-hostd']
            tar(root / name, {prefix + '/' + f: b'content' for f in files})
    for target in ('macos-arm64', 'linux-x86_64'):
        prefix = 'mt7610u-lab-' + target
        source = prefix + '-source.tar.gz'
        manifest = {'source_archive': source, 'source_sha256': MODULE.digest(root / source),
                    'binary_sha256': hashlib.sha256(b'binary').hexdigest()}
        files = {prefix + '/' + f: b'notice' for f in ['LICENSE', 'THIRD_PARTY_NOTICES', 'LIBUSB-LICENSE']}
        files[prefix + '/manifest.json'] = json.dumps(manifest).encode()
        files[prefix + '/bin/mt7610u-lab'] = b'binary'
        tar(root / (prefix + '.tar.gz'), files)


class ReleaseAssetChecks(unittest.TestCase):
    def test_complete_set_has_independently_recomputed_checksums(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture(root)
            MODULE.check(root)
            lines = (root / 'SHA256SUMS').read_text().splitlines()
            self.assertEqual(len(lines), 11)
            for line in lines:
                digest, name = line.split()
                self.assertEqual(digest, hashlib.sha256((root / name).read_bytes()).hexdigest())

    def test_missing_asset_and_substituted_corresponding_source_fail(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture(root)
            (root / 'airhound-xiao.bin').unlink()
            with self.assertRaisesRegex(ValueError, 'asset set mismatch'):
                MODULE.check(root)
            fixture(root)
            source = root / 'mt7610u-lab-macos-arm64-source.tar.gz'
            source.write_bytes(source.read_bytes() + b'substitution')
            with self.assertRaisesRegex(ValueError, 'source mismatch'):
                MODULE.check(root)

    def test_traversal_is_rejected_before_publication(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture(root)
            tar(root / 'airhound-source.tar.gz', {'../outside': b'bad'})
            with self.assertRaisesRegex(ValueError, 'unsafe archive'):
                MODULE.check(root)


if __name__ == '__main__':
    unittest.main()
