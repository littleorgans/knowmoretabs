"""`library.json` read once, as Rust's `State::read` reads it: the decisions every view of the app follows, from one
consistent read of the file. Every name and page here is invented."""

import json
import os
import shutil
import tempfile
import unittest
from datetime import UTC, datetime
from pathlib import Path
from unittest.mock import patch

import synthetic_archive
from test_app import Served

from tagger import archive
from tagger.archive import STATE_FILE, Archive, LibraryState, Unreadable, signature

STATE = {
    "schema_version": 1,
    "forgotten": ["https://gone.example/"],
    "tags": {
        "https://a.example/": {"add": ["coffee", "Old name", "zeppelins"], "remove": ["Rust"], "note": "kept"},
        "https://b.example/": {"remove": ["old NAME"]},
    },
    "vocabulary": {
        "Coffee": {"created_at": "2026-10-01T00:00:00Z", "used_at": "2026-10-02T00:00:00Z", "definition": "beans"},
        "Rust": {"created_at": "2026-10-01T00:00:00Z", "used_at": "2026-10-02T00:00:00"},  # no offset: no use
        "Old name": {"created_at": "2026-10-01T00:00:00Z", "retired_at": "2026-10-03T00:00:00Z"},
        "Atlas": {"created_at": "2026-10-01T00:00:00Z", "used_at": "not a time"},
        "zebra": {"created_at": "2026-10-01T00:00:00Z", "implies": ["Atlas"]},
    },
    "opaque": {"from": "a newer build"},
}


class StateCase(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root)

    def write(self, state) -> None:
        synthetic_archive.write_state(self.root, state)


class ReaderTests(StateCase):
    def test_decisions_vocabulary_and_uses_as_the_library_holds_them(self):
        self.write(STATE)
        state = LibraryState.read(self.root)
        self.assertEqual({"coffee": "Coffee", "rust": "Rust", "atlas": "Atlas", "zebra": "zebra"}, state.active)
        self.assertEqual({"old name": "Old name"}, state.retired)
        self.assertEqual({"coffee": datetime(2026, 10, 2, tzinfo=UTC).timestamp()}, state.used, "only an offset time")
        self.assertEqual(frozenset({"https://gone.example/"}), state.forgotten)
        self.assertEqual(
            ["Coffee"], state.tags("https://a.example/"), "active names only, in the vocabulary's spelling"
        )
        self.assertEqual(["coffee", "Old name", "zeppelins"], state.adds["https://a.example/"], "retired names kept")
        self.assertEqual({"rust"}, state.dismissed("https://a.example/"))
        self.assertEqual({"old name"}, state.dismissed("https://b.example/"), "a retired name's dismissal is kept")
        self.assertEqual(([], set()), (state.tags("https://none.example/"), state.dismissed("https://none.example/")))
        self.assertEqual(signature(self.root / STATE_FILE), state.signature)

    def test_tags_follow_the_librarys_tag_order(self):
        names = ["beta", "Alpha", "Gamma", "delta"]
        vocabulary = {n: {"created_at": "2026-10-01T00:00:00Z"} for n in names}
        self.write({"schema_version": 1, "forgotten": [], "tags": {"u": {"add": names}}, "vocabulary": vocabulary})
        self.assertEqual(["Alpha", "beta", "delta", "Gamma"], LibraryState.read(self.root).tags("u"), "case aside")

    def test_a_missing_file_is_an_empty_library_and_optional_fields_default(self):
        state = LibraryState.read(self.root)
        self.assertEqual(LibraryState(), state)
        self.assertIsNone(state.signature)
        self.write({"schema_version": 1, "forgotten": []})
        state = LibraryState.read(self.root)
        self.assertEqual(({}, {}, {}, frozenset()), (state.adds, state.active, state.used, state.forgotten))
        self.assertIsNotNone(state.signature)

    def test_a_damaged_or_unsupported_file_is_refused_never_read_as_empty(self):
        term = {"created_at": "2026-10-01T00:00:00Z"}
        for name, raw in (
            ("not json", b"{"),
            ("not an object", b"[]"),
            ("no schema", json.dumps({"forgotten": []}).encode()),
            ("another schema", json.dumps({"schema_version": 2, "forgotten": []}).encode()),
            ("schema as text", json.dumps({"schema_version": "1", "forgotten": []}).encode()),
            ("no forgotten list", json.dumps({"schema_version": 1}).encode()),
            ("forgotten not names", json.dumps({"schema_version": 1, "forgotten": [1]}).encode()),
            ("tags not a map", json.dumps({"schema_version": 1, "forgotten": [], "tags": []}).encode()),
            (
                "add not a list",
                json.dumps({"schema_version": 1, "forgotten": [], "tags": {"u": {"add": "x"}}}).encode(),
            ),
            (
                "remove not names",
                json.dumps({"schema_version": 1, "forgotten": [], "tags": {"u": {"remove": [2]}}}).encode(),
            ),
            ("term not a map", json.dumps({"schema_version": 1, "forgotten": [], "vocabulary": {"A": 1}}).encode()),
            ("term undated", json.dumps({"schema_version": 1, "forgotten": [], "vocabulary": {"A": {}}}).encode()),
            (
                "retired not a time",
                json.dumps(
                    {"schema_version": 1, "forgotten": [], "vocabulary": {"A": {**term, "retired_at": 5}}}
                ).encode(),
            ),
        ):
            with self.subTest(name):
                (self.root / STATE_FILE).write_bytes(raw)
                with self.assertRaises(Unreadable):
                    LibraryState.read(self.root)

    def test_an_unreadable_file_is_refused(self):
        (self.root / STATE_FILE).mkdir()
        with self.assertRaises(Unreadable):
            LibraryState.read(self.root)

    def test_the_archive_reads_its_decisions_through_the_same_reader(self):
        synthetic_archive.build(self.root)
        loaded = Archive.load(self.root)
        self.assertEqual(LibraryState.read(self.root), loaded.state)
        self.assertTrue(any(loaded.state.tags(url) for url in loaded.known))


