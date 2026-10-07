"""The app on the synthetic archive with a fake encoder (no model): load, search fusion, prechecks, the store's
persistence and export, and the HTTP guards and flow."""

import http.client
import json
import os
import shutil
import stat
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np
import synthetic_archive

from tagger import dataset
from tagger.app import engine, server
from tagger.app.store import SOURCE, Store
from tagger.heads import unit
from tagger.paths import Paths

TOPICS = list(synthetic_archive.TOPICS)
DIM = len(TOPICS) + 4


def topic_of(key: str) -> str:
    return key.split("/")[3]


def fake_encode(texts: list[str]) -> np.ndarray:
    """A topic axis for every topic named, by key or by one of its words; a shared offset otherwise."""
    out = np.zeros((len(texts), DIM), np.float32)
    for i, text in enumerate(texts):
        low = text.lower()
        for j, (topic, (tag, _, words, _)) in enumerate(synthetic_archive.TOPICS.items()):
            if topic in low or tag.lower() in low or any(w in low for w in words):
                out[i, j] = 1
        out[i, -1] = 0.2
    return unit(out)


def build(root: Path) -> Paths:
    """Archive, dataset and fake cached vectors (topic axis plus noise; images on the same axes)."""
    archive, paths = root / "archive", Paths(root / "data")
    synthetic_archive.build(archive, paths.data)
    with patch.dict(os.environ, {"KMT_TAGGER_SNAPSHOT": str(archive)}):
        dataset.run(paths)
    records, _ = dataset.load(paths)
    rng = np.random.default_rng(3)
    X = np.zeros((len(records), DIM), np.float32)
    for i, r in enumerate(records):
        X[i, TOPICS.index(topic_of(r["key"]))] = 1
    X = unit(X + 0.3 * rng.normal(size=X.shape).astype(np.float32))
    (paths.emb / "eg2").mkdir(parents=True)
    np.save(paths.emb / "eg2" / "B.npy", X)
    (paths.emb / "image").mkdir(parents=True)
    mask = np.array([r["image_ok"] for r in records])
    np.save(paths.emb / "image" / "eg2-full.npy", np.where(mask[:, None], X, 0))
    np.save(paths.emb / "image" / "eg2-full-mask.npy", mask)
    return paths


class Fixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp())
        cls.paths = build(cls.tmp)
        with patch("builtins.print"):
            cls.lib, cls.stats = engine.load(cls.paths, cls.tmp / "archive", fake_encode)

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp)


