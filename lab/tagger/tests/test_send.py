"""Send to library on the synthetic archive against a fake knowmoretabs: the preview counts what the mirror will
change and writes nothing; a send leaves every library page with exactly the app's tags (none for a page the app
never tagged), forgets and restores until the forgotten pages match, skips pages the library lacks, and a resend
after a partial failure converges. A failed call stops the send with the error on screen; a snapshot root is never
written. The tests that inject no CLI failure also run against the real knowmoretabs (KMT_BIN, scratch roots)."""

import json
import errno
import os
import shlex
import subprocess
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

import numpy as np
from test_add import Served, fake_binary

from tagger.app import send
from tagger.app.store import Store
from tagger.archive import Archive


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

    def adds(self, key: str) -> list[str]:
        return self.state().get("tags", {}).get(key, {}).get("add", [])

    def runs(self) -> list[list[str]]:
        return [json.loads(line)["argv"][2:] for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def kmt(self, *args: str) -> None:
        """A change made in the Library, by the same binary the app runs."""
        subprocess.run([self.app.binary, "--root", str(self.root), *args], check=True, capture_output=True)

    def tag(self, rows: list[int], name: str, value: bool = True) -> dict:
        name = self.post("/api/tags", {"name": name})["tag"]  # the library's spelling, when it has the name
        return self.post("/api/apply", {"rows": rows, "tag": name, "value": value})

    def send(self) -> dict:
        self.post("/api/send/preview")
        return self.post("/api/send")

    def settle(self) -> None:
        """The first send: the synthetic library's own tags come off and its forgotten page comes back."""
        self.assertTrue(self.send()["ok"])

    def counts(self, out: dict) -> dict:
        keys = ("tags_on", "tags_off", "never_tags", "cleared", "forget", "restore", "missing")
        return {k: out[k] for k in keys if out[k]}

    def assert_mirrored(self) -> None:
        archive = Archive.load(self.root)
        app_tags = self.app.store.app_tags()
        for key in archive.known:
            fold = lambda tags: sorted(t.lower() for t in tags)  # noqa: E731
            self.assertEqual(fold(app_tags.get(key, [])), fold(archive.owner_tags.get(key, [])), "tags differ")
        self.assertEqual(set(self.app.forgotten.keys) & set(archive.known), archive.forgotten)
        self.assertFalse(self.post("/api/send/preview")["ready"])

    def test_the_preview_counts_the_mirror_and_writes_nothing(self):
        self.tag(self.rows[:2], "Trains")
        self.tag(self.rows[1:3], "Sleeper cars")
        self.post("/api/forget", {"rows": [self.rows[3]], "value": True})
        before = (self.root / "library.json").read_bytes()
        out = self.post("/api/send/preview")
        expect = {"tags_on": 4, "tags_off": 99, "never_tags": 99, "cleared": 85, "forget": 1, "restore": 1}
        self.assertEqual(expect, self.counts(out))
        self.assertEqual((3, 85, 85), (out["pages_on"], out["pages_off"], out["never_pages"]))
        self.assertEqual((["Sleeper cars"], []), (out["new"], out["revived"]))
        self.assertIn("tag '--add=Sleeper cars' -- ", out["commands"][0])
        self.assertIn("tag --add=Trains -- ", out["commands"][1])
        self.assertIn(" restore -- ", out["commands"][-1])
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
        self.assertEqual(([], {}), (again["commands"], self.counts(again)))
        self.assertEqual(runs, len(self.runs()))

    def test_a_library_with_its_own_tags_ends_equal_to_the_app(self):
        tagged = [self.lib.rows[k] for k in sorted(self.start.owner_tags) if k not in self.start.forgotten][:4]
        names = [self.start.owner_tags[self.lib.records[r]["key"]][0] for r in tagged]
        self.tag(tagged[:1], names[0].upper())  # the library's tag, typed in another case: no change
        self.tag(tagged[1:2], next(n for n in self.start.active.values() if n != names[1]))  # another known name
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
        self.assertEqual({"tags_off": 2, "never_tags": 2, "cleared": 1}, self.counts(out))
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertEqual([], self.adds(self.keys[0]))
        self.assert_mirrored()

    def test_a_tag_taken_off_in_the_app_comes_off(self):
        self.tag(self.rows[:2], "Trains")
        self.settle()
        self.tag(self.rows[:1], "Trains", False)
        out = self.post("/api/send/preview")
        self.assertEqual({"tags_off": 1, "cleared": 1}, self.counts(out), "tagged here once, so not never tagged")
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertEqual(([], ["Trains"]), (self.adds(self.keys[0]), self.adds(self.keys[1])))
        self.assert_mirrored()

    def test_a_retired_name_comes_back_only_where_the_app_has_it(self):
        self.settle()
        state = self.state()
        state["vocabulary"]["Sleeper cars"] = {
            "created_at": "2026-01-01T00:00:00Z",
            "retired_at": "2026-01-02T00:00:00Z",
        }
        state.setdefault("tags", {})[self.keys[1]] = {"add": ["Sleeper cars"], "remove": []}
        (self.root / "library.json").write_text(json.dumps(state))
        self.tag(self.rows[:1], "sleeper cars")
        out = self.post("/api/send/preview")
        self.assertEqual((["sleeper cars"], []), (out["revived"], out["new"]))
        self.assertEqual({"tags_on": 1, "tags_off": 1, "never_tags": 1}, self.counts(out), "hidden while retired")
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertIsNone(self.state()["vocabulary"]["Sleeper cars"].get("retired_at"))
        self.assertEqual((["Sleeper cars"], []), (self.adds(self.keys[0]), self.adds(self.keys[1])))
        self.assert_mirrored()

    def test_forgotten_pages_are_forgotten_and_restored_until_both_lists_match(self):
        restored = self.lib.rows[next(iter(self.start.forgotten))]
        self.assertFalse(self.app.live()[restored])
        self.settle()
        self.assertTrue(self.app.live()[restored], "the app shows a page the library brought back")
        self.kmt("forget", "--", self.keys[1])  # in the Library only
        self.post("/api/forget", {"rows": [self.rows[0]], "value": True})
        out = self.post("/api/send/preview")
        self.assertEqual({"forget": 1, "restore": 1}, self.counts(out))
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertEqual([self.keys[0]], self.state()["forgotten"])
        self.assertEqual([self.keys[0]], self.app.forgotten.keys)
        self.assertEqual((False, True), (self.app.live()[self.rows[0]], self.app.live()[self.rows[1]]))
        self.assert_mirrored()

    def test_pages_the_library_does_not_hold_are_counted_and_left_out(self):
        with self.app.lock:
            self.app.store.apply(["synthetic-missing"], "Trains", True)
            self.app.forgotten.set(["synthetic-gone"])
        out = self.post("/api/send/preview")
        self.assertEqual(2, out["missing"])
        self.assertFalse(any("synthetic-" in command for command in out["commands"]))
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertEqual({"missing": 2}, self.counts(self.post("/api/send/preview")))
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
        self.assertEqual({"forget": 1, "restore": 1}, self.counts(again), "the rest, and nothing twice")
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

    def test_a_failed_first_call_leaves_the_library_unchanged_and_the_error_on_screen(self):
        self.tag(self.rows[:2], "Sleeper cars")
        before = (self.root / "library.json").read_bytes()
        with patch.dict(os.environ, {"FAKE_KMT_FAIL": "--add=Sleeper cars"}):
            out = self.send()
        self.assertEqual((False, "error: --add=Sleeper cars failed"), (out["ok"], out["error"]))
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
        self.assertEqual({"tags_on": 1}, self.counts(self.post("/api/send/preview")))

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