class ConsistentReadTests(StateCase):
    """The bytes parsed and the signature kept come from one file, even while Rust replaces it."""

    def test_a_replacement_during_the_read_is_read_again_and_stamped_with_the_bytes_parsed(self):
        self.write(STATE)
        newer = {**STATE, "forgotten": ["https://newer.example/"]}
        calls = []

        def replaced_once(path):
            if not calls:
                self.write(newer)  # Rust's rename lands after the open: the read had the old file
            calls.append(path)
            return signature(path)

        with patch.object(archive, "signature", side_effect=replaced_once):
            state = LibraryState.read(self.root)
        self.assertEqual(2, len(calls), "read again once")
        self.assertEqual(frozenset({"https://newer.example/"}), state.forgotten)
        self.assertEqual(signature(self.root / STATE_FILE), state.signature, "the signature of the bytes parsed")

    def test_a_file_that_never_settles_is_refused(self):
        self.write(STATE)
        with (
            patch.object(archive, "signature", return_value=("never", "the same")),
            self.assertRaises(Unreadable),
        ):
            LibraryState.read(self.root)

    def test_a_refusal_recovers_once_the_file_is_whole_again(self):
        (self.root / STATE_FILE).write_bytes(b'{"schema_version": 1, "forgo')
        with self.assertRaises(Unreadable):
            LibraryState.read(self.root)
        self.write(STATE)
        self.assertEqual(frozenset({"https://gone.example/"}), LibraryState.read(self.root).forgotten)

    def test_a_replacement_or_an_edit_changes_the_signature(self):
        path = self.root / STATE_FILE
        self.assertIsNone(signature(path))
        self.write(STATE)
        first = signature(path)
        self.write(STATE)
        os.utime(path, ns=(first[3], first[3]))  # the same bytes and mtime, another file
        replaced = signature(path)
        self.assertEqual(first[2:], replaced[2:])
        self.assertNotEqual(first, replaced, "a new inode")
        os.utime(path, ns=(replaced[3] + 1, replaced[3] + 1))
        self.assertNotEqual(replaced, signature(path), "the same inode, a newer mtime")


class FollowerTests(Served):
    """The app reads `library.json` again exactly when its signature moved, and only then."""

    def tags_of(self, row: int) -> list[str]:
        """The page's tags as the service shows them after its next request."""
        self.assertEqual(200, self.call("GET", "/api/library")[0])
        return self.app.page(row)["tags"]

    def swapped(self, row: int) -> tuple[int, bytes]:
        """The file's bytes with one of the page's tags swapped for another name of the same length."""
        raw = (self.root / STATE_FILE).read_bytes()
        state = json.loads(raw)
        state["tags"][self.lib.records[row]["key"]]["add"] = ["Coffee"]
        return len(raw), (json.dumps(state, indent=1) + "\n").encode()

    def tagged_row(self) -> int:
        return next(i for i, r in enumerate(self.lib.records) if r["tags"] == ["Trains"] and not r["forgotten"])

    def test_unchanged_requests_never_read_the_file_or_encode_again(self):
        self.call("GET", "/api/library")
        with (
            patch.object(LibraryState, "read", wraps=LibraryState.read) as read,
            patch.object(self.app, "encode", side_effect=AssertionError("encoded")),
        ):
            for _ in range(3):
                self.assertEqual(200, self.call("GET", "/api/library")[0])
                self.assertEqual(200, self.call("GET", "/api/pins")[0])
        read.assert_not_called()

    def test_a_new_file_with_the_old_size_and_mtime_is_read(self):
        row = self.tagged_row()
        self.assertEqual(["Trains"], self.tags_of(row))
        before = signature(self.root / STATE_FILE)
        size, raw = self.swapped(row)
        self.assertEqual(size, len(raw))
        part = self.root / "library.json.part"
        part.write_bytes(raw)
        os.utime(part, ns=(before[3], before[3]))
        part.replace(self.root / STATE_FILE)
        self.assertEqual(before[2:], signature(self.root / STATE_FILE)[2:], "only the inode tells them apart")
        self.assertEqual(["Coffee"], self.tags_of(row))

    def test_an_edit_in_place_with_a_newer_mtime_is_read(self):
        row = self.tagged_row()
        self.assertEqual(["Trains"], self.tags_of(row))
        before = signature(self.root / STATE_FILE)
        _, raw = self.swapped(row)
        with (self.root / STATE_FILE).open("r+b") as f:
            f.write(raw)
        os.utime(self.root / STATE_FILE, ns=(before[3] + 1, before[3] + 1))
        self.assertEqual(before[:3], signature(self.root / STATE_FILE)[:3], "only the mtime tells them apart")
        self.assertEqual(["Coffee"], self.tags_of(row))

    def test_a_missing_file_is_an_empty_library_until_it_is_created(self):
        row = self.tagged_row()
        state = json.loads((self.root / STATE_FILE).read_text())
        (self.root / STATE_FILE).unlink()
        self.assertEqual([], self.tags_of(row))
        self.assertEqual([], self.call("GET", "/api/library")[1]["app_tags"])
        synthetic_archive.write_state(self.root, state)
        self.assertEqual(["Trains"], self.tags_of(row))
