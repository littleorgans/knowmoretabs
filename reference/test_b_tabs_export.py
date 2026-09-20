import hashlib
import json
import os
from pathlib import Path
import runpy
import struct
import subprocess
import tempfile
import unittest
from datetime import datetime, timezone


SCRIPT = Path(__file__).resolve().parents[1] / 'b_tabs_export'
APP = runpy.run_path(str(SCRIPT))


def session_bytes(url='https://example.com/path?q=1&b=2'):
    def command(identifier, payload=b''):
        return struct.pack('<HB', len(payload) + 1, identifier) + payload

    def string(value, encoding):
        encoded = value.encode(encoding)
        length = len(encoded) // 2 if encoding == 'utf-16-le' else len(encoded)
        return struct.pack('<i', length) + encoded + b'\0' * (-len(encoded) % 4)

    navigation = struct.pack('<ii', 2, 0)
    navigation += string(url, 'utf-8')
    navigation += string('A title with <markup> & text', 'utf-16-le')
    return b'SNSS\x03\0\0\0' + b''.join([
        command(0, struct.pack('<ii', 1, 2)),
        command(2, struct.pack('<ii', 2, 0)),
        command(9, struct.pack('<ii', 1, 0)),
        command(6, struct.pack('<I', len(navigation)) + navigation),
        command(7, struct.pack('<ii', 2, 0)),
        command(255),
    ])


class ExportTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.home = Path(self.temporary.name)
        self.chrome = self.home / 'Library/Application Support/Google/Chrome'
        self.sessions = self.chrome / 'Default/Sessions'
        self.sessions.mkdir(parents=True)
        (self.chrome / 'Local State').write_text(json.dumps({'profile': {'last_used': 'Default'}}))
        self.source = self.sessions / 'Session_20'
        self.source.write_bytes(session_bytes())
        self.root = self.home / 'Documents/chrome-tabs'
        self.environment = dict(os.environ, HOME=str(self.home))

    def run_cli(self, *arguments):
        return subprocess.run([str(SCRIPT), *arguments], env=self.environment,
                              text=True, capture_output=True)

    def test_no_arguments_exports_newest_session_and_preserves_source(self):
        (self.sessions / 'Session_9').write_bytes(b'older unreadable session')
        (self.sessions / 'Session_junk').write_bytes(b'ignore this')
        before = self.source.read_bytes()
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        archives = list(self.root.glob('*/tabs.json'))
        self.assertEqual(len(archives), 1)
        archive = json.loads(archives[0].read_text())
        self.assertEqual(archive['metadata']['source'], str(self.source))
        self.assertEqual(archive['metadata']['profile'], 'Default')
        self.assertEqual(archive['metadata']['sha256'], hashlib.sha256(before).hexdigest())
        self.assertEqual(self.source.read_bytes(), before)
        self.assertEqual((archives[0].parent / archive['metadata']['snapshot']).read_bytes(), before)
        self.assertEqual(len(archive['tabs']), 1)
        self.assertIn(archives[0].parent.name + '/tabs.html', (self.root / 'index.html').read_text())

    def test_same_second_exports_never_overwrite(self):
        self.root.mkdir(parents=True)
        now = datetime(2026, 9, 15, 12, 30, 8, tzinfo=timezone.utc)
        first, _, _ = APP['publish_snapshot'](self.root, self.source, 'Default', now)
        original = (first / 'tabs.html').read_bytes()
        second, _, _ = APP['publish_snapshot'](self.root, self.source, 'Default', now)
        self.assertEqual(first.name, '2026-09-15-123008')
        self.assertEqual(second.name, '2026-09-15-123008-2')
        self.assertEqual((first / 'tabs.html').read_bytes(), original)

    def test_incomplete_session_leaves_existing_archive_and_index_intact(self):
        self.assertEqual(self.run_cli().returncode, 0)
        index = (self.root / 'index.html').read_bytes()
        archives = list(self.root.glob('*/tabs.json'))
        self.source.write_bytes(session_bytes() + b'\x08')
        result = self.run_cli()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Incomplete command', result.stderr)
        self.assertEqual((self.root / 'index.html').read_bytes(), index)
        self.assertEqual(list(self.root.glob('*/tabs.json')), archives)
        self.assertEqual(list(self.root.glob('.pending-*')), [])

    def test_no_session_does_not_create_empty_archive(self):
        self.source.unlink()
        result = self.run_cli()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('No saved Chrome sessions', result.stderr)
        self.assertFalse(self.root.exists())

    def test_explicit_session_does_not_require_chrome_state(self):
        (self.chrome / 'Local State').unlink()
        destination = self.home / 'custom archives'
        result = self.run_cli('--session', str(self.source), '--output-root', str(destination))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(list(destination.glob('*/tabs.html'))), 1)

    def test_profile_selection(self):
        other = self.chrome / 'Profile 2/Sessions'
        other.mkdir(parents=True)
        (other / 'Session_30').write_bytes(session_bytes())
        result = self.run_cli('--profile', 'Profile 2')
        self.assertEqual(result.returncode, 0, result.stderr)
        archive = json.loads(next(self.root.glob('*/tabs.json')).read_text())
        self.assertEqual(archive['metadata']['profile'], 'Profile 2')

    def test_index_rebuild_includes_nested_legacy_archive(self):
        self.root.mkdir(parents=True)
        legacy = self.root / '2026-09-05-123008/earlier-session'
        legacy.mkdir(parents=True)
        rows, metadata = APP['read_session'](self.source)
        metadata['saved_at'] = '2026-09-05T11:36:40'
        APP['write_export'](rows, metadata, legacy)
        result = self.run_cli('--index-only')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(list(self.root.rglob('tabs.json'))), 1)
        self.assertIn('2026-09-05-123008/earlier-session/tabs.html', (self.root / 'index.html').read_text())

    def test_concurrent_runs_publish_both_archives_in_index(self):
        processes = [subprocess.Popen([str(SCRIPT), '--force'], env=self.environment, text=True,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE) for _ in range(2)]
        for process in processes:
            _, errors = process.communicate(timeout=20)
            self.assertEqual(process.returncode, 0, errors)
        archives = list(self.root.glob('*/tabs.html'))
        self.assertEqual(len(archives), 2)
        index = (self.root / 'index.html').read_text()
        for archive in archives:
            self.assertIn(archive.parent.name + '/tabs.html', index)

    def test_unchanged_session_is_skipped_unless_forced(self):
        self.assertEqual(self.run_cli().returncode, 0)
        (self.sessions / 'Session_21').write_bytes(session_bytes())
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('No change since', result.stdout)
        self.assertEqual(len(list(self.root.glob('*/tabs.json'))), 1)
        self.assertEqual(list(self.root.glob('.pending-*')), [])
        self.assertEqual(self.run_cli('--force').returncode, 0)
        self.assertEqual(len(list(self.root.glob('*/tabs.json'))), 2)

    def test_changed_session_is_saved(self):
        self.assertEqual(self.run_cli().returncode, 0)
        (self.sessions / 'Session_21').write_bytes(session_bytes('https://example.com/other'))
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(list(self.root.glob('*/tabs.json'))), 2)

    def test_library_lists_each_url_once_with_its_history(self):
        self.root.mkdir(parents=True)
        other = self.sessions / 'Session_21'
        other.write_bytes(session_bytes('https://example.com/other'))
        for day, source in ((1, self.source), (2, other), (3, self.source)):
            APP['publish_snapshot'](self.root, source, 'Default', datetime(2026, 9, day, tzinfo=timezone.utc))
        archives = APP['load_archives'](self.root)
        self.assertEqual([a['path'] for a in archives],
                         ['2026-09-03-000000', '2026-09-02-000000', '2026-09-01-000000'])
        pages = {page['url']: page for page in APP['library_records'](archives, {})}
        self.assertEqual(len(pages), 2)
        self.assertEqual(pages['https://example.com/path?q=1&b=2']['seen'], [[0, [[1, 2]]], [2, [[1, 2]]]])
        self.assertEqual(pages['https://example.com/other']['seen'], [[1, [[1, 2]]]])
        page = APP['render_index'](archives, {})
        self.assertIn('"pages": [{"url": "https://example.com/path?q=1&b=2"', page)
        self.assertNotIn('/* ', page.split('<script id="data"')[0])

    def test_forget_hides_a_page_until_restored_and_leaves_snapshots_alone(self):
        self.assertEqual(self.run_cli().returncode, 0)
        url = 'https://example.com/path?q=1&b=2'
        snapshot = next(self.root.glob('*/tabs.json'))
        before = snapshot.read_bytes()
        marker = '"pages": [{"url": "' + url
        self.assertIn(marker, (self.root / 'index.html').read_text())
        result = self.run_cli('--forget', url)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn(marker, (self.root / 'index.html').read_text())
        self.assertEqual(list(json.loads((self.root / 'forgotten.json').read_text())), [url])
        self.assertEqual(self.run_cli().returncode, 0)
        self.assertNotIn(marker, (self.root / 'index.html').read_text())
        self.assertEqual(self.run_cli('--restore', url).returncode, 0)
        self.assertIn(marker, (self.root / 'index.html').read_text())
        self.assertEqual(snapshot.read_bytes(), before)
        self.assertEqual(len(list(self.root.glob('*/tabs.json'))), 1)

    def test_forget_rejects_a_url_that_was_never_saved(self):
        self.assertEqual(self.run_cli().returncode, 0)
        result = self.run_cli('--forget', 'https://example.com/typo')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Not in your library', result.stderr)
        self.assertFalse((self.root / 'forgotten.json').exists())

    def test_library_leaves_out_local_pages(self):
        for url in ('file:///Users/me/page.html', 'http://127.0.0.1:8798/?run=1', 'http://localhost:5173/',
                    'https://localhost/'):
            self.assertTrue(APP['is_local'](url), url)
        for url in ('https://example.com/file://x', 'https://localhost.example.com/', 'chrome://history/'):
            self.assertFalse(APP['is_local'](url), url)
        self.root.mkdir(parents=True)
        self.source.write_bytes(session_bytes('http://localhost:5173/'))
        APP['publish_snapshot'](self.root, self.source, 'Default', datetime(2026, 9, 1, tzinfo=timezone.utc))
        archives = APP['load_archives'](self.root)
        self.assertEqual(len(archives[0]['tabs']), 1)
        self.assertEqual(APP['library_records'](archives, {}), [])

    def test_open_flag_opens_index(self):
        binaries = self.home / 'bin'
        binaries.mkdir()
        opener = binaries / 'open'
        opener.write_text('#!/bin/sh\nprintf "%s" "$1" > "$HOME/opened-path"\n')
        opener.chmod(0o755)
        self.environment['PATH'] = str(binaries) + os.pathsep + os.environ['PATH']
        result = self.run_cli('--open')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.home / 'opened-path').read_text(), str((self.root / 'index.html').resolve()))


if __name__ == '__main__':
    unittest.main()
