"""Send to library on the synthetic archive against a fake knowmoretabs: the preview counts what the mirror will
change and writes nothing; a send creates the app's names first, leaves every library page with exactly the app's
tags (none for a page the app never tagged), dismisses the app's "no" where the library is undecided, forgets the
app's forgotten pages and leaves the library's own forgotten, skips pages the library lacks, and a resend after a
partial failure converges. A failed call stops the send with the error on screen; a snapshot root is never
written. `RealSendTests` runs them against the real knowmoretabs named by KMT_BIN, each on its own scratch root,
except those that make the fake fail a call."""

import json
import errno
import os
import shlex
import subprocess
import sys
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

import numpy as np
from test_add import Served, fake_binary

from tagger.app import send
from tagger.app.store import Store
from tagger.archive import Archive


def fake_only(test):
    """A test that makes the fake knowmoretabs fail a call (`FAKE_KMT_FAIL`), which the real one cannot."""
    test.fake_only = True
    return test


def logged(folder: Path, binary: str) -> str:
    """`binary` behind a script that logs each argument list to `FAKE_KMT_LOG`, as the fake does."""
    path = folder / "knowmoretabs-logged"
    path.write_text(
        f"#!{sys.executable}\nimport json, os, sys\n"
        "if log := os.environ.get('FAKE_KMT_LOG'):\n"
        "    with open(log, 'a') as f:\n"
        "        f.write(json.dumps({'argv': sys.argv[1:]}) + '\\n')\n"
        f"os.execv({binary!r}, [{binary!r}, *sys.argv[1:]])\n"
    )
    path.chmod(0o700)
    return str(path)


