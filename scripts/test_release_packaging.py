"""Offline contracts for app bundles, existing updates and remote deployment."""
from datetime import datetime, timezone
import hashlib
from pathlib import Path
import plistlib
from subprocess import CompletedProcess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from ds_store import DSStore
from mac_alias import Alias, TargetInfo, VolumeInfo

from collect_release import collect
from dmg_layout import POSITIONS, background_alias, validate_layout, write_layout
import package_release
from release_metadata import TARGETS, WORKER, apple_versions, archive_name, dmg_name, expected_assets


class PackagingTests(unittest.TestCase):
    def test_cargo_versions_map_to_distinct_numeric_bundle_versions(self):
        versions = ['0.1.0-alpha.0', '0.1.0-alpha.99999', '0.1.0-beta.0',
                    '0.1.0-beta.99999', '0.1.0', '0.1.1-alpha.0']
        builds = [tuple(map(int, apple_versions(v)[1].split('.'))) for v in versions]
        self.assertEqual(builds, sorted(set(builds)))
        self.assertEqual(apple_versions('0.1.0-beta.1'), ('0.1.0', '0.1.100001'))
        for invalid in ['01.1.0', '0.1.0-rc.1', '0.1.0+meta', '0.1.0-beta.100000', '0.1.2148']:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                apple_versions(invalid)

    def test_dmg_names_never_match_the_legacy_updater_target_selection(self):
        for target in TARGETS:
            self.assertIn(target, archive_name('0.1.0', target))
            if target.endswith('apple-darwin'):
                self.assertNotIn(target, dmg_name('0.1.0', target))
        self.assertEqual(len(expected_assets('0.1.0')), 8)

    def test_bundle_metadata_resources_and_manual_update_marker(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, icon = root / 'silicolab', root / 'AppIcon.icns'
            binary.write_bytes(b'binary')
            icon.write_bytes(b'icon')
            for manual, name in [(False, 'silicolab.app'), (True, 'SilicoLab.app')]:
                app = root / ('dmg' if manual else 'archive') / name
                package_release.create_app(binary, app, '0.1.0-beta.1', icon, manual)
                contents = app / 'Contents'
                info = plistlib.loads((contents / 'Info.plist').read_bytes())
                self.assertEqual(info['CFBundleIdentifier'], 'com.silicolab.SilicoLab')
                self.assertEqual(info['SilicoLabCargoVersion'], '0.1.0-beta.1')
                self.assertEqual(info['LSMinimumSystemVersion'], '13.0')
                self.assertNotIn('CFBundleDocumentTypes', info)
                self.assertEqual((contents / 'MacOS/silicolab').stat().st_mode & 0o7777, 0o755)
                resources = contents / 'Resources'
                self.assertEqual((resources / 'manual-dmg-updates').exists(), manual)
                self.assertTrue((resources / 'assets/forcefields/charmm36.ff/LICENSE').is_file())
                self.assertTrue((resources / 'assets/fonts/LiberationSans-LICENSE').is_file())

    def test_legacy_macos_archive_retains_exact_executable_path_and_no_manual_marker(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / 'target/aarch64-apple-darwin/release/silicolab'
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b'binary')

            smoked = []

            def command(*args, **kwargs):
                if '--help' in args:
                    smoked.append(Path(args[0]))
                if 'scripts/gen-icons.py' in args:
                    artwork = Path(args[-1])
                    artwork.mkdir(parents=True)
                    (artwork / 'AppIcon.icns').write_bytes(b'icon')
                return CompletedProcess(args, 0, stdout='SilicoLab help', stderr='')

            with patch.object(package_release, 'cargo_version', return_value='0.1.0'), \
                    patch.object(package_release, 'run', side_effect=command), \
                    patch.object(package_release, 'validate_app'), \
                    patch.object(package_release, 'create_dmg', side_effect=RuntimeError('stop after ZIP')), \
                    patch.dict('os.environ', {'CARGO_TARGET_DIR': str(root / 'target')}), \
                    patch.object(package_release.sys, 'platform', 'darwin'):
                with self.assertRaisesRegex(RuntimeError, 'stop after ZIP'):
                    package_release.package('aarch64-apple-darwin', root / 'out', skip_build=True)
            self.assertEqual(len(smoked), 1)
            self.assertNotEqual(smoked[0], binary)
            self.assertTrue(str(smoked[0]).endswith('silicolab.app/Contents/MacOS/silicolab'))
            with zipfile.ZipFile(root / 'out' / archive_name('0.1.0', 'aarch64-apple-darwin')) as archive:
                path = 'silicolab.app/Contents/MacOS/silicolab'
                self.assertIn(path, archive.namelist())
                self.assertEqual(archive.getinfo(path).external_attr >> 16 & 0o777, 0o755)
                self.assertFalse(any('manual-dmg-updates' in name for name in archive.namelist()))

    def test_actual_binary_inspection_rejects_wrong_architecture_and_local_dylibs(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / 'silicolab'
            binary.write_bytes(b'fixture')
            binary.chmod(0o755)
            dependencies = 'silicolab:\n\t/usr/lib/libSystem.B.dylib (version 1)\n'
            with patch('subprocess.check_output', side_effect=['arm64', dependencies, 'minos 13.0\n']):
                self.assertEqual(package_release.inspect_binary(binary, 'arm64')['minimum_macos'], '13.0')
            for outputs in [['x86_64'], ['arm64', 'silicolab:\n\t/opt/homebrew/lib/a.dylib (version 1)'],
                            ['arm64', dependencies, 'minos 14.0\n']]:
                with patch('subprocess.check_output', side_effect=outputs), self.assertRaises(ValueError):
                    package_release.inspect_binary(binary, 'arm64')

    def test_layout_keeps_large_icons_left_to_right_and_background_on_its_volume(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / '.background.tiff').write_bytes(b'fixture')
            date = datetime(2026, 1, 1, tzinfo=timezone.utc)
            alias = Alias(volume=VolumeInfo('SilicoLab', date, b'H+', 0, 0, b'\0\0',
                                           posix_path='/Volumes/SilicoLab'),
                          target=TargetInfo(0, '.background.tiff', 2, 16, date, b'\0'*4, b'\0'*4,
                                            posix_path='/.background.tiff'))
            with patch('dmg_layout.background_alias', return_value=alias.to_bytes()):
                write_layout(directory)
            report = validate_layout(directory)
            self.assertEqual(report['icon_size'], 160)
            self.assertLess(POSITIONS['SilicoLab.app'][0], POSITIONS['Applications'][0])
            with DSStore.open(str(directory / '.DS_Store'), 'r+') as store:
                store['SilicoLab.app']['Iloc'] = POSITIONS['Applications']
            with self.assertRaisesRegex(ValueError, 'position'):
                validate_layout(directory)

    def test_background_alias_resolves_var_private_var_mount_aliases(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            actual = root / 'volume'
            actual.mkdir()
            (actual / '.background.tiff').write_bytes(b'fixture')
            link = root / 'link'
            link.symlink_to(actual, target_is_directory=True)
            with patch('dmg_layout.Alias.for_file') as alias:
                background_alias(link)
                alias.assert_called_once_with(str((actual / '.background.tiff').resolve()))
                self.assertEqual(alias.return_value.volume.posix_path, '/Volumes/SilicoLab')

    def test_collect_requires_all_eight_assets_and_the_exact_worker_checksum(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for target in TARGETS:
                path = 'silicolab.app/Contents/MacOS/silicolab' if 'apple-darwin' in target else (
                    'silicolab.exe' if 'windows' in target else 'silicolab')
                with zipfile.ZipFile(directory / archive_name('0.1.0', target), 'w') as archive:
                    archive.writestr(path, b'binary')
                if 'apple-darwin' in target:
                    (directory / dmg_name('0.1.0', target)).write_bytes(b'dmg')
            worker = b'production worker fixture'
            (directory / WORKER).write_bytes(worker)
            (directory / (WORKER + '.sha256')).write_text(hashlib.sha256(worker).hexdigest() + '  ' + WORKER)
            collect(directory, '0.1.0')
            self.assertEqual(len((directory / 'SHA256SUMS').read_text().splitlines()), 8)
            (directory / (WORKER + '.sha256')).write_text('wrong')
            with self.assertRaisesRegex(ValueError, 'checksum'):
                collect(directory, '0.1.0')
            (directory / WORKER).unlink()
            with self.assertRaisesRegex(ValueError, 'missing'):
                collect(directory, '0.1.0')

    def test_worker_packaging_keeps_raw_filename_version_and_checksum_contract(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / 'target/x86_64-unknown-linux-musl/release/silicolab-compute'
            binary.parent.mkdir(parents=True)
            header = bytearray(20)
            header[:6] = b'\x7fELF\x02\x01'
            header[18:20] = (62).to_bytes(2, 'little')
            binary.write_bytes(header)
            with patch.object(package_release, 'ROOT', root), \
                    patch.object(package_release, 'cargo_version', return_value='0.1.0'), \
                    patch.object(package_release, 'run', return_value=CompletedProcess([], 0, stdout='0.1.0\n')):
                package_release.package_worker(root / 'out')
            self.assertEqual((root / 'out' / WORKER).read_bytes(), header)
            self.assertEqual((root / 'out' / (WORKER + '.sha256')).read_text().split()[0],
                             hashlib.sha256(header).hexdigest())


if __name__ == '__main__':
    unittest.main()