class EngineTests(Fixture):
    def test_heads_only_for_tags_with_ten_positives(self):
        positives = self.lib.Y.sum(axis=0)
        heads = set(self.lib.rules.heads.tolist())
        self.assertEqual({j for j in range(len(self.lib.tags)) if positives[j] >= 10}, heads)
        self.assertIn(0, positives)  # a tag nobody holds yet still gets a finite threshold
        self.assertTrue(np.isfinite(self.lib.threshold).all())
        self.assertEqual(self.stats["heads"], len(heads))

    def test_search_fuses_sources_and_skips_forgotten_pages(self):
        q = fake_encode(["espresso grinder"])[0]
        hits = engine.search(self.lib, q, "espresso grinder", 20)
        self.assertEqual(20, len(hits))
        self.assertEqual({"text", "keyword"}, set(hits[0]["sources"]))
        self.assertTrue(all(self.lib.live[h["row"]] for h in hits))
        self.assertTrue(all(topic_of(self.lib.records[h["row"]]["key"]) == "coffee" for h in hits[:10]))
        fused = [h["fused"] for h in hits]
        self.assertEqual(sorted(fused, reverse=True), fused)
        for h in hits:
            kw = h["sources"]["keyword"]
            self.assertEqual(kw["score"] > 0, kw["rank"] > 0)
        forgotten = [i for i, r in enumerate(self.lib.records) if r["forgotten"]]
        self.assertTrue(forgotten)
        every = engine.search(self.lib, q, "espresso", 10_000)
        self.assertEqual(int(self.lib.live.sum()), len(every))
        self.assertFalse({h["row"] for h in every} & set(forgotten))

    def test_images_add_a_source_ranking_only_pages_with_an_image(self):
        q = fake_encode(["night train"])[0]
        hits = engine.search(self.lib, q, "night train", 50, images=True)
        for h in hits:
            self.assertEqual(h["sources"]["image"]["rank"] > 0, bool(self.lib.has_image[h["row"]]))

    def test_suggestions_rank_the_result_sets_topic_first(self):
        q = fake_encode(["sourdough"])[0]
        rows = [h["row"] for h in engine.search(self.lib, q, "sourdough", 20)]
        self.assertEqual("Recipes", engine.suggest(self.lib, rows)[0]["tag"])

    def test_prechecks_offer_only_picked_tags_the_page_lacks(self):
        rows = list(range(len(self.lib.records)))
        picked = ["Coffee", "Read later"]
        for row, sugg in zip(rows, engine.prechecks(self.lib, rows, picked), strict=True):
            own = {t for j, t in enumerate(self.lib.tags) if self.lib.Y[row, j]}
            self.assertEqual(set(picked) - own, {s["tag"] for s in sugg})
            for s in sugg:
                j = self.lib.tags.index(s["tag"])
                self.assertEqual(s["checked"], bool(self.lib.S[row, j] >= self.lib.threshold[j]))
                self.assertEqual(s["checked"], s["p"] >= 0.5)

    def test_a_dataset_from_another_snapshot_is_refused(self):
        other = self.tmp / "other"
        shutil.copytree(self.tmp / "archive", other)
        shutil.rmtree(other / "snapshots")
        (other / "snapshots").mkdir()
        with self.assertRaises(SystemExit):
            engine.load(self.paths, other, fake_encode)


class StoreTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.old = os.umask(0o077)
        self.addCleanup(os.umask, self.old)

    @staticmethod
    def sugg(*pairs):
        return [{"tag": t, "checked": c, "p": 0.7 if c else 0.3, "source": "head"} for t, c in pairs]

    def test_decisions_survive_a_restart_and_the_latest_wins(self):
        store = Store(self.tmp)
        a = store.create(
            "q1", False, ["u1", "u2"], ["A", "B"], [self.sugg(("A", True), ("B", False)), self.sugg(("A", False))]
        )
        store.update(a["id"], 0, {"B": True}, "decided")
        store.update(a["id"], 1, None, "skipped")
        b = store.create("q2", True, ["u1"], ["A"], [self.sugg(("A", True))])
        store.update(b["id"], 0, {"A": False}, "decided")
        again = Store(self.tmp)
        self.assertEqual(again.state, store.state)
        latest = again.decisions()
        self.assertEqual({("u1", "A"), ("u1", "B")}, set(latest))  # the skipped page is not a decision
        self.assertFalse(latest[("u1", "A")]["value"])
        self.assertEqual(b["id"], latest[("u1", "A")]["session"])
        self.assertTrue(latest[("u1", "B")]["value"])

    def test_marks_name_only_offered_tags(self):
        store = Store(self.tmp)
        s = store.create("q", False, ["u1"], ["A"], [self.sugg(("A", True))])
        with self.assertRaises(ValueError):
            store.update(s["id"], 0, {"Z": True}, None)
        with self.assertRaises(ValueError):
            store.update(s["id"], 0, None, "maybe")

    def test_export_writes_private_answers_and_log_alone_in_a_folder(self):
        store = Store(self.tmp)
        s = store.create("q", False, ["u1", "u2", "u3"], ["A", "B"], [self.sugg(("A", True), ("B", True))] * 3)
        store.update(s["id"], 0, {"B": False}, "decided")
        store.update(s["id"], 1, {"A": False, "B": False}, "decided")
        out = store.export()
        answers = [json.loads(line) for line in Path(out["answers"]).read_text().splitlines()]
        self.assertEqual([{"url": "u1", "tags": ["A"], "source": SOURCE}], answers)
        log = [json.loads(line) for line in Path(out["decisions"]).read_text().splitlines()]
        self.assertEqual(4, len(log))
        self.assertEqual(3, sum(row["answer"] != row["model"] for row in log))
        self.assertEqual((4, 3, 1, 1), (out["decided"], out["flipped"], out["answer_pages"], out["answer_tags"]))
        folder = Path(out["folder"])
        self.assertEqual({"answers.jsonl", "decisions.jsonl"}, {p.name for p in folder.iterdir()})
        for p in [*folder.iterdir(), self.tmp / "state.json"]:
            self.assertEqual(0o600, stat.S_IMODE(p.stat().st_mode))
        self.assertNotEqual(folder, Path(store.export()["folder"]))


