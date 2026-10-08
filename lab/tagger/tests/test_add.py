"""Quick add on the synthetic archive: the intake fold, the dataset as a subset of the archive with known pages it
lacks embedded at load from `--root`, a library with no owner tags, every store tag name loaded, page keyed image
addresses, the fixed port, and the Add link jobs driven through HTTP against a fake knowmoretabs."""

import http.client
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

import fake_knowmoretabs
import numpy as np
from test_app import DIM, build, fake_embed, fake_encode

from tagger import cli
from tagger.app import add, engine, server
from tagger.app.store import Store
from tagger.archive import Archive, sha256_hex

NEW = "https://added.example/notes/one"


def fake_binary(folder: Path) -> str:
    path = folder / "knowmoretabs"
    path.write_text(f"#!/bin/sh\nexec '{sys.executable}' '{fake_knowmoretabs.__file__}' \"$@\"\n")
    path.chmod(0o700)
    return str(path)


def capture(root: Path, url: str, title: str | None = None, image: bool = True) -> None:
    """What `knowmoretabs add` leaves for a page: an intake line, its text and, with `image`, its picture."""
    name = sha256_hex(url)
    line = {"schema_version": 1, "url": url, "added_at": fake_knowmoretabs.AT, **({"title": title} if title else {})}
    fake_knowmoretabs.append(root / "pages/added.jsonl", line)
    (root / "pages/content" / f"{name}.md").write_text(f"---\nurl: {url}\n---\n{fake_knowmoretabs.BODY}\n")
    log = {"schema_version": 1, "url": url, "status": "ok", "attempted_at": fake_knowmoretabs.AT}
    fake_knowmoretabs.append(root / "pages/content.jsonl", log)
    if image:
        shutil.copy(next((root / "pages/images").glob("*.jpg")), root / "pages/images" / f"{name}.jpg")
        fake_knowmoretabs.append(root / "pages/images.jsonl", log)


