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

from tagger import cli, dataset
from tagger.app import engine, server
from tagger.app.store import SOURCE, Exclusions, Store, tag_name
from tagger.heads import unit
from tagger.paths import Paths

TOPICS = list(synthetic_archive.TOPICS)
DIM = len(TOPICS) + 4


class CliTests(unittest.TestCase):
    def test_app_model_load_cannot_contact_the_hub(self):
        def cached_only(*args, **kwargs):
            self.assertTrue(kwargs.get("local_files_only"))
            raise SystemExit

        with patch("sentence_transformers.SentenceTransformer", side_effect=cached_only), self.assertRaises(SystemExit):
            server.run(Paths(Path(tempfile.gettempdir())), None, 0, True)

    def test_app_forces_offline_before_loading_the_model(self):
        old_umask = os.umask(0o077)
        self.addCleanup(os.umask, old_umask)
        with (
            patch.dict(os.environ, {"HF_HUB_OFFLINE": "0", "HF_HUB_DISABLE_TELEMETRY": "0"}),
            patch("sys.argv", ["tagger", "--data", tempfile.gettempdir(), "app", "--smoke"]),
            patch("tagger.app.server.run") as run,
        ):
            cli.main()
            run.assert_called_once()
            self.assertEqual("1", os.environ["HF_HUB_OFFLINE"])
            self.assertEqual("1", os.environ["HF_HUB_DISABLE_TELEMETRY"])


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
        self.assertEqual(int(self.lib.Y.any(axis=1).sum()), self.stats["labelled_pages"])
        np.testing.assert_array_equal(self.lib.rules.fitted.rows, np.arange(self.stats["labelled_pages"]))

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
            self.assertAlmostEqual(
                h["fused"], sum(1 / (engine.RRF_K + s["rank"]) for s in h["sources"].values() if s["rank"])
            )
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

    def test_a_created_tag_is_a_zero_shot_tag_queried_by_its_name(self):
        lib = engine.add_tags(self.lib, ["Sleeper cars"], fake_encode)
        self.assertEqual(len(self.lib.tags), len(lib.tags) - 1)  # the loaded library is left as it was
        j = lib.tags.index("Sleeper cars")
        s = lib.X @ fake_encode(["Sleeper cars"])[0]
        np.testing.assert_allclose(s, lib.S[:, j], rtol=1e-6)
        self.assertAlmostEqual(s.mean() + engine.ZERO_K_SD * s.std(), lib.threshold[j], places=5)
        unseen = self.lib.tags.index("Woodworking")  # an owner tag nobody holds: the rule `load` gives it
        self.assertFalse(self.lib.Y[:, unseen].any())
        u = self.lib.S[:, unseen]
        self.assertAlmostEqual(u.mean() + engine.ZERO_K_SD * u.std(), self.lib.threshold[unseen], places=5)
        self.assertEqual({"name": "Sleeper cars", "positives": 0, "source": "zero shot"}, engine.tag_info(lib)[j])
        rows = list(range(len(lib.records)))
        sugg = [p[0] for p in engine.prechecks(lib, rows, ["Sleeper cars"])]
        self.assertEqual({"zero shot"}, {x["source"] for x in sugg})
        self.assertEqual([bool(x) for x in s >= lib.threshold[j]], [x["checked"] for x in sugg])
        self.assertIn("Sleeper cars", {x["tag"] for x in engine.suggest(lib, rows, len(lib.tags))})

    def test_add_tags_skips_names_the_library_has_in_any_case(self):
        self.assertIs(self.lib, engine.add_tags(self.lib, ["coffee", "READ LATER"], fake_encode))
        lib = engine.add_tags(self.lib, ["New", "NEW", "trains"], fake_encode)
        self.assertEqual([*self.lib.tags, "New"], lib.tags)
        self.assertEqual("Coffee", engine.find_tag(lib, "cOFFEE"))

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
        answers = [json.loads(line) for line in Path(again.export()["answers"]).read_text().splitlines()]
        self.assertEqual([{"url": "u1", "tags": ["B"], "source": SOURCE}], answers)

    def test_tag_names_follow_the_import_rules(self):
        self.assertEqual("Model context protocol", tag_name("  Model \t context\nprotocol "))
        self.assertEqual("x" * 40, tag_name("x" * 40))
        self.assertEqual("研究", tag_name("研究"))
        self.assertEqual("a b", tag_name("a\u00a0\u3000b"))
        for bad, reason in [
            ("", "empty"),
            ("   ", "empty"),
            ("x" * 41, "longer than 40"),
            ("a\u0007b", "control"),
            ("\u0000", "control"),
            ("a\u001cb", "control"),  # whitespace to str.split, a control character to Rust
        ]:
            with self.assertRaisesRegex(ValueError, f"cannot use that name as a tag: it .*{reason}"):
                tag_name(bad)

    def test_created_tags_persist_privately_and_export_with_their_decisions(self):
        store = Store(self.tmp)
        store.add_tag("Sleeper cars")
        again = Store(self.tmp)
        self.assertEqual(["Sleeper cars"], again.tags())
        self.assertEqual(0o600, stat.S_IMODE((self.tmp / "state.json").stat().st_mode))
        s = again.create("q", False, ["u1"], ["Sleeper cars"], [self.sugg(("Sleeper cars", False))])
        again.update(s["id"], 0, {"Sleeper cars": True}, "decided")
        answers = [json.loads(line) for line in Path(again.export()["answers"]).read_text().splitlines()]
        self.assertEqual([{"url": "u1", "tags": ["Sleeper cars"], "source": SOURCE}], answers)

    def test_marks_name_only_offered_tags(self):
        store = Store(self.tmp)
        s = store.create("q", False, ["u1"], ["A"], [self.sugg(("A", True))])
        with self.assertRaises(ValueError):
            store.update(s["id"], 0, {"Z": True}, None)
        with self.assertRaises(ValueError):
            store.update(s["id"], 0, None, "maybe")

    def test_invalid_status_does_not_change_marks_in_memory(self):
        store = Store(self.tmp)
        s = store.create("q", False, ["u1"], ["A"], [self.sugg(("A", True))])
        with self.assertRaises(ValueError):
            store.update(s["id"], 0, {"A": False}, "maybe")
        self.assertTrue(store.session(s["id"])["pages"][0]["marks"]["A"])
        self.assertEqual(Store(self.tmp).state, store.state)

    def test_undo_reject_restores_flips_even_after_restart(self):
        store = Store(self.tmp)
        s = store.create("q", False, ["u1"], ["A", "B"], [self.sugg(("A", True), ("B", True))])
        store.update(s["id"], 0, {"B": False}, None)
        store.update(s["id"], 0, {"A": False, "B": False}, "decided")
        again = Store(self.tmp)
        page = again.update(s["id"], 0, None, "open")
        self.assertEqual({"A": True, "B": False}, page["marks"])
        self.assertEqual({}, again.decisions())

    def test_editing_an_older_decided_set_is_the_latest_decision(self):
        store = Store(self.tmp)
        sets = [store.create("q", False, ["u1"], ["A"], [self.sugg(("A", True))]) for _ in range(2)]
        store.update(sets[0]["id"], 0, None, "decided")
        store.update(sets[1]["id"], 0, None, "decided")
        store.update(sets[0]["id"], 0, {"A": False}, None)
        self.assertFalse(Store(self.tmp).decisions()[("u1", "A")]["value"])

    def test_applying_to_a_partly_tagged_selection_tags_only_the_rest(self):
        store = Store(self.tmp)
        self.assertEqual(["u1", "u2"], store.apply(["u1", "u2"], "A", True))
        self.assertEqual(["u3"], store.apply(["u1", "u2", "u3"], "A", True))
        self.assertEqual([], store.apply(["u1", "u2", "u3"], "A", True))  # applying twice writes nothing
        self.assertEqual([], store.apply(["u4"], "A", False))  # nor does removing a tag a page lacks
        self.assertEqual(3, len(store.state["direct"]))
        self.assertEqual(["u2"], store.apply(["u2", "u4"], "A", False))
        self.assertEqual({"u1": ["A"], "u3": ["A"]}, Store(self.tmp).app_tags())

    def test_export_carries_direct_tags_and_a_direct_removal_overrides_a_review(self):
        store = Store(self.tmp)
        s = store.create("q", False, ["u1", "u2"], ["A"], [self.sugg(("A", False))] * 2)
        store.update(s["id"], 0, {"A": True}, "decided")
        store.update(s["id"], 1, {"A": True}, "decided")
        self.assertEqual({"u1": ["A"], "u2": ["A"]}, store.app_tags())  # kept in a review
        store.apply(["u2"], "A", False)
        store.apply(["u3"], "B", True)
        out = Store(self.tmp).export()
        answers = [json.loads(line) for line in Path(out["answers"]).read_text().splitlines()]
        expected = [{"url": "u1", "tags": ["A"], "source": SOURCE}, {"url": "u3", "tags": ["B"], "source": SOURCE}]
        self.assertEqual(expected, answers)
        log = {(r["url"], r["tag"]): r for r in map(json.loads, Path(out["decisions"]).read_text().splitlines())}
        self.assertEqual(("no", None, None), tuple(log[("u2", "A")][k] for k in ("answer", "model", "session")))
        self.assertEqual({"url", "tag", "answer", "model", "session", "at"}, set(log[("u3", "B")]))
        self.assertEqual((3, 1), (out["decided"], out["flipped"]))  # direct decisions have no model to flip

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


class ExclusionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.old = os.umask(0o077)
        self.addCleanup(os.umask, self.old)
        self.keys = [f"u{i}" for i in range(6)]

    def test_a_click_excludes_one_result_and_logs_its_rank_privately_apart_from_decisions(self):
        ex = Exclusions(self.tmp)
        ex.apply("q", self.keys, "exclude", "u2")
        ex.apply("other", self.keys, "exclude", "u0")
        again = Exclusions(self.tmp)
        self.assertEqual(({"u2"}, None), again.view("q", self.keys))
        entry = again.state["queries"]["q"]["excluded"]["u2"]
        self.assertEqual((3, "click"), (entry["rank"], entry["by"]))
        self.assertEqual({"u0"}, again.keys("other"))
        self.assertEqual(0o600, stat.S_IMODE(again.path.stat().st_mode))
        self.assertFalse((self.tmp / "state.json").exists())
        again.apply("q", self.keys, "include", "u2")
        self.assertEqual(set(), Exclusions(self.tmp).keys("q"))
        self.assertNotIn("q", Exclusions(self.tmp).state["queries"])

    def test_a_cut_excludes_everything_below_and_undoing_it_keeps_clicks(self):
        ex = Exclusions(self.tmp)
        ex.apply("q", self.keys, "exclude", "u5")
        ex.apply("q", self.keys, "exclude", "u0")
        ex.apply("q", self.keys, "cut", "u2")
        self.assertEqual(({"u0", "u3", "u4", "u5"}, "u2"), Exclusions(self.tmp).view("q", self.keys))
        self.assertEqual("click", ex.state["queries"]["q"]["excluded"]["u5"]["by"])
        ex.apply("q", self.keys, "cut", "u3")  # a new cut replaces the old one
        self.assertEqual(({"u0", "u4", "u5"}, "u3"), ex.view("q", self.keys))
        ex.apply("q", self.keys, "uncut", None)
        self.assertEqual(({"u0", "u5"}, None), Exclusions(self.tmp).view("q", self.keys))

    def test_a_cut_covers_results_shown_later_but_not_ones_brought_back(self):
        ex = Exclusions(self.tmp)
        ex.apply("q", self.keys[:4], "cut", "u1")
        ex.apply("q", self.keys[:4], "include", "u3")
        self.assertEqual(({"u2", "u4", "u5"}, "u1"), ex.view("q", self.keys))
        self.assertEqual(5, Exclusions(self.tmp).state["queries"]["q"]["excluded"]["u4"]["rank"])

    def test_a_cut_covers_new_results_a_reordered_list_puts_below_it(self):
        ex = Exclusions(self.tmp)
        ex.apply("q", self.keys[:4], "cut", "u1")
        ex.apply("q", self.keys[:4], "include", "u3")
        reordered = ["u0", "u1", "u5", "u3", "u2"]  # refine or images: a new result below the cut, u3 brought back
        self.assertEqual(({"u2", "u5"}, "u1"), ex.view("q", reordered))

    def test_queries_match_only_as_typed(self):
        ex = Exclusions(self.tmp)
        ex.apply("classifier", self.keys, "cut", "u0")
        for other in ("classifiers", "Classifier", "classifier ", "a classifier"):
            self.assertEqual((set(), None), ex.view(other, self.keys))
            self.assertEqual(set(), ex.keys(other))

    def test_actions_name_a_shown_result(self):
        ex = Exclusions(self.tmp)
        with self.assertRaises(ValueError):
            ex.apply("q", self.keys, "exclude", "elsewhere")
        with self.assertRaises(ValueError):
            ex.apply("q", self.keys, "forget", "u1")


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

    def test_a_new_tag_is_made_once_then_reviewed_exported_and_kept_over_a_restart(self):
        status, made, _ = self.call("POST", "/api/tags", {"name": "  Sleeper   cars "})
        self.assertEqual((200, "Sleeper cars", True), (status, made["tag"], made["created"]))
        self.assertIn({"name": "Sleeper cars", "positives": 0, "source": "zero shot"}, made["tags"])
        for same, spelling in (("sleeper CARS", "Sleeper cars"), ("coffee", "Coffee")):
            status, again, _ = self.call("POST", "/api/tags", {"name": same})
            self.assertEqual((200, spelling, False), (status, again["tag"], again["created"]))
            self.assertEqual(made["tags"], again["tags"])
        for bad in ("  ", "x" * 41, "a\u0007b"):
            status, err, _ = self.call("POST", "/api/tags", {"name": bad})
            self.assertEqual(400, status)
            self.assertRegex(err["error"], "^cannot use that name as a tag: it ")
        self.assertEqual(["Sleeper cars"], Store(self.data).tags())
        rows = [h["row"] for h in self.call("POST", "/api/search", {"query": "night train"})[1]["hits"]]
        s = self.call("POST", "/api/sessions", {"query": "night train", "rows": rows, "picked": ["Sleeper cars"]})[1]
        page = s["pages"][0]
        self.assertEqual([("Sleeper cars", "zero shot")], [(x["tag"], x["source"]) for x in page["sugg"]])
        path = f"/api/sessions/{s['id']}/pages/{page['index']}"
        self.call("POST", path, {"marks": {"Sleeper cars": True}, "status": "decided"})
        out = self.call("POST", "/api/export", {})[1]
        answers = [json.loads(line) for line in Path(out["answers"]).read_text().splitlines()]
        self.assertEqual([{"url": page["url"], "tags": ["Sleeper cars"], "source": SOURCE}], answers)
        restarted = server.App(self.lib, Store(self.data), fake_encode)
        self.assertEqual(made["tags"], restarted.library()["tags"])

    def test_excluded_results_stay_out_of_suggestions_review_and_export(self):
        found = self.call("POST", "/api/search", {"query": "night train", "n": 20})[1]
        rows = [h["row"] for h in found["hits"]]
        self.assertEqual(([], None), (found["excluded"], found["cut"]))
        judge = {"query": "night train", "rows": rows}
        status, after, _ = self.call("POST", "/api/exclusions", judge | {"action": "exclude", "row": rows[0]})
        self.assertEqual((200, [rows[0]]), (status, after["excluded"]))
        after = self.call("POST", "/api/exclusions", judge | {"action": "cut", "row": rows[9]})[1]
        self.assertEqual([rows[0], *rows[10:]], after["excluded"])
        self.assertEqual(rows[9], after["cut"])
        kept = rows[1:10]
        self.assertEqual(engine.suggest(self.lib, kept), after["suggested"])
        self.assertNotEqual(engine.suggest(self.lib, rows), after["suggested"])
        again = self.call("POST", "/api/search", {"query": "night train", "n": 20})[1]  # a reload or restart
        self.assertEqual((after["excluded"], rows[9]), (again["excluded"], again["cut"]))
        self.assertEqual(400, self.call("POST", "/api/exclusions", judge | {"action": "exclude", "row": -1})[0])
        s = self.call("POST", "/api/sessions", {"query": "night train", "rows": rows, "picked": ["Trains"]})[1]
        keys = {self.lib.records[r]["key"] for r in kept}
        self.assertEqual(keys, {p["url"] for p in s["pages"]})
        for p in s["pages"]:
            if p["sugg"]:
                self.call("POST", f"/api/sessions/{s['id']}/pages/{p['index']}", {"status": "decided"})
        out = self.call("POST", "/api/export", {})[1]
        exported = Path(out["answers"]).read_text() + Path(out["decisions"]).read_text()
        self.assertTrue(all(json.loads(line)["url"] in keys for line in exported.splitlines()))
        self.assertTrue((self.data / "not-relevant.json").exists())
        self.assertEqual(
            400,
            self.call("POST", "/api/sessions", {"query": "night train", "rows": rows[10:], "picked": ["Trains"]})[0],
        )

    def test_a_page_excluded_in_one_search_is_untouched_in_another(self):
        other = {"query": "sleeper trains", "n": 20}
        before = self.call("POST", "/api/search", other)[1]
        refined = self.call("POST", "/api/search", other | {"refine": True})[1]
        rows = [h["row"] for h in self.call("POST", "/api/search", {"query": "night train", "n": 20})[1]["hits"]]
        shared = next(h["row"] for h in before["hits"] if h["row"] in rows)
        self.call("POST", "/api/exclusions", {"query": "night train", "rows": rows, "action": "cut", "row": rows[0]})
        self.call(
            "POST", "/api/exclusions", {"query": "night train", "rows": rows, "action": "exclude", "row": rows[0]}
        )
        after = self.call("POST", "/api/search", other)[1]
        self.assertEqual(before, after | {"ms": before["ms"]})  # same hits, ranks, suggestions; nothing excluded
        self.assertEqual([], after["excluded"])
        again = self.call("POST", "/api/search", other | {"refine": True})[1]
        self.assertEqual(refined["hits"], again["hits"])
        s = self.call(
            "POST", "/api/sessions", {"query": "sleeper trains", "rows": [shared], "picked": ["Trains", "Coffee"]}
        )
        self.assertEqual(200, s[0])
        self.assertEqual([self.lib.records[shared]["key"]], [p["url"] for p in s[1]["pages"]])
        self.assertEqual({}, Store(self.data).decisions())  # an exclusion is not a tag rejection

    def test_refine_ranks_away_from_excluded_pages_and_keeps_them_excluded(self):
        plain = self.call("POST", "/api/search", {"query": "night train", "n": 20})[1]
        rows = [h["row"] for h in plain["hits"]]
        judge = {"query": "night train", "rows": rows}
        for r in rows[:5]:
            self.call("POST", "/api/exclusions", judge | {"action": "exclude", "row": r})
        refined = self.call("POST", "/api/search", {"query": "night train", "n": 20, "refine": True})[1]
        new = [h["row"] for h in refined["hits"]]
        self.assertNotEqual(rows, new)
        self.assertEqual(sorted(set(rows[:5]) & set(new)), sorted(refined["excluded"]))
        self.assertTrue(set(new) - set(rows))

    def test_search_uses_the_exact_query_for_exclusions(self):
        query = "night train"
        rows = [h["row"] for h in self.call("POST", "/api/search", {"query": query})[1]["hits"]]
        self.call("POST", "/api/exclusions", {"query": query, "rows": rows, "action": "exclude", "row": rows[0]})
        for other in (" night train", "night train ", "Night train", "night  train"):
            found = self.call("POST", "/api/search", {"query": other})[1]
            self.assertEqual([], found["excluded"])
        padded = " night train "
        self.call("POST", "/api/exclusions", {"query": padded, "rows": rows, "action": "exclude", "row": rows[1]})
        self.assertEqual([rows[1]], self.call("POST", "/api/search", {"query": padded})[1]["excluded"])

    def test_refine_applies_the_cut_before_using_new_results_as_positives(self):
        app = server.App(self.lib, Store(self.data), fake_encode)
        query = "night train"
        q = fake_encode([query])[0]
        hits = engine.search(self.lib, q, query, 50)
        keys = [self.lib.records[h["row"]]["key"] for h in hits]
        app.exclusions.apply(query, keys[:20], "cut", keys[9])
        with patch("tagger.app.server.prototype", wraps=server.prototype) as prototype:
            app.refined(query, q, hits)
        self.assertEqual(10, len(prototype.call_args.args[1]))
        self.assertEqual(40, len(prototype.call_args.args[2]))

    def rows_for(self, query: str, n: int = 20, **body) -> list[int]:
        return [h["row"] for h in self.call("POST", "/api/search", {"query": query, "n": n, **body})[1]["hits"]]

    def test_a_tag_applies_to_and_comes_off_the_selected_pages(self):
        rows = self.rows_for("night train")[:3]
        status, out, _ = self.call("POST", "/api/apply", {"rows": rows, "tag": "Trains", "value": True})
        self.assertEqual((200, 3), (status, out["changed"]))
        self.assertEqual([{"row": r, "tags": ["Trains"]} for r in rows], out["pages"])
        self.assertEqual(["Trains"], out["app_tags"])
        out = self.call("POST", "/api/apply", {"rows": rows[:1], "tag": "Trains", "value": False})[1]
        self.assertEqual(([{"row": rows[0], "tags": []}], 1), (out["pages"], out["changed"]))
        found = {h["row"]: h["tags"] for h in self.call("POST", "/api/search", {"query": "night train"})[1]["hits"]}
        self.assertEqual([[], ["Trains"], ["Trains"]], [found[r] for r in rows])
        for bad in ({"tag": "Nope", "value": True}, {"tag": "Trains", "value": "yes"}, {"tag": "Trains"}):
            self.assertEqual(400, self.call("POST", "/api/apply", {"rows": rows, **bad})[0])
        self.assertEqual(400, self.call("POST", "/api/apply", {"rows": [], "tag": "Trains", "value": True})[0])

    def test_the_selection_persists_privately_in_order_until_cleared(self):
        self.assertEqual({"pages": []}, self.call("GET", "/api/selection")[1])
        rows = self.rows_for("espresso")[:2] + self.rows_for("night train")[:2]
        picked = [rows[3], rows[0], rows[2]]
        self.assertEqual(200, self.call("POST", "/api/selection", {"rows": picked})[0])
        restarted = server.App(self.lib, Store(self.data), fake_encode)
        self.assertEqual(picked, [p["row"] for p in restarted.selected()["pages"]])
        self.assertEqual(0o600, stat.S_IMODE((self.data / "selection.json").stat().st_mode))
        self.assertEqual(400, self.call("POST", "/api/selection", {"rows": [-1]})[0])
        self.assertEqual(picked, [p["row"] for p in self.call("GET", "/api/selection")[1]["pages"]])
        self.call("POST", "/api/selection", {"rows": []})
        self.assertEqual([], server.App(self.lib, Store(self.data), fake_encode).selected()["pages"])

    def test_untagged_only_leaves_out_pages_with_an_app_tag_and_ignores_archive_tags(self):
        rows = self.rows_for("night train")
        self.call("POST", "/api/apply", {"rows": rows[:5], "tag": "Trains", "value": True})
        untagged = self.rows_for("night train", untagged=True)
        self.assertEqual(20, len(untagged))
        self.assertFalse(set(rows[:5]) & set(untagged))
        self.assertEqual(set(rows[5:]), set(untagged[:15]))  # the rest, ranked again among untagged pages
        archive_tagged = [r for r in untagged if self.lib.Y[r].any()]
        self.assertTrue(archive_tagged)  # a page holding only archive tags is still untagged here

    def test_more_like_this_excludes_only_its_tag_near_the_tags_pages(self):
        self.call("POST", "/api/tags", {"name": "Sleeper cars"})
        trains = [r for r in self.rows_for("night train") if topic_of(self.lib.records[r]["key"]) == "trains"]
        self.call("POST", "/api/apply", {"rows": trains[:4], "tag": "Sleeper cars", "value": True})
        self.call("POST", "/api/apply", {"rows": trains[4:6], "tag": "Rust", "value": True})
        status, out, _ = self.call("POST", "/api/like", {"tag": "Sleeper cars", "n": 20})
        self.assertEqual((200, "Sleeper cars", 20), (status, out["tag"], len(out["hits"])))
        liked = [h["row"] for h in out["hits"]]
        self.assertFalse(set(trains[:4]) & set(liked))
        self.assertTrue(set(trains[4:6]) <= set(liked))  # a different app tag still belongs in this view
        self.assertTrue(all("Sleeper cars" not in h["tags"] for h in out["hits"]))
        self.assertEqual(["trains"] * 5, [topic_of(self.lib.records[r]["key"]) for r in liked[:5]])
        self.assertEqual(400, self.call("POST", "/api/like", {"tag": "Nope"})[0])

    def test_more_like_this_ranks_toward_the_tags_pages(self):
        for name in ("Zephyr", "Quill"):  # names no page or topic word shares: only the tag's pages can pull
            self.call("POST", "/api/tags", {"name": name})
        trains = [r for r in self.rows_for("night train") if topic_of(self.lib.records[r]["key"]) == "trains"]
        self.call("POST", "/api/apply", {"rows": trains[:6], "tag": "Zephyr", "value": True})

        def trains_in_top_10(tag: str) -> int:
            hits = self.call("POST", "/api/like", {"tag": tag, "n": 20})[1]["hits"][:10]
            return sum(topic_of(h["url"]) == "trains" for h in hits)

        self.assertGreaterEqual(trains_in_top_10("Zephyr"), trains_in_top_10("Quill") + 3)

    def test_refined_cut_ranking_is_stable_after_reload_and_restart(self):
        body = {"query": "night train ", "n": 50}
        rows = [h["row"] for h in self.call("POST", "/api/search", body)[1]["hits"]]
        judge = {"query": body["query"], "rows": rows}
        self.call("POST", "/api/exclusions", judge | {"action": "exclude", "row": rows[1]})
        self.call("POST", "/api/exclusions", judge | {"action": "cut", "row": rows[9]})
        first = self.call("POST", "/api/search", body | {"refine": True})[1]
        again = server.App(self.lib, Store(self.data), fake_encode).search(body | {"refine": True})
        self.assertEqual([h["row"] for h in first["hits"]], [h["row"] for h in again["hits"]])
        self.assertEqual(first["excluded"], again["excluded"])
        self.assertEqual(first["suggested"], again["suggested"])


if __name__ == "__main__":
    unittest.main()