class ServerTests(Fixture):
    def setUp(self):
        self.data = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.data)
        self.server = server.serve(server.App(self.lib, Store(self.data), fake_encode), 0)
        self.port = self.server.server_address[1]
        thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)

    def call(self, method, path, body=None, headers=None):
        conn = http.client.HTTPConnection("127.0.0.1", self.port)
        sent = {"Host": f"127.0.0.1:{self.port}"}
        if body is not None:
            sent["Content-Type"] = "application/json"
        sent |= headers or {}
        conn.request(method, path, None if body is None else json.dumps(body), sent)
        r = conn.getresponse()
        data = r.read()
        conn.close()
        kind = r.getheader("Content-Type", "")
        return r.status, json.loads(data) if kind.startswith("application/json") else data, r

    def test_guards(self):
        self.assertEqual(421, self.call("GET", "/api/library", headers={"Host": "attacker.example"})[0])
        self.assertEqual(403, self.call("POST", "/api/export", {}, {"Origin": "http://attacker.example"})[0])
        self.assertEqual(415, self.call("POST", "/api/export", {}, {"Content-Type": "text/plain"})[0])
        self.assertEqual(404, self.call("GET", "/..%2Fserver.py")[0])
        self.assertEqual(404, self.call("GET", "/static/../server.py")[0])
        status, body, r = self.call("GET", "/")
        self.assertEqual(200, status)
        self.assertIn(b"knowmoretabs", body)
        self.assertIn("default-src 'none'", r.getheader("Content-Security-Policy"))
        missing = next(i for i, r in enumerate(self.lib.records) if not r["image_ok"])
        self.assertEqual(404, self.call("GET", f"/img/{missing}")[0])
        shown = next(i for i, r in enumerate(self.lib.records) if r["image_ok"])
        status, body, r = self.call("GET", f"/img/{shown}")
        self.assertEqual((200, "image/jpeg"), (status, r.getheader("Content-Type")))

    def test_search_pick_review_export(self):
        status, found, _ = self.call("POST", "/api/search", {"query": "night train", "n": 20})
        self.assertEqual(200, status)
        self.assertEqual(20, len(found["hits"]))
        self.assertEqual(400, self.call("POST", "/api/search", {"query": " "})[0])
        rows = [h["row"] for h in found["hits"]]
        self.assertEqual(400, self.call("POST", "/api/sessions", {"rows": rows, "picked": ["Nope"]})[0])
        status, s, _ = self.call("POST", "/api/sessions", {"query": "night train", "rows": rows, "picked": ["Trains"]})
        self.assertEqual(200, status)
        page = next(p for p in s["pages"] if p["sugg"])
        flipped = {page["sugg"][0]["tag"]: not page["sugg"][0]["checked"]}
        path = f"/api/sessions/{s['id']}/pages/{page['index']}"
        status, after, _ = self.call("POST", path, {"marks": flipped, "status": "decided"})
        self.assertEqual((200, "decided"), (status, after["status"]))
        self.assertEqual(404, self.call("POST", f"/api/sessions/{s['id']}/pages/999", {"status": "decided"})[0])
        listed = self.call("GET", "/api/sessions")[1]
        self.assertEqual(1, listed[0]["decided"])
        status, out, _ = self.call("POST", "/api/export", {})
        self.assertEqual((200, 1, 1), (status, out["decided"], out["flipped"]))


if __name__ == "__main__":
    unittest.main()