class Synthetic(unittest.TestCase):
    """A fresh copy of the synthetic archive per test (jobs write to it); the dataset and vectors are shared."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp())
        cls.paths = build(cls.tmp)

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp)

    def setUp(self):
        self.work = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.work)
        self.root = self.work / "root"
        shutil.copytree(self.tmp / "archive", self.root)
        # the running root is the only archive: the snapshot path the `dataset` step reads does not exist
        missing = patch.dict(os.environ, {"KMT_TAGGER_SNAPSHOT": str(self.work / "no-snapshot")})
        missing.start()
        self.addCleanup(missing.stop)

    def load(self, encode=fake_encode) -> tuple[engine.Library, dict]:
        return engine.load(self.paths, self.root, encode, fake_embed)


class IntakeFoldTests(Synthetic):
    def test_added_pages_join_the_known_pages_titled_by_their_latest_line(self):
        before = Archive.load(self.root)
        log = self.root / "pages/added.jsonl"
        lines = [
            {"schema_version": 1, "url": NEW, "added_at": "2026-10-08T10:00:00Z"},
            {
                "schema_version": 1,
                "url": "https://added.example/two",
                "added_at": "2026-10-08T10:01:00Z",
                "title": "Two",
            },
            {"schema_version": 1, "url": NEW, "added_at": "2026-10-08T10:02:00Z", "title": "One note"},
            {"schema_version": 1, "url": "http://localhost:3000/x", "added_at": "2026-10-08T10:03:00Z"},
            {"schema_version": 1, "url": "file:///tmp/x", "added_at": "2026-10-08T10:03:00Z"},
            {"schema_version": 2, "url": "https://added.example/v2", "added_at": "2026-10-08T10:04:00Z"},
            {"schema_version": 1, "url": "https://added.example/undated"},
        ]
        log.write_text("".join(json.dumps(line) + "\n" for line in lines) + "\nnot json\n")
        after = Archive.load(self.root)
        self.assertEqual(sorted({*before.known, NEW, "https://added.example/two"}), after.known)
        self.assertEqual(("One note", "Two"), (after.titles[NEW], after.titles["https://added.example/two"]))

    def test_a_later_line_without_a_title_wins(self):
        log = self.root / "pages/added.jsonl"
        fake_knowmoretabs.append(
            log, {"schema_version": 1, "url": NEW, "added_at": "2026-10-08T10:00:00Z", "title": "A"}
        )
        fake_knowmoretabs.append(log, {"schema_version": 1, "url": NEW, "added_at": "2026-10-08T10:01:00Z"})
        self.assertEqual("", Archive.load(self.root).titles[NEW])


class LoadTests(Synthetic):
    def test_a_library_with_no_owner_vocabulary_starts_with_no_tags(self):
        state = json.loads((self.root / "library.json").read_text())
        for term in state["vocabulary"].values():
            term["retired_at"] = "2026-10-08T00:00:00Z"
        (self.root / "library.json").write_text(json.dumps(state))

        def encode(texts):  # as sentence-transformers does, no texts give a flat empty array
            return np.zeros(0, np.float32) if not texts else fake_encode(texts)

        lib, stats = self.load(encode)
        self.assertEqual(([], 0, 0), (lib.tags, stats["tags"], stats["labelled_pages"]))
        self.assertEqual((0, DIM), lib.rules.q.shape)
        self.assertEqual((len(lib.records), 0), lib.S.shape)
        self.assertEqual(0, len(lib.rules.heads))
        q = fake_encode(["night train"])[0]
        self.assertEqual(5, len(engine.search(lib, q, "night train", 5)))
        app = server.App(lib, Store(self.work / "app"), encode, fake_embed)
        self.assertEqual({"tags": [], "app_tags": []}, {k: app.library()[k] for k in ("tags", "app_tags")})
        made = app.tag({"name": "Trains"})
        self.assertTrue(made["created"])
        self.assertEqual(["Trains"], [s["tag"] for s in engine.suggest(app.lib, [0, 1])])

    def test_known_pages_the_dataset_lacks_are_embedded_at_load_from_the_running_root(self):
        before, _ = self.load()
        capture(self.root, NEW, "An invented page on zeppelin timetables")
        gone = "https://added.example/gone"
        capture(self.root, gone, image=False)
        state = json.loads((self.root / "library.json").read_text())
        (self.root / "library.json").write_text(json.dumps({**state, "forgotten": [*state["forgotten"], gone]}))
        lib, stats = self.load()
        self.assertEqual((2, len(before.records) + 2), (stats["gap_pages"], stats["pages"]))
        self.assertEqual([r["key"] for r in before.records], [r["key"] for r in lib.records[:-2]])
        row = lib.rows[NEW]
        r = lib.records[row]
        self.assertEqual("An invented page on zeppelin timetables", r["title"])
        self.assertEqual(self.root / "pages/content" / f"{sha256_hex(NEW)}.md", Path(r["content_path"]))
        self.assertEqual(self.root / "pages/images" / f"{sha256_hex(NEW)}.jpg", Path(r["image_path"]))
        np.testing.assert_allclose(fake_embed([r], lib.input_name)[0], lib.X[row])
        self.assertFalse(lib.Y[row].any() or lib.has_image[row] or lib.image[row].any())
        self.assertEqual((True, False), (bool(lib.live[row]), bool(lib.live[lib.rows[gone]])))
        np.testing.assert_allclose(lib.rules.scores(lib.X[row : row + 1])["supervised"][0], lib.S[row], rtol=1e-6)
        hits = engine.search(lib, fake_encode(["zeppelin"])[0], "zeppelin", 3)
        self.assertEqual(row, hits[0]["row"], "the keyword index covers the page's text, read under the root")
        self.assertGreater(hits[0]["sources"]["keyword"]["score"], 0)

    def test_pages_added_after_app_tags_are_scored_by_them(self):
        lib = engine.add_tags(self.load()[0], ["Sleeper cars"], fake_encode)
        capture(self.root, NEW)
        lib = engine.sync(lib, Archive.load(self.root), NEW, fake_embed)
        row, j = lib.rows[NEW], lib.tags.index("Sleeper cars")
        self.assertEqual(len(lib.records), len(lib.S))
        self.assertAlmostEqual(float(lib.X[row] @ fake_encode(["Sleeper cars"])[0]), float(lib.S[row, j]), places=5)

    def test_appended_pages_update_the_spread_used_by_suggestions_and_prechecks(self):
        lib = engine.add_tags(self.load()[0], ["Sleeper cars"], fake_encode)
        capture(self.root, NEW)
        lib = engine.sync(lib, Archive.load(self.root), NEW, fake_embed)
        expected = np.maximum(lib.S.std(axis=0), 1e-12)
        np.testing.assert_allclose(lib.spread, expected)
        row = lib.rows[NEW]
        for result in engine.suggest(lib, [row], len(lib.tags)):
            col = lib.tags.index(result["tag"])
            self.assertAlmostEqual(
                float((lib.S[row, col] - lib.S[:, col].mean()) / expected[col]), result["z"], places=5
            )

    def test_sync_follows_the_archive_for_one_page(self):
        lib, _ = self.load()
        self.assertIs(lib, engine.sync(lib, Archive.load(self.root), NEW, fake_embed), "a page the archive lacks")
        key = lib.records[5]["key"]
        state = json.loads((self.root / "library.json").read_text())
        (self.root / "library.json").write_text(json.dumps({**state, "forgotten": [key]}))
        hidden = engine.sync(lib, Archive.load(self.root), key, fake_embed)
        self.assertEqual((False, True), (bool(hidden.live[5]), bool(lib.live[5])))
        (self.root / "library.json").write_text(json.dumps({**state, "forgotten": []}))
        self.assertTrue(engine.sync(hidden, Archive.load(self.root), key, fake_embed).live[5])

    def test_a_dataset_page_the_archive_does_not_list_is_refused(self):
        state = json.loads((self.root / "snapshots/2026-10-01-000000Z/snapshot.json").read_text())
        state["tabs"] = state["tabs"][1:]
        (self.root / "snapshots/2026-10-01-000000Z/snapshot.json").write_text(json.dumps(state))
        with self.assertRaisesRegex(SystemExit, "1 of its"):
            self.load()


class StoreTagNameTests(Synthetic):
    def test_every_tag_name_the_store_holds_loads_as_a_tag(self):
        lib, _ = self.load()
        store = Store(self.work / "app")
        store.add_tag("Made here")
        store.apply([lib.records[0]["key"]], "Retired spelling", True)
        store.apply([lib.records[1]["key"]], "Taken off", True)
        store.apply([lib.records[1]["key"]], "Taken off", False)
        app = server.App(lib, store, fake_encode, fake_embed)
        names = [t["name"] for t in app.library()["tags"]]
        self.assertEqual(["Made here", "Retired spelling", "Taken off"], names[len(lib.tags) :])
        self.assertEqual(
            ["Retired spelling"], app.apply({"rows": [2], "tag": "Retired spelling", "value": True})["pages"][0]["tags"]
        )


class PortTests(unittest.TestCase):
    def test_the_app_listens_on_7879_unless_told(self):
        for args, port in (([], 7879), (["--port", "0"], 0)):
            with (
                patch("sys.argv", ["tagger", "--data", tempfile.gettempdir(), "app", *args]),
                patch("tagger.app.server.run") as run,
                patch.dict(os.environ),
            ):
                cli.main()
                self.assertEqual(port, run.call_args.args[2])


class Served(Synthetic):
    def serve(self, lib: engine.Library) -> tuple[server.App, int]:
        app = server.App(lib, Store(self.work / "lab-data" / f"app-{id(lib)}"), fake_encode, fake_embed)
        httpd = server.serve(app, 0)
        threading.Thread(target=httpd.serve_forever, daemon=True).start()
        self.addCleanup(httpd.server_close)
        self.addCleanup(httpd.shutdown)
        return app, httpd.server_address[1]

    def call(self, port: int, method: str, path: str, body=None):
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
        headers = {"Host": f"127.0.0.1:{port}", **({"Content-Type": "application/json"} if body is not None else {})}
        conn.request(method, path, None if body is None else json.dumps(body), headers)
        r = conn.getresponse()
        data = r.read()
        conn.close()
        return r.status, json.loads(data) if r.getheader("Content-Type", "").startswith("application/json") else data


class ImageAddressTests(Served):
    def test_a_row_reorder_cannot_serve_another_pages_image_under_an_address(self):
        lib, _ = self.load()
        flipped = replace(lib, records=list(reversed(lib.records)))
        served = {}
        for which in (lib, flipped):
            app, port = self.serve(which)
            for row, r in enumerate(which.records):
                if not r["image_ok"]:
                    self.assertIsNone(app.page(row)["image"])
                    continue
                address = app.page(row)["image"]
                status, body = self.call(port, "GET", f"/img/{address}")
                self.assertEqual((200, Path(r["image_path"]).read_bytes()), (status, body))
                self.assertEqual(served.setdefault(r["key"], address), address, "one address per page")
        self.assertEqual(len(served), len(set(served.values())))


class AddJobTests(Served):
    def setUp(self):
        super().setUp()
        self.lib, _ = self.load()
        self.app, self.port = self.serve(self.lib)
        self.app.adds.binary = fake_binary(self.work)
        self.log = self.work / "argv.jsonl"
        env = patch.dict(os.environ, {"FAKE_KMT_LOG": str(self.log)})
        env.start()
        self.addCleanup(env.stop)

    def add(self, url: str, action: str = "add", **flags) -> dict:
        status, job = self.call(self.port, "POST", "/api/add", {"url": url, "action": action, **flags})
        self.assertEqual(200, status, job)
        for _ in range(400):
            status, job = self.call(self.port, "GET", f"/api/add/{job['id']}")
            if job["finished"]:
                return job
            time.sleep(0.025)
        self.fail("the job never finished")

    def values(self, job: dict) -> dict:
        return {k: e.get("value", e.get("status", e["state"])) for k, e in job["stages"].items()}

    def argv(self, n: int = -1) -> list[str]:
        """The flags of the `n`th knowmoretabs run, root and link left out."""
        argv = json.loads(self.log.read_text().splitlines()[n])["argv"]
        return argv[2 : argv.index("--")]

    def logs(self) -> dict:
        return {name: (self.root / "pages" / name).read_bytes() for name in ("content.jsonl", "images.jsonl")}

    def test_a_new_page_is_added_captured_indexed_and_tagged(self):
        job = self.add(NEW)
        self.assertEqual({"library": "added", "content": "ok", "image": "ok", "search": "indexed"}, self.values(job))
        self.assertEqual(("web", fake_knowmoretabs.TITLE), (job["stages"]["content"]["tier"], job["page"]["title"]))
        self.assertFalse(job["failed"])
        page = job["page"]
        self.assertEqual((NEW, len(self.lib.records), []), (page["url"], page["row"], page["tags"]))
        self.assertEqual(200, self.call(self.port, "GET", f"/img/{page['image']}")[0])
        hits = self.call(self.port, "POST", "/api/search", {"query": "zeppelin timetables", "n": 5})[1]["hits"]
        self.assertEqual(page["row"], hits[0]["row"])
        status, out = self.call(
            self.port, "POST", "/api/apply", {"rows": [page["row"]], "tag": "Trains", "value": True}
        )
        self.assertEqual((200, ["Trains"]), (status, out["pages"][0]["tags"]))
        self.assertEqual(self.app.library()["pages"], self.call(self.port, "GET", "/api/library")[1]["pages"])
        argv = json.loads(self.log.read_text().splitlines()[0])["argv"]
        self.assertEqual(["--root", str(self.root), "add", "--json", "--", NEW], argv)

    def test_a_known_page_is_already_in_the_library_and_not_indexed_twice(self):
        key = self.lib.records[3]["key"]
        job = self.add(key)
        self.assertEqual(
            ("known", "indexed", 3), (self.values(job)["library"], self.values(job)["search"], job["page"]["row"])
        )
        self.assertEqual(len(self.lib.records), len(self.app.lib.records))

    def test_a_link_that_is_not_a_web_page_is_refused_without_a_search(self):
        job = self.add("ftp://added.example/one")
        self.assertEqual({"library": "refused"}, self.values(job))
        self.assertEqual("not_web", job["stages"]["library"]["reason"])
        self.assertEqual((False, None), (job["failed"], job["page"]))

    def test_an_option_like_link_stays_a_link(self):
        self.add("--root=/elsewhere")
        self.assertEqual(["--", "--root=/elsewhere"], json.loads(self.log.read_text())["argv"][-2:])

    def test_each_request_runs_its_commands(self):
        for kw, expected in (
            ({}, [["add", "--json", "--", NEW]]),
            ({"title": "-One"}, [["add", "--json", "--title=-One", "--", NEW]]),
            ({"signed_in": True}, [["add", "--json", "--signed-in", "--", NEW]]),
            ({"retry": ("content",)}, [["add", "--json", "--retry", "content", "--", NEW]]),
            (
                {"retry": ("content",), "signed_in": True},
                [["add", "--json", "--retry", "content", "--signed-in", "--", NEW]],
            ),
            ({"retry": ("image",)}, [["add", "--json", "--retry", "image", "--", NEW]]),
            ({"retry": ("content", "image")}, [["add", "--json", "--retry", "content", "--retry", "image", "--", NEW]]),
            ({"action": "restore"}, [["restore", "--", NEW], ["add", "--json", "--", NEW]]),
            ({"action": "forget"}, [["forget", "--", NEW]]),
            ({"action": "index"}, []),
        ):
            with self.subTest(**kw):
                self.assertEqual(expected, add.commands(add.Job(1, NEW, **{"action": "add", **kw})))

    def test_a_deep_link_title_joins_the_intake_line(self):
        job = self.add(NEW, title=" One note ")
        self.assertEqual(["add", "--json", "--title=One note"], self.argv())
        self.assertEqual("One note", json.loads((self.root / "pages/added.jsonl").read_text().splitlines()[0])["title"])
        self.assertEqual("One note", job["title"])

    def test_a_timeout_retries_the_text_alone_and_its_image_follows(self):
        url = "https://added.example/timeout"
        job = self.add(url)
        self.assertEqual(("error", "timeout"), (self.values(job)["content"], job["stages"]["content"]["reason"]))
        self.assertEqual("unknown", self.values(job)["image"], "no image after a failed text")
        intake = (self.root / "pages/added.jsonl").read_bytes()
        job = self.add(url, retry=["content"])
        self.assertEqual(["add", "--json", "--retry", "content"], self.argv())
        self.assertEqual({"library": "known", "content": "ok", "image": "ok", "search": "indexed"}, self.values(job))
        self.assertTrue((self.root / "pages/added.jsonl").read_bytes().startswith(intake), "no new address line")
        self.assertTrue(self.app.lib.records[job["page"]["row"]]["text_ok"])

    def test_chrome_out_of_reach_retries_the_text_signed_in(self):
        for name, status in (("offline", "off"), ("asleep", "not_running"), ("refused", "not_allowed")):
            with self.subTest(status=status):
                url = f"https://added.example/{name}"
                self.add(url)
                text = self.logs()["content.jsonl"]
                job = self.add(url, retry=["content"], signed_in=True)
                self.assertEqual((status, "signed_in"), (self.values(job)["content"], job["stages"]["content"]["tier"]))
                self.assertEqual(text, self.logs()["content.jsonl"], "nothing recorded")
                job = self.add(url, retry=["content"], signed_in=True)
                self.assertEqual(["add", "--json", "--retry", "content", "--signed-in"], self.argv())
                self.assertEqual(("ok", "signed_in"), (self.values(job)["content"], job["stages"]["content"]["tier"]))

    def test_signed_in_retry_uses_thin_403_and_retries_image_when_chrome_is_off(self):
        for name in ("thin403", "offline"):
            with self.subTest(name=name):
                url = f"https://added.example/{name}"
                if name == "thin403":
                    self.add(url)
                    fake_knowmoretabs.record(
                        self.root,
                        "content.jsonl",
                        url,
                        fake_knowmoretabs.text("thin", "headless", 200, "short text; not rendered: HTTP 403"),
                    )
                else:
                    self.add(url)
                fake_knowmoretabs.record(self.root, "images.jsonl", url, {"status": "error"})
                job = self.add(url, retry=["content", "image"], signed_in=True)
                self.assertEqual("ok" if name == "thin403" else "off", self.values(job)["content"])
                self.assertEqual("ok", self.values(job)["image"], "image runs even without Chrome")
                self.assertEqual(
                    ["add", "--json", "--retry", "content", "--retry", "image", "--signed-in"], self.argv()
                )

    def test_an_image_retry_leaves_the_text_alone(self):
        url = "https://added.example/imagefail"
        job = self.add(url)
        self.assertEqual(("ok", "error"), (self.values(job)["content"], self.values(job)["image"]))
        text = self.logs()["content.jsonl"]
        job = self.add(url, retry=["image"])
        self.assertEqual(["add", "--json", "--retry", "image"], self.argv())
        self.assertEqual(("ok", "ok", "indexed"), tuple(self.values(job)[k] for k in ("content", "image", "search")))
        self.assertEqual(text, self.logs()["content.jsonl"], "a kept text is never read again")
        self.assertTrue(self.app.lib.records[job["page"]["row"]]["image_ok"])

    def test_both_failed_stages_retry_in_one_run(self):
        url = "https://added.example/bothfail"
        job = self.add(url)
        self.assertEqual(("error", "error"), (self.values(job)["content"], self.values(job)["image"]))
        runs = len(self.log.read_text().splitlines())
        job = self.add(url, retry=["image", "content"])
        self.assertEqual(runs + 1, len(self.log.read_text().splitlines()), "one run")
        self.assertEqual(["add", "--json", "--retry", "content", "--retry", "image"], self.argv())
        self.assertEqual(("ok", "ok"), (self.values(job)["content"], self.values(job)["image"]))

    def test_a_third_failed_run_is_unavailable_and_stands(self):
        url = "https://added.example/stuck"
        ended = [self.values(self.add(url))["content"]]
        for _ in range(3):
            ended.append(self.values(self.add(url, retry=["content"]))["content"])
            self.assertEqual(["add", "--json", "--retry", "content"], self.argv())
        self.assertEqual(["error", "error", "unavailable", "unavailable"], ended)
        image = fake_knowmoretabs.latest(self.root, "images.jsonl", url)
        self.assertEqual("none", image["status"], "an unavailable text settles the image with no request")

    def test_nothing_recorded_is_unknown(self):
        job = self.add("https://added.example/video")
        self.assertEqual(("unknown", "not_recorded"), (self.values(job)["content"], job["stages"]["content"]["reason"]))
        self.assertEqual(("unknown", "indexed"), (self.values(job)["image"], self.values(job)["search"]))

    def test_a_retry_never_adds_a_page(self):
        job = self.add(NEW, retry=["content"])
        self.assertEqual({"library": "refused"}, self.values(job))
        self.assertEqual(
            ("not_in_library", False, None), (job["stages"]["library"]["reason"], job["failed"], job["page"])
        )
        self.assertFalse((self.root / "pages/added.jsonl").exists())
        self.assertEqual(len(self.lib.records), len(self.app.lib.records))

    def test_a_failure_after_restore_never_restores_again(self):
        url = "https://added.example/relapse"
        self.add(url)
        self.add(url, "forget")
        job = self.add(url, "restore")
        self.assertEqual((True, True, {"library": "running"}), (job["failed"], job["restored"], self.values(job)))
        self.assertNotIn(url, json.loads((self.root / "library.json").read_text())["forgotten"], "the restore held")
        job = self.add(url)
        self.assertEqual(("known", "indexed"), (self.values(job)["library"], self.values(job)["search"]))
        self.assertEqual(["restore", "add", "add"], [self.argv(n)[0] for n in (-3, -2, -1)])

    def test_blocked_then_signed_in(self):
        url = "https://added.example/blocked"
        job = self.add(url)
        self.assertEqual(
            {"library": "added", "content": "blocked", "image": "none", "search": "indexed"}, self.values(job)
        )
        self.assertEqual(403, job["stages"]["content"]["http_status"])
        job = self.add(url, retry=["content"], signed_in=True)
        self.assertEqual(
            ("known", "ok", "signed_in"),
            (self.values(job)["library"], self.values(job)["content"], job["stages"]["content"]["tier"]),
        )
        self.assertIn("--signed-in", json.loads(self.log.read_text().splitlines()[-1])["argv"])
        self.assertTrue(self.app.lib.records[job["page"]["row"]]["text_ok"], "the indexed page points at its new text")

    def test_signed_in_content_becomes_searchable_without_reembedding(self):
        url = "https://added.example/blocked"
        job = self.add(url)
        row = job["page"]["row"]
        vector = self.app.lib.X[row].copy()
        with patch.object(self.app, "embed", side_effect=AssertionError("an existing vector stays cached")):
            job = self.add(url, retry=["content"], signed_in=True)
        self.assertEqual("indexed", self.values(job)["search"])
        hits = self.call(self.port, "POST", "/api/search", {"query": "airship", "n": 50})[1]["hits"]
        hit = next(h for h in hits if h["row"] == row)
        self.assertGreater(hit["sources"]["keyword"]["score"], 0)
        np.testing.assert_array_equal(vector, self.app.lib.X[row])

    def test_not_found_then_remove_then_restore(self):
        url = "https://added.example/missing"
        job = self.add(url)
        self.assertEqual(("not_found", 404), (self.values(job)["content"], job["stages"]["content"]["http_status"]))
        row = job["page"]["row"]
        job = self.add(url, "forget")
        self.assertEqual(({"library": "forgotten"}, None), (self.values(job), job["page"]))
        self.assertFalse(self.app.live()[row])
        self.assertEqual({"library": "forgotten"}, self.values(self.add(url)))
        job = self.add(url, "restore")
        self.assertEqual(
            ("known", "indexed", row), (self.values(job)["library"], self.values(job)["search"], job["page"]["row"])
        )
        self.assertTrue(self.app.live()[row])

    def test_an_index_failure_is_not_indexed(self):
        with patch.object(self.app, "embed", side_effect=RuntimeError("no model")):
            job = self.add(NEW)
        self.assertEqual(
            ("added", "not_indexed", None), (self.values(job)["library"], self.values(job)["search"], job["page"])
        )
        self.assertEqual("indexed", self.values(self.add(NEW))["search"], "Retry indexes it")

    def test_an_index_retry_runs_only_the_lab_without_another_archive_command(self):
        with patch.object(self.app, "embed", side_effect=RuntimeError("no model")):
            job = self.add(NEW)
        self.assertEqual("not_indexed", self.values(job)["search"])
        commands = self.log.read_bytes()
        with patch("tagger.app.add.subprocess.Popen") as popen:
            job = self.add(NEW, "index")
            popen.assert_not_called()
        self.assertEqual("indexed", self.values(job)["search"])
        self.assertEqual(NEW, job["page"]["url"])
        self.assertEqual(commands, self.log.read_bytes())

    def test_a_missing_binary_fails_the_job(self):
        self.app.adds.binary = str(self.work / "absent")
        job = self.add(NEW)
        self.assertTrue(job["failed"])
        self.assertEqual({"library": "running"}, self.values(job))

    def test_an_archive_read_failure_finishes_remove_as_failed(self):
        url = self.lib.records[3]["key"]
        with patch("tagger.app.add.Archive.load", side_effect=OSError("unreadable archive")):
            job = self.add(url, "forget")
        self.assertTrue(job["finished"] and job["failed"])

    def test_poll_results_do_not_change_when_the_worker_reports_another_stage(self):
        job = add.Job(99, NEW, "add")
        job.take('{"stage":"library","state":"running"}')
        with self.app.adds.lock:
            self.app.adds.jobs[job.id] = job
        view = self.app.adds.view(job.id)
        with self.app.adds.lock:
            job.take('{"stage":"library","state":"done","value":"added"}')
            job.take('{"stage":"content","state":"running","tier":"web"}')
        self.assertEqual({"library": {"stage": "library", "state": "running"}}, view["stages"])

    def test_start_and_poll_never_wait_on_the_model(self):
        with patch.dict(os.environ, {"FAKE_KMT_DELAY": "0.05"}), self.app.lock:
            start = time.perf_counter()
            status, job = self.call(self.port, "POST", "/api/add", {"url": NEW})
            self.assertEqual(200, self.call(self.port, "GET", f"/api/add/{job['id']}")[0])
            self.assertLess(time.perf_counter() - start, 1)
            for _ in range(200):
                job = self.call(self.port, "GET", f"/api/add/{job['id']}")[1]
                if job["stages"].get("search"):
                    break
                time.sleep(0.025)
            self.assertEqual("running", job["stages"]["search"]["state"], "Search waits for the model's lock")
        self.assertEqual("indexed", self.values(self.add(NEW))["search"])

    def test_requests_are_checked(self):
        self.assertEqual(400, self.call(self.port, "POST", "/api/add", {})[0])
        self.assertEqual(400, self.call(self.port, "POST", "/api/add", {"url": NEW, "action": "rm"})[0])
        for bad in (
            {"retry": ["text"]},
            {"retry": "content"},
            {"retry": ["image", "image"]},
            {"retry": ["content"], "title": "One"},
            {"signed_in": "yes"},
            {"title": 3},
            {"title": "x" * (add.MAX + 1)},
            {"action": "forget", "retry": ["content"]},
            {"action": "restore", "signed_in": True},
            {"action": "index", "title": "One"},
        ):
            with self.subTest(**bad):
                self.assertEqual(400, self.call(self.port, "POST", "/api/add", {"url": NEW, **bad})[0])
        self.assertEqual(404, self.call(self.port, "GET", "/api/add/99")[0])


class FakeTests(unittest.TestCase):
    """The fake keeps the contract's usage errors: a Retry takes no title and never skips its content."""

    def test_a_retry_with_a_title_or_without_content_is_a_usage_error(self):
        with tempfile.TemporaryDirectory() as work:
            binary = fake_binary(Path(work))
            for flags in (
                ["--retry", "content", "--title=T"],
                ["--retry", "content", "--no-content"],
                ["--retry=text"],
            ):
                with self.subTest(flags=flags):
                    out = subprocess.run(
                        [binary, "--root", work, "add", "--json", *flags, "--", NEW], capture_output=True, text=True
                    )
                    self.assertEqual((2, ""), (out.returncode, out.stdout))


