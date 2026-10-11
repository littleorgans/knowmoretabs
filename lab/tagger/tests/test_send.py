"""Send to library on the synthetic archive against a fake knowmoretabs: the preview writes nothing, the send puts
the app's tags on as the owner's own and forgets the app's forgotten pages, a resend converges, a later Library edit
is kept, a failed call stops the send with the error on screen, and a snapshot root is never written."""

import json
import os
import subprocess
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

import numpy as np
from test_add import Served, fake_binary

from tagger.app import send


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

    def post(self, path: str, body: dict | None = None, ok: int = 200) -> dict:
        status, out = self.call(self.port, "POST", path, body or {})
        self.assertEqual(ok, status, out)
        return out

    def state(self) -> dict:
        return json.loads((self.root / "library.json").read_text())

    def tags(self, key: str) -> dict:
        page = self.state().get("tags", {}).get(key, {})
        return {"add": page.get("add", []), "remove": page.get("remove", [])}

    def runs(self) -> list[list[str]]:
        return [json.loads(line)["argv"][2:] for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def tag(self, rows: list[int], name: str, value: bool = True) -> dict:
        self.post("/api/tags", {"name": name})
        return self.post("/api/apply", {"rows": rows, "tag": name, "value": value})

    def send(self) -> dict:
        self.post("/api/send/preview")
        return self.post("/api/send")

    def test_the_preview_plans_tags_new_names_and_forgets_and_writes_nothing(self):
        self.tag(self.rows[:2], "Trains")
        self.tag(self.rows[1:3], "Sleeper cars")
        self.post("/api/forget", {"rows": [self.rows[3]], "value": True})
        before = (self.root / "library.json").read_bytes()
        out = self.post("/api/send/preview")
        expect = {"tags_on": 4, "pages_on": 3, "tags_off": 0, "pages_off": 0, "new": ["Sleeper cars"], "forget": 1}
        self.assertEqual(expect, {k: out[k] for k in expect})
        self.assertEqual(([], 0), (out["revived"], out["skipped"]))
        self.assertEqual(3, len(out["commands"]))  # one call per tag, then the forget
        self.assertIn("tag --add=Trains -- ", out["commands"][1])
        self.assertEqual(before, (self.root / "library.json").read_bytes())
        self.assertEqual([], self.runs())
        self.assertFalse((self.app.store.root / "exports").exists(), "the record is written by the send")

    def test_a_send_puts_the_owners_own_tags_on_forgets_and_a_resend_has_nothing_to_do(self):
        self.tag(self.rows[:2], "Sleeper cars")
        self.post("/api/forget", {"rows": [self.rows[3]], "value": True})
        out = self.send()
        self.assertTrue(out["ok"], out)
        self.assertIsNone(out["error"])
        self.assertEqual({"add": ["Sleeper cars"], "remove": []}, self.tags(self.keys[0]))
        self.assertIn("Sleeper cars", self.state()["vocabulary"])
        self.assertIn(self.keys[3], self.state()["forgotten"])
        self.assertEqual([], self.app.forgotten.keys, "a page restored in the Library is not forgotten again")
        self.assertFalse(self.app.live()[self.rows[3]], "the app hides what the library forgot")
        self.assertTrue(Path(out["record"]["answers"]).is_file())
        self.assertEqual(self.keys[3] + "\0", Path(out["record"]["forget"]).read_text())
        self.assertEqual([True, True], [s["ok"] for s in out["steps"]])
        runs = len(self.runs())
        again = self.post("/api/send/preview")
        self.assertEqual(([], 0, 0), (again["commands"], again["tags_on"], again["forget"]))
        self.assertEqual(runs, len(self.runs()))

    def test_a_tag_taken_off_in_the_app_comes_off_and_a_library_edit_since_is_kept(self):
        self.tag(self.rows[:3], "Sleeper cars")
        self.send()
        # in the Library, the owner takes the tag off the second page; the app never sees it
        subprocess.run([self.app.binary, "--root", str(self.root), "tag", "--remove=Sleeper cars", "--", self.keys[1]])
        self.tag(self.rows[:1], "Sleeper cars", False)
        batch = self.tag(self.rows[3:4], "Sleeper cars")["batch"]  # sent, then undone below
        self.send()
        self.post("/api/undo", {"batch": batch})
        out = self.post("/api/send/preview")
        self.assertEqual((0, 1, 1), (out["tags_on"], out["tags_off"], out["pages_off"]))
        self.post("/api/send")
        self.assertEqual({"add": [], "remove": ["Sleeper cars"]}, self.tags(self.keys[0]))
        self.assertEqual({"add": [], "remove": ["Sleeper cars"]}, self.tags(self.keys[1]), "not added back")
        self.assertEqual({"add": ["Sleeper cars"], "remove": []}, self.tags(self.keys[2]))
        self.assertEqual({"add": [], "remove": ["Sleeper cars"]}, self.tags(self.keys[3]), "Undo takes it off")

    def test_a_failed_forget_keeps_the_tags_sent_and_a_resend_runs_only_the_forget(self):
        self.tag(self.rows[:2], "Sleeper cars")
        self.post("/api/forget", {"rows": [self.rows[3]], "value": True})
        with patch.dict(os.environ, {"FAKE_KMT_FAIL": "forget"}):
            out = self.send()
        self.assertFalse(out["ok"])
        self.assertEqual("error: forget failed", out["error"])
        self.assertEqual([True, False], [s["ok"] for s in out["steps"]])
        self.assertEqual(["Sleeper cars"], self.tags(self.keys[0])["add"])
        self.assertNotIn(self.keys[3], self.state()["forgotten"])
        self.assertEqual([self.keys[3]], self.app.forgotten.keys)
        again = self.post("/api/send/preview")
        self.assertEqual((0, 1, 1), (again["tags_on"], again["forget"], len(again["commands"])))
        self.assertTrue(self.post("/api/send")["ok"])
        self.assertIn(self.keys[3], self.state()["forgotten"])

    def test_a_failed_first_call_leaves_the_library_unchanged_and_the_error_on_screen(self):
        self.tag(self.rows[:2], "Sleeper cars")
        before = (self.root / "library.json").read_bytes()
        with patch.dict(os.environ, {"FAKE_KMT_FAIL": "--add=Sleeper cars"}):
            out = self.send()
        self.assertEqual((False, "error: --add=Sleeper cars failed"), (out["ok"], out["error"]))
        self.assertEqual(before, (self.root / "library.json").read_bytes())
        self.assertEqual(2, self.post("/api/send/preview")["tags_on"], "nothing marked sent")

    def test_a_call_that_cannot_run_or_times_out_stops_the_send(self):
        self.tag(self.rows[:1], "Sleeper cars")
        self.app.binary = str(self.work / "absent")
        self.assertRegex(self.send()["error"], "^could not run knowmoretabs: ")
        with patch("tagger.app.send.subprocess.run", side_effect=subprocess.TimeoutExpired("knowmoretabs", 1)):
            out = self.send()
        self.assertEqual(f"knowmoretabs did not finish within {send.TIMEOUT} seconds", out["error"])

    def test_a_send_needs_a_preview_and_long_lists_are_split(self):
        self.assertEqual("nothing previewed to send", self.post("/api/send", ok=400)["error"])
        with patch.object(send, "ARGS", 14):
            calls = send.Plan(forget=["a" * 6, "b" * 6, "c" * 6]).calls()
        self.assertEqual([["a" * 6, "b" * 6], ["c" * 6]], [c.urls for c in calls])

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

    def test_an_unsent_off_decision_records_the_owners_dismissal(self):
        self.tag(self.rows[:1], "Trains")
        self.tag(self.rows[:1], "Trains", False)
        out = self.post("/api/send/preview")
        self.assertEqual(1, out["tags_off"])
        self.post("/api/send")
        self.assertEqual(["Trains"], self.tags(self.keys[0])["remove"])

    def test_a_new_name_added_elsewhere_in_the_send_can_be_dismissed(self):
        self.tag(self.rows[:2], "Sleeper cars")
        self.tag(self.rows[:1], "Sleeper cars", False)
        out = self.post("/api/send/preview")
        self.assertEqual((1, 1), (out["tags_on"], out["tags_off"]))
        self.post("/api/send")
        self.assertEqual(["Sleeper cars"], self.tags(self.keys[0])["remove"])

    def test_already_matched_work_can_be_confirmed_without_cli_calls(self):
        self.tag(self.rows[:1], "Trains")
        subprocess.run(
            [self.app.binary, "--root", str(self.root), "tag", "--add=Trains", "--", self.keys[0]], check=True
        )
        self.post("/api/forget", {"rows": self.rows[1:2], "value": True})
        subprocess.run([self.app.binary, "--root", str(self.root), "forget", "--", self.keys[1]], check=True)
        out = self.post("/api/send/preview")
        self.assertEqual([], out["commands"])
        self.assertTrue(out["ready"], "matched decisions and forgotten pages still need app bookkeeping")
        self.assertEqual((1, 1), (out["matched"], out["settled"]))
        count = len(self.runs())
        self.post("/api/send")
        self.assertEqual(count, len(self.runs()))
        self.assertTrue(self.app.store.sent()[(self.keys[0], "Trains")])
        self.assertEqual([], self.app.forgotten.keys)
        subprocess.run(
            [self.app.binary, "--root", str(self.root), "tag", "--remove=Trains", "--", self.keys[0]], check=True
        )
        self.assertFalse(self.post("/api/send/preview")["ready"], "a later Library removal stays removed")