class SendTests(Served):
    def setUp(self):
        super().setUp()
        self.lib, _ = self.load()
        self.app, self.port = self.serve(self.lib)
        self.app.binary = fake_binary(self.work)
        self.log = self.work / "argv.jsonl"
        env = patch.dict(os.environ, {"FAKE_KMT_LOG": str(self.log)})
        env.start()
        self.addCleanup(env.stop)
        # live pages holding no owner tag in the archive, so every tag the app puts on is a change
        self.rows = [int(r) for r in np.flatnonzero(self.app.live()) if not self.lib.Y[r].any()][:4]
        self.keys = [self.lib.records[r]["key"] for r in self.rows]
        self.start = Archive.load(self.root)  # 85 pages with 99 owner tags, one page forgotten

    def post(self, path: str, body: dict | None = None, ok: int = 200) -> dict:
        if path == "/api/send" and body is None:
            body = {"preview": getattr(self, "preview_id", None)}
        status, out = self.call(self.port, "POST", path, body or {})
        self.assertEqual(ok, status, out)
        if path == "/api/send/preview" and status == 200:
            self.preview_id = out.get("preview")
        return out

    def state(self) -> dict:
        return json.loads((self.root / "library.json").read_text())

    def lists(self, key: str) -> dict:
        return self.state().get("tags", {}).get(key, {})

    def adds(self, key: str) -> list[str]:
        return self.lists(key).get("add", [])

    def removes(self, key: str) -> list[str]:
        return self.lists(key).get("remove", [])

    def retire(self, name: str) -> None:
        """The library retires `name` (holding it first when it never did)."""
        state = self.state()
        state["vocabulary"].setdefault(name, {"created_at": "2026-01-01T00:00:00Z"})[
            "retired_at"
        ] = "2026-01-02T00:00:00Z"
        (self.root / "library.json").write_text(json.dumps(state))

    def runs(self) -> list[list[str]]:
        return [json.loads(line)["argv"][2:] for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def kmt(self, *args: str) -> None:
        """A change made in the Library, by the same binary the app runs."""
        subprocess.run([self.app.binary, "--root", str(self.root), *args], check=True, capture_output=True)

    def tag(self, rows: list[int], name: str, value: bool = True) -> dict:
        name = self.post("/api/tags", {"name": name})["tag"]  # the library's spelling, when it has the name
        return self.post("/api/apply", {"rows": rows, "tag": name, "value": value})

    def no(self, rows: list[int], name: str) -> None:
        """A "no" in the app: the tag put on, then taken off."""
        self.tag(rows, name)
        self.tag(rows, name, False)

    def send(self) -> dict:
        self.post("/api/send/preview")
        return self.post("/api/send")

    def settle(self) -> None:
        """The first send: the synthetic library's own tags come off; its forgotten page stays forgotten."""
        self.assertTrue(self.send()["ok"])

    def counts(self, out: dict) -> dict:
        keys = ("tags_on", "tags_off", "never_tags", "cleared", "dismiss_tags", "forget", "kept_forgotten", "missing")
        return {k: out[k] for k in keys if out.get(k)}

    def assert_mirrored(self) -> None:
        archive = Archive.load(self.root)
        app_tags = self.app.store.app_tags()
        for key in archive.known:
            fold = lambda tags: sorted(t.lower() for t in tags)  # noqa: E731
            self.assertEqual(fold(app_tags.get(key, [])), fold(archive.state.tags(key)), "tags differ")
        self.assertLessEqual(set(self.app.forgotten.keys) & set(archive.known), archive.state.forgotten)
        self.assertFalse(self.post("/api/send/preview")["ready"])

    def test_the_preview_counts_the_mirror_and_writes_nothing(self):
        self.tag(self.rows[:2], "Trains")
        self.tag(self.rows[1:3], "Sleeper cars")
        self.no(self.rows[2:3], "Trains")
        self.post("/api/tags", {"name": "Night trains"})
        self.post("/api/forget", {"rows": [self.rows[3]], "value": True})
        before = (self.root / "library.json").read_bytes()
        out = self.post("/api/send/preview")
        expect = {"tags_on": 4, "tags_off": 99, "never_tags": 99, "cleared": 85, "dismiss_tags": 1, "forget": 1}
        self.assertEqual({**expect, "kept_forgotten": 1}, self.counts(out))
        self.assertEqual((3, 85, 85, 1), (out["pages_on"], out["pages_off"], out["never_pages"], out["dismiss_pages"]))
        self.assertEqual((["Night trains", "Sleeper cars"], []), (out["new"], out["revived"]))
        self.assertIn(" tags '--create=Night trains' '--create=Sleeper cars'", out["commands"][0])
        self.assertIn("tag '--add=Sleeper cars' -- ", out["commands"][1])
        self.assertIn("tag --add=Trains -- ", out["commands"][2])
        self.assertIn(shlex.join(["tag", "--remove=Trains", "--", self.keys[2]]), out["commands"][-2])
        self.assertIn(" forget -- ", out["commands"][-1])
        self.assertEqual(before, (self.root / "library.json").read_bytes())
        self.assertEqual([], self.runs())
        self.assertFalse((self.app.store.root / "exports").exists(), "the record is written by the send")

    def test_a_send_mirrors_the_app_and_a_resend_has_nothing_to_send(self):
        self.tag(self.rows[:2], "Sleeper cars")
        self.post("/api/forget", {"rows": [self.rows[3]], "value": True})
        out = self.send()
        self.assertTrue(out["ok"], out)
        self.assertIsNone(out["error"])
        self.assertEqual(["Sleeper cars"], self.adds(self.keys[0]))
        self.assertIn("Sleeper cars", self.state()["vocabulary"])
        self.assertEqual([self.keys[3]], self.app.forgotten.keys, "the app's forgotten list stays")
        self.assertFalse(self.app.live()[self.rows[3]])
        self.assertTrue(Path(out["record"]["answers"]).is_file())
        self.assertEqual(self.keys[3] + "\0", Path(out["record"]["forget"]).read_text())
        self.assertTrue(all(step["ok"] for step in out["steps"]))
        self.assert_mirrored()
        runs = len(self.runs())
        again = self.post("/api/send/preview")
        self.assertEqual(([], {"kept_forgotten": 1}), (again["commands"], self.counts(again)))
        self.assertEqual(runs, len(self.runs()))

    def test_a_library_with_its_own_tags_ends_equal_to_the_app(self):
        owned = [k for k in self.start.known if self.start.state.tags(k) and k not in self.start.state.forgotten]
        tagged = [self.lib.rows[k] for k in owned][:4]
        names = [self.start.state.tags(self.lib.records[r]["key"])[0] for r in tagged]
        self.tag(tagged[:1], names[0].upper())  # the library's tag, typed in another case: no change
        self.tag(tagged[1:2], next(n for n in self.start.state.active.values() if n != names[1]))  # another known name
        self.tag(tagged[2:3], "Sleeper cars")  # a new name
        self.tag(tagged[3:4], names[3], False)  # off here: the page ends with none
        self.post("/api/forget", {"rows": tagged[:1], "value": True})
        self.assertTrue(self.send()["ok"])
        self.assert_mirrored()

    def test_a_page_the_app_never_tagged_loses_its_library_tags(self):
        self.settle()
        self.kmt("tag", "--add=Trains", "--", self.keys[0])
        self.kmt("tag", "--add=Sleeper cars", "--", self.keys[0])
        out = self.post("/api/send/preview")
        self.assertEqual({"tags_off": 2, "never_tags": 2, "cleared": 1, "kept_forgotten": 1}, self.counts(out))
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertEqual([], self.adds(self.keys[0]))
        self.assert_mirrored()

    def test_a_tag_taken_off_in_the_app_comes_off(self):
        self.tag(self.rows[:2], "Trains")
        self.settle()
        self.tag(self.rows[:1], "Trains", False)
        out = self.post("/api/send/preview")
        expect = {"tags_off": 1, "cleared": 1, "kept_forgotten": 1}
        self.assertEqual(expect, self.counts(out), "tagged here once, so not never tagged")
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertEqual(([], ["Trains"]), (self.adds(self.keys[0]), self.adds(self.keys[1])))
        self.assert_mirrored()

    def test_a_retired_name_comes_back_only_where_the_app_has_it(self):
        self.settle()
        self.retire("Sleeper cars")
        state = self.state()
        state.setdefault("tags", {})[self.keys[1]] = {"add": ["Sleeper cars"], "remove": []}
        (self.root / "library.json").write_text(json.dumps(state))
        self.tag(self.rows[:1], "sleeper cars")
        out = self.post("/api/send/preview")
        self.assertEqual((["sleeper cars"], []), (out["revived"], out["new"]))
        expect = {"tags_on": 1, "tags_off": 1, "never_tags": 1, "kept_forgotten": 1}
        self.assertEqual(expect, self.counts(out), "hidden while retired")
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertIsNone(self.state()["vocabulary"]["Sleeper cars"].get("retired_at"))
        self.assertEqual((["Sleeper cars"], []), (self.adds(self.keys[0]), self.adds(self.keys[1])))
        self.assert_mirrored()

    def test_a_page_forgotten_only_in_the_library_stays_forgotten(self):
        kept = next(iter(self.start.state.forgotten))
        self.settle()
        self.assertFalse(self.app.live()[self.lib.rows[kept]], "the library's own forgotten page stays hidden")
        self.kmt("forget", "--", self.keys[1])  # in the Library only, as its view and Add link's Remove forget
        self.post("/api/forget", {"rows": [self.rows[0]], "value": True})
        out = self.post("/api/send/preview")
        self.assertEqual({"forget": 1, "kept_forgotten": 2}, self.counts(out))
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertEqual(sorted([kept, self.keys[0], self.keys[1]]), sorted(self.state()["forgotten"]))
        self.assertEqual([self.keys[0]], self.app.forgotten.keys)
        self.assertFalse(self.app.live()[self.rows[0]])
        self.assert_mirrored()

    def test_pages_the_library_does_not_hold_are_counted_and_left_out(self):
        with self.app.lock:
            self.app.store.apply(["synthetic-missing"], "Trains", True)
            self.app.forgotten.set(["synthetic-gone"])
        out = self.post("/api/send/preview")
        self.assertEqual(2, out["missing"])
        self.assertFalse(any("synthetic-" in command for command in out["commands"]))
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertEqual({"kept_forgotten": 1, "missing": 2}, self.counts(self.post("/api/send/preview")))
        self.assert_mirrored()

    def test_a_no_for_a_new_name_creates_it_before_its_dismissal(self):
        self.settle()
        self.no(self.rows[:1], "Sleeper cars")
        out = self.post("/api/send/preview")
        self.assertEqual(({"dismiss_tags": 1, "kept_forgotten": 1}, ["Sleeper cars"]), (self.counts(out), out["new"]))
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertEqual(["Sleeper cars"], self.removes(self.keys[0]))
        self.assertIsNone(self.state()["vocabulary"]["Sleeper cars"].get("retired_at"))
        create, dismiss = ["tags", "--create=Sleeper cars"], ["tag", "--remove=Sleeper cars", "--", self.keys[0]]
        self.assertEqual([create, dismiss], self.runs()[-2:])
        self.assert_mirrored()

    def test_a_no_becomes_a_dismissal_only_where_the_library_is_undecided(self):
        self.settle()
        self.kmt("tag", "--add=Trains", "--", self.keys[0])
        self.kmt("tag", "--remove=Trains", "--", self.keys[1])
        self.no(self.rows[:3], "Trains")
        out = self.post("/api/send/preview")
        self.assertEqual({"tags_off": 1, "cleared": 1, "dismiss_tags": 1, "kept_forgotten": 1}, self.counts(out))
        self.assertFalse(any(self.keys[1] in command for command in out["commands"]), "already dismissed")
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertEqual([{"add": [], "remove": ["Trains"]}] * 3, [self.lists(k) for k in self.keys[:3]])
        self.assert_mirrored()

    def test_a_retired_name_takes_a_dismissal_without_revival(self):
        self.settle()
        self.no(self.rows[:1], "Trains")
        self.retire("Trains")
        out = self.post("/api/send/preview")
        self.assertEqual(([], []), (out["new"], out["revived"]))
        self.assertEqual({"dismiss_tags": 1, "kept_forgotten": 1}, self.counts(out))
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertEqual(["Trains"], self.removes(self.keys[0]))
        self.assertIsNotNone(self.state()["vocabulary"]["Trains"].get("retired_at"))
        self.assert_mirrored()

    def test_a_name_made_in_the_app_reaches_the_vocabulary(self):
        self.settle()
        self.post("/api/tags", {"name": "Sleeper cars"})
        out = self.post("/api/send/preview")
        self.assertEqual((["Sleeper cars"], True), (out["new"], out["ready"]))
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertIsNone(self.state()["vocabulary"]["Sleeper cars"].get("retired_at"))
        self.assert_mirrored()

    def test_the_extended_send_converges_to_an_empty_preview(self):
        self.tag(self.rows[:2], "Sleeper cars")  # a new name, added
        self.post("/api/tags", {"name": "Night trains"})  # a new name on no page
        self.no(self.rows[2:3], "Trains")
        self.no(self.rows[3:4], "Typography")
        self.retire("Typography")  # a dismissal without revival
        self.tag(self.rows[:1], "Keyboards")
        self.retire("Keyboards")  # added, so back from retirement
        self.post("/api/forget", {"rows": [self.rows[1]], "value": True})
        out = self.post("/api/send/preview")
        expect = ("tags_on", "tags_off", "never_tags", "cleared", "dismiss_tags", "forget", "kept_forgotten")
        self.assertEqual(expect, tuple(self.counts(out)), "every category at once")
        self.assertEqual((["Night trains", "Sleeper cars"], ["Keyboards"]), (out["new"], out["revived"]))
        self.assertEqual(2, out["dismiss_tags"])
        self.assertTrue(self.post("/api/send")["ok"])
        again = self.post("/api/send/preview")
        self.assertEqual(([], {"kept_forgotten": 1}), (again["commands"], self.counts(again)))
        self.assert_mirrored()

    @fake_only
    def test_a_failed_create_stops_the_send_and_a_resend_converges(self):
        self.settle()
        self.no(self.rows[:1], "Sleeper cars")
        before = (self.root / "library.json").read_bytes()
        with patch.dict(os.environ, {"FAKE_KMT_FAIL": "tags"}):
            out = self.send()
        self.assertEqual(
            (False, "error: tags failed", [False]), (out["ok"], out["error"], [s["ok"] for s in out["steps"]])
        )
        self.assertEqual(before, (self.root / "library.json").read_bytes())
        self.assertTrue(self.send()["ok"])
        self.assertEqual(["Sleeper cars"], self.removes(self.keys[0]))
        self.assert_mirrored()

    def test_undo_forget_after_send_shows_the_page_here_and_leaves_the_library_forgotten(self):
        row, key = self.rows[0], self.keys[0]
        self.post("/api/forget", {"rows": [row], "value": True})
        self.assertTrue(self.send()["ok"])
        self.assertIn(key, self.state()["forgotten"])
        before = int(self.app.live().sum())
        self.post("/api/forget", {"rows": [row], "value": False})
        self.assertTrue(self.app.live()[row], "Undo shows the page before sending the restore")
        self.assertEqual(before + 1, self.app.library()["pages"])
        out = self.post("/api/send/preview")
        self.assertEqual(({"kept_forgotten": 2}, False), (self.counts(out), out["ready"]), "Send never restores")
        self.assertIn(key, self.state()["forgotten"])
        self.assert_mirrored()

    def test_a_partial_failure_then_a_resend_converges(self):
        self.tag(self.rows[:2], "Sleeper cars")
        self.post("/api/forget", {"rows": [self.rows[3]], "value": True})
        original = send.run

        def no_forget(app, call):
            return "error: synthetic forget failure" if call.args[0] == "forget" else original(app, call)

        with patch.object(send, "run", side_effect=no_forget):
            out = self.send()
        self.assertEqual((False, "error: synthetic forget failure"), (out["ok"], out["error"]))
        self.assertEqual([True] * (len(out["steps"]) - 1) + [False], [s["ok"] for s in out["steps"]])
        self.assertEqual(["Sleeper cars"], self.adds(self.keys[0]), "the calls before it stay applied")
        self.assertNotIn(self.keys[3], self.state()["forgotten"])
        again = self.post("/api/send/preview")
        self.assertEqual({"forget": 1, "kept_forgotten": 1}, self.counts(again), "the rest, and nothing twice")
        self.assertTrue(self.post("/api/send")["ok"])
        self.assert_mirrored()

    def test_an_uncertain_failure_after_a_write_needs_no_second_write(self):
        self.tag(self.rows[:1], "Trains")
        original = send.run

        def failed_after_write(app, call):
            self.assertIsNone(original(app, call))
            return "synthetic timeout after write"

        with patch.object(send, "run", side_effect=failed_after_write):
            out = self.send()
        self.assertFalse(out["ok"])
        self.assertEqual(["Trains"], self.adds(self.keys[0]))
        again = self.post("/api/send/preview")
        self.assertFalse(any("--add=Trains" in command for command in again["commands"]))
        self.assertTrue(self.post("/api/send")["ok"])
        self.assert_mirrored()

    @fake_only
    def test_a_failed_first_call_leaves_the_library_unchanged_and_the_error_on_screen(self):
        self.tag(self.rows[:2], "Trains")
        before = (self.root / "library.json").read_bytes()
        with patch.dict(os.environ, {"FAKE_KMT_FAIL": "--add=Trains"}):
            out = self.send()
        self.assertEqual((False, "error: --add=Trains failed"), (out["ok"], out["error"]))
        self.assertEqual([False], [s["ok"] for s in out["steps"]])
        self.assertEqual(before, (self.root / "library.json").read_bytes())
        self.assertEqual(2, self.post("/api/send/preview")["tags_on"])

    def test_a_call_that_cannot_run_or_times_out_stops_the_send(self):
        self.tag(self.rows[:1], "Sleeper cars")
        self.app.binary = str(self.work / "absent")
        self.assertRegex(self.send()["error"], "^could not run knowmoretabs: ")
        with patch("tagger.app.send.subprocess.run", side_effect=subprocess.TimeoutExpired("knowmoretabs", 1)):
            out = self.send()
        self.assertEqual(f"knowmoretabs did not finish within {send.TIMEOUT} seconds", out["error"])

    def test_timeout_kills_and_reaps_the_actual_child(self):
        pidfile = self.work / "sleeping-pid"
        binary = self.work / "sleeping-kmt"
        binary.write_text(
            f"#!{sys.executable}\nimport os, time\nfrom pathlib import Path\n"
            f"Path({str(pidfile)!r}).write_text(str(os.getpid()))\ntime.sleep(30)\n"
        )
        binary.chmod(0o700)
        self.app.binary = str(binary)
        self.tag(self.rows[:1], "Trains")
        with patch.object(send, "TIMEOUT", 1):
            out = self.send()
        self.assertFalse(out["ok"])
        self.assertIn("did not finish", out["error"])
        with self.assertRaises(ProcessLookupError):
            os.kill(int(pidfile.read_text()), 0)

    def test_a_send_needs_a_preview_and_long_lists_are_split(self):
        self.assertEqual("nothing previewed to send", self.post("/api/send", ok=400)["error"])
        with patch.object(send, "ARGS", 14):
            calls = send.Plan(forget=["a" * 6, "b" * 6, "c" * 6]).calls()
        self.assertEqual([["a" * 6, "b" * 6], ["c" * 6]], [c.urls for c in calls])

    def test_batches_use_encoded_bytes_and_keep_each_address_intact(self):
        urls = ["synthetic-" + "雪" * 4, "synthetic-'quoted'", "synthetic-two words"]
        with patch.object(send, "ARGS", 34):
            calls = send.Plan(on={"-Owner's tag": urls}).calls()
        self.assertEqual(urls, [url for call in calls for url in call.urls])
        for call in calls:
            self.assertLessEqual(sum(len(os.fsencode(url)) + 1 for url in call.urls), 34)
            self.assertEqual(call.urls, call.args[3:])
            self.assertEqual("--add=-Owner's tag", call.args[1])
            self.assertEqual(
                [self.app.binary, "--root", str(self.root), *call.args], shlex.split(call.command(self.app))
            )

    def test_a_snapshot_root_is_never_sent_to(self):
        data = self.work / "lab-data"
        protected = data / "snapshot-synthetic"
        protected.mkdir(parents=True)
        self.app.lib = replace(self.app.lib, root=protected)
        with patch("tagger.app.send.subprocess.run") as run:
            out = self.post("/api/send/preview", ok=400)
            self.post("/api/send", ok=400)
        self.assertEqual("this app runs on a snapshot copy, which Send never writes", out["error"])
        run.assert_not_called()

    def test_changed_decisions_require_a_new_preview_before_any_write(self):
        batch = self.tag(self.rows[:1], "Sleeper cars")["batch"]
        first = self.post("/api/send/preview")
        self.post("/api/undo", {"batch": batch})
        status, _ = self.call(self.port, "POST", "/api/send", {"preview": first.get("preview")})
        self.assertEqual(400, status)
        self.assertEqual([], self.runs())
        self.assertFalse((self.app.store.root / "exports").exists())

    def test_another_clients_preview_cannot_replace_the_confirmed_plan(self):
        self.tag(self.rows[:1], "Sleeper cars")
        first = self.post("/api/send/preview")
        self.tag(self.rows[1:2], "Trains")
        self.post("/api/send/preview")
        status, _ = self.call(self.port, "POST", "/api/send", {"preview": first.get("preview")})
        self.assertEqual(400, status)
        self.assertEqual([], self.runs())

    def test_a_library_change_after_preview_requires_another_preview(self):
        self.tag(self.rows[:1], "Trains")
        first = self.post("/api/send/preview")
        self.kmt("tag", "--add=Trains", "--", self.keys[0])
        count = len(self.runs())
        status, _ = self.call(self.port, "POST", "/api/send", {"preview": first.get("preview")})
        self.assertEqual(400, status)
        self.assertEqual(count, len(self.runs()))
        self.assertFalse((self.app.store.root / "exports").exists())

    def test_running_send_keeps_cli_outside_app_lock_and_refuses_another_preview(self):
        self.tag(self.rows[:1], "Sleeper cars")
        first = self.post("/api/send/preview")
        entered, release = threading.Event(), threading.Event()
        original = send.run

        def blocked(app, call):
            entered.set()
            self.assertTrue(release.wait(5))
            return original(app, call)

        with patch.object(send, "run", side_effect=blocked), ThreadPoolExecutor() as pool:
            future = pool.submit(self.call, self.port, "POST", "/api/send", {"preview": first.get("preview")})
            try:
                self.assertTrue(entered.wait(5))
                self.assertTrue(self.app.lock.acquire(timeout=1), "CLI must not hold the app lock")
                self.app.lock.release()
                status, _ = self.call(self.port, "POST", "/api/send/preview", {})
                self.assertEqual(400, status)
                status, _ = self.call(self.port, "POST", "/api/send", {"preview": first.get("preview")})
                self.assertEqual(400, status)
                self.tag(self.rows[1:2], "Trains")
            finally:
                release.set()
            self.assertTrue(future.result()[1]["ok"])
        self.assertEqual({"tags_on": 1, "kept_forgotten": 1}, self.counts(self.post("/api/send/preview")))

    def test_export_failure_returns_a_result_without_running_the_cli(self):
        self.tag(self.rows[:1], "Trains")
        self.post("/api/send/preview")
        with patch.object(self.app.store, "export", side_effect=OSError(errno.ENOSPC, "synthetic full disk")):
            out = self.post("/api/send")
        self.assertEqual((False, []), (out["ok"], out["steps"]))
        self.assertIn("could not write the send record", out["error"])
        self.assertEqual([], self.runs())
        self.assertTrue(self.send()["ok"])

    def test_send_marks_left_by_an_unreleased_build_are_ignored(self):
        self.tag(self.rows[:1], "Trains")
        store = self.app.store
        store.path.write_text(json.dumps({**store.state, "sent": {self.keys[0]: {"Trains": True}}}))
        self.assertNotIn("sent", Store(store.root).state)
        self.app.store = Store(store.root)
        self.assertEqual(1, self.post("/api/send/preview")["tags_on"])


@unittest.skipUnless(os.environ.get("KMT_BIN"), "KMT_BIN names no knowmoretabs build")
class RealSendTests(SendTests):
    def setUp(self):
        if getattr(getattr(self, self._testMethodName), "fake_only", False):
            self.skipTest("makes the fake knowmoretabs fail a call")
        super().setUp()
        self.app.binary = logged(self.work, os.environ["KMT_BIN"])
