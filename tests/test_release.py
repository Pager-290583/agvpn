"""Validate release assets without GitHub credentials or publishing anything."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('prepare_release', ROOT / 'scripts/prepare-release.py')
RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RELEASE)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='agvpn-release-test-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.site = self.root / 'site'
        self.site.mkdir()
        self.output = self.root / 'assets'
        (self.root / 'Cargo.toml').write_text('[package]\nversion = "1.1.1"\n')
        for name in RELEASE.ASSETS[:2]:
            template = (ROOT / 'flatpak/pages' / name).read_text()
            (self.site / name).write_text(template.replace('@URL@', 'https://pager-290583.github.io/agvpn'))

    def prepare(self, sha='a' * 40):
        return RELEASE.prepare_release(self.root, self.site, self.output, 'Pager-290583/agvpn', sha, 8, 123)

    def test_assets_identify_build_and_have_valid_checksums(self):
        metadata = self.prepare()
        self.assertEqual(metadata['tag'], 'v1.1.1-build.8')
        for line in (self.output / 'SHA256SUMS').read_text().splitlines():
            digest, name = line.split('  ', 1)
            self.assertEqual(hashlib.sha256((self.output / name).read_bytes()).hexdigest(), digest)
        info = json.loads((self.output / 'build-info.json').read_text())
        self.assertEqual(info['commit'], 'a' * 40)
        self.assertEqual(info['architectures'], ['x86_64', 'aarch64'])
        notes = (self.output / 'release-notes.txt').read_text()
        self.assertIn('/releases/download/v1.1.1-build.8/agvpn.flatpakref', notes)
        self.assertNotIn('@URL@', (self.output / 'agvpn.flatpakref').read_text())

    def test_rejects_wrong_repository_before_creating_assets(self):
        path = self.site / 'agvpn.flatpakref'
        path.write_text(path.read_text().replace('pager-290583.github.io', 'example.com'))
        with self.assertRaises(ValueError):
            self.prepare()
        self.assertFalse(self.output.exists())

    def test_rejects_unexpanded_template(self):
        (self.site / 'agvpn.flatpakrepo').write_text((ROOT / 'flatpak/pages/agvpn.flatpakrepo').read_text())
        with self.assertRaises(ValueError):
            self.prepare()

    def test_rejects_unknown_application(self):
        path = self.site / 'agvpn.flatpakref'
        path.write_text(path.read_text().replace(RELEASE.APP_ID, 'org.example.Other'))
        with self.assertRaises(ValueError):
            self.prepare()

    def test_rejects_short_source_commit(self):
        with self.assertRaises(ValueError):
            self.prepare(sha='abcdef0')


if __name__ == '__main__':
    unittest.main()