class SnapshotWriteTests(Served):
    def test_snapshot_roots_refuse_every_archive_action_without_starting_a_process(self):
        data = self.work / "lab-data"
        protected = data / "snapshot-synthetic"
        shutil.copytree(self.root, protected)
        original = {p.relative_to(protected): p.read_bytes() for p in protected.rglob("*") if p.is_file()}
        lib, _ = engine.load(self.paths, protected, fake_encode, fake_embed)
        alias = self.work / "snapshot-alias"
        alias.symlink_to(protected, target_is_directory=True)
        for root in (protected, alias, data):
            app, port = self.serve(replace(lib, root=root))
            with patch("tagger.app.add.subprocess.Popen") as popen, app.lock:
                for action in add.ACTIONS:
                    with self.subTest(root=root.name, action=action):
                        status, job = self.call(port, "POST", "/api/add", {"url": NEW, "action": action})
                        self.assertEqual(200, status)
                        self.assertTrue(job["finished"])
                        self.assertFalse(job["failed"])
                        self.assertEqual(
                            {
                                "library": {
                                    "stage": "library",
                                    "state": "done",
                                    "value": "refused",
                                    "reason": "snapshot",
                                }
                            },
                            job["stages"],
                        )
                        self.assertIsNone(job["page"])
                        self.assertEqual(job["stages"], self.call(port, "GET", f"/api/add/{job['id']}")[1]["stages"])
                popen.assert_not_called()
        self.assertEqual(
            original, {p.relative_to(protected): p.read_bytes() for p in protected.rglob("*") if p.is_file()}
        )


if __name__ == "__main__":
    unittest.main()
