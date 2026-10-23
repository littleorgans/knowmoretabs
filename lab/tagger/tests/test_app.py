"""The app on the synthetic archive with a fake encoder (no model): load, search fusion, prechecks, the decisions
followed from `library.json`, the app's own lists, and the HTTP guards and flow."""

import dataclasses
import json
import os
import shutil
import stat
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from unittest.mock import patch

import numpy as np
import synthetic_archive
import signed

from tagger import cli, dataset
from tagger.embed import CHAR_CAP, IMAGE_MODEL, MAX_TOKENS, MODELS, model_identity, page_texts
from tagger.app import engine, server
from tagger.app.store import Exclusions
from tagger.archive import sha256_hex
from tagger.heads import unit
from tagger.paths import Paths

TOPICS = list(synthetic_archive.TOPICS)
DIM = len(TOPICS) + 4


class CliTests(unittest.TestCase):
    def test_app_model_load_cannot_contact_the_hub(self):
        def cached_only(*args, **kwargs):
            self.assertTrue(kwargs.get("local_files_only"))
            raise SystemExit

        with (
            patch("tagger.app.engine.require_vectors"),
            patch("sentence_transformers.SentenceTransformer", side_effect=cached_only),
            self.assertRaises(SystemExit),
        ):
            server.run(Paths(Path(tempfile.gettempdir())), Path(tempfile.gettempdir()), 0, True)

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


def fake_embed(records: list[dict], name: str) -> np.ndarray:
    """Page vectors on the same topic axes, from each formatted input including its body."""
    return fake_encode([f"{title}\n{text}" for title, text in page_texts(records, name == "B", MAX_TOKENS * CHAR_CAP)])


def fake_image(records: list[dict]) -> np.ndarray:
    return fake_embed(records, "A")


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
    (paths.emb / "eg2" / "B.json").write_text(json.dumps(model_identity(MODELS["eg2"], dim=DIM)))
    (paths.emb / "image").mkdir(parents=True)
    mask = np.array([r["image_ok"] for r in records])
    np.save(paths.emb / "image" / "eg2-full.npy", np.where(mask[:, None], X, 0))
    np.save(paths.emb / "image" / "eg2-full-mask.npy", mask)
    (paths.emb / "image" / "eg2-full.json").write_text(json.dumps(model_identity(IMAGE_MODEL, image=True, dim=DIM)))
    return paths


class Fixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp())
        cls.paths = build(cls.tmp)
        with patch("builtins.print"):
            cls.lib, cls.stats = engine.load(
                cls.paths, cls.tmp / "archive", fake_encode, fake_embed, embed_image=fake_image, dim=DIM
            )

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

    def test_add_suggestions_require_a_margin_in_each_tags_score_spread(self):
        app = server.App.__new__(server.App)
        threshold = np.ones(len(self.lib.tags))
        spread = np.linspace(0.05, 0.2, len(self.lib.tags))
        scores = np.zeros(self.lib.S.shape, dtype=float)
        app.lib = dataclasses.replace(
            self.lib, S=scores, threshold=threshold, spread=spread, Y=np.zeros_like(self.lib.Y)
        )
        heads = set(self.lib.rules.heads.tolist())
        for j in (next(iter(heads)), next(j for j in range(len(self.lib.tags)) if j not in heads)):
            tag = self.lib.tags[j]
            boundary = threshold[j] + server.ADD_K_SD * spread[j]
            for score, shown in ((np.nextafter(boundary, -np.inf), False), (boundary, True)):
                with self.subTest(tag=tag, score=score):
                    scores[0, j] = score
                    self.assertTrue(engine.prechecks(app.lib, [0], [tag])[0][0]["checked"])
                    self.assertEqual(shown, tag in app.suggested(0, []))
            scores[0, j] = 0

    def test_a_name_the_vocabulary_gains_is_a_zero_shot_tag_queried_by_its_name(self):
        gained = {**self.lib.state.active, "sleeper cars": "Sleeper cars"}
        lib = engine.apply_state(self.lib, dataclasses.replace(self.lib.state, active=gained), fake_encode)
        self.assertEqual(len(self.lib.tags), len(lib.tags) - 1)  # the loaded library is left as it was
        j = lib.tags.index("Sleeper cars")
        s = lib.X @ fake_encode(["Sleeper cars"])[0]
        np.testing.assert_allclose(s, lib.S[:, j], rtol=1e-6)
        self.assertAlmostEqual(s.mean() + engine.ZERO_K_SD * s.std(), lib.threshold[j], places=5)
        unseen = self.lib.tags.index("Woodworking")  # an owner tag nobody holds: the rule `load` gives it
        self.assertFalse(self.lib.Y[:, unseen].any())
        u = self.lib.S[:, unseen]
        self.assertAlmostEqual(u.mean() + engine.ZERO_K_SD * u.std(), self.lib.threshold[unseen], places=5)
        self.assertIn({"name": "Sleeper cars", "positives": 0, "source": "zero shot"}, engine.tag_info(lib))
        rows = list(range(len(lib.records)))
        sugg = [p[0] for p in engine.prechecks(lib, rows, ["Sleeper cars"])]
        self.assertEqual({"zero shot"}, {x["source"] for x in sugg})
        self.assertEqual([bool(x) for x in s >= lib.threshold[j]], [x["checked"] for x in sugg])
        self.assertIn("Sleeper cars", {x["tag"] for x in engine.suggest(lib, rows, len(lib.tags))})

    def test_a_retired_name_keeps_its_column_out_of_sight_and_comes_back_in_it(self):
        j, rows = self.lib.tags.index("Coffee"), list(range(len(self.lib.records)))
        state = self.lib.state
        retired = dataclasses.replace(
            state,
            active={k: v for k, v in state.active.items() if k != "coffee"},
            retired={**state.retired, "coffee": "Coffee"},
        )
        lib = engine.apply_state(self.lib, retired, fake_encode)
        self.assertEqual(self.lib.tags, lib.tags, "no column goes or moves")
        self.assertNotIn("Coffee", [t["name"] for t in engine.tag_info(lib)])
        self.assertNotIn("Coffee", {x["tag"] for x in engine.suggest(lib, rows, len(lib.tags))})
        self.assertFalse(lib.Y[:, j].any())
        self.assertFalse(any("Coffee" in r["tags"] for r in lib.records))
        self.assertTrue(state.adds, "its decisions are still there")
        back = engine.apply_state(lib, state, lambda texts: self.fail(f"encoded {texts}"))
        self.assertEqual(self.lib.tags, back.tags)
        np.testing.assert_array_equal(self.lib.Y, back.Y)
        self.assertIs(self.lib.S, back.S, "the column it had, scored as before")
        self.assertEqual(engine.tag_info(self.lib), engine.tag_info(back))

    def test_add_tags_skips_names_the_library_has_in_any_case(self):
        self.assertIs(self.lib, engine.add_tags(self.lib, ["coffee", "READ LATER"], fake_encode))
        lib = engine.add_tags(self.lib, ["New", "NEW", "trains"], fake_encode)
        self.assertEqual([*self.lib.tags, "New"], lib.tags)
        self.assertEqual("Coffee", engine.find_tag(lib, "cOFFEE"))


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
        self.assertEqual(["not-relevant.json"], [p.name for p in self.tmp.iterdir()])
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


class Served(Fixture):
    """The app on its own copy of the archive and fresh lists, served on a free port, and one HTTP call to it as the
    shell makes it. `edit` and `tag` change `library.json` as knowmoretabs does, replacing it whole."""

    def setUp(self):
        self.data = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.data)
        self.root = self.data / "archive"
        shutil.copytree(self.tmp / "archive", self.root)
        self.app = self.restart()
        self.server = server.serve(self.app, 0, signed.KEY)
        self.port = self.server.server_address[1]
        thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)

    def restart(self) -> server.App:
        """An app on this archive and these lists, as a restart starts one."""
        return server.App(dataclasses.replace(self.lib, root=self.root), self.data / "app", fake_encode, fake_embed)

    def call(self, method, path, body=None, headers=None, prove=True):
        return signed.call(self.port, method, path, body, headers, prove)

    def edit(self, change) -> dict:
        return synthetic_archive.edit_state(self.root, change)

    def tag(self, rows: list[int], name: str, side: str = "add", used_at: str | None = None) -> None:
        keys = [self.lib.records[r]["key"] for r in rows]
        self.edit(lambda state: synthetic_archive.decide(state, keys, name, side, used_at))

    def forget(self, rows: list[int], value: bool = True) -> None:
        keys = {self.lib.records[r]["key"] for r in rows}

        def change(state):
            kept = [k for k in state["forgotten"] if k not in keys]
            state["forgotten"] = kept + sorted(keys) if value else kept

        self.edit(change)

    def rows_for(self, query: str, n: int = 20, **body) -> list[int]:
        return [h["row"] for h in self.call("POST", "/api/search", {"query": query, "n": n, **body})[1]["hits"]]

    def found(self, query: str) -> dict:
        """Each hit's tags, by row."""
        return {h["row"]: h["tags"] for h in self.call("POST", "/api/search", {"query": query, "n": 50})[1]["hits"]}


class ServerTests(Served):
    def test_api_finishes_while_an_image_transfer_is_blocked(self):
        started, release, answered = threading.Event(), threading.Event(), threading.Event()
        handler = self.server.RequestHandlerClass
        send = handler.send

        def blocked_image(request, status, body, kind, cache=False):
            if kind == "image/jpeg":
                started.set()
                release.wait(5)
            return send(request, status, body, kind, cache)

        row = next(i for i, r in enumerate(self.lib.records) if r["image_ok"])
        with patch.object(handler, "send", blocked_image), ThreadPoolExecutor(max_workers=2) as pool:
            image = pool.submit(self.call, "GET", f"/img/{self.app.page(row)['image']}")
            try:
                self.assertTrue(started.wait(2), "image request reached the transfer")
                api = pool.submit(self.call, "GET", "/api/library")
                api.add_done_callback(lambda _: answered.set())
                self.assertTrue(answered.wait(2), "API must complete before the image transfer is released")
                self.assertEqual(200, api.result(timeout=2)[0])
            finally:
                release.set()
            self.assertEqual(200, image.result(timeout=2)[0])

    def test_concurrent_requests_serialize_model_and_list_access(self):
        started, release = threading.Event(), threading.Event()
        dispatched, encoded = threading.Event(), threading.Event()
        handler = self.server.RequestHandlerClass
        api, library = handler.api, self.app.library

        def blocked_library():
            started.set()
            release.wait(5)
            return library()

        def tracked_api(request, method, path, body):
            if path == "/api/search":
                dispatched.set()
            return api(request, method, path, body)

        def encode(texts):
            encoded.set()
            return fake_encode(texts)

        with (
            patch.object(self.app, "library", blocked_library),
            patch.object(self.app, "encode", encode),
            patch.object(handler, "api", tracked_api),
            ThreadPoolExecutor(max_workers=2) as pool,
        ):
            first = pool.submit(self.call, "GET", "/api/library")
            try:
                self.assertTrue(started.wait(2))
                second = pool.submit(self.call, "POST", "/api/search", {"query": "night train"})
                self.assertTrue(dispatched.wait(2), "second request reached the API")
                self.assertFalse(encoded.wait(0.2), "model waits for the earlier API operation")
            finally:
                release.set()
            self.assertEqual(200, first.result(timeout=2)[0])
            self.assertEqual(200, second.result(timeout=2)[0])
        self.assertTrue(encoded.is_set())

    def test_guards(self):
        self.assertEqual(421, self.call("GET", "/api/library", headers={"Host": "attacker.example"})[0])
        self.assertEqual(403, self.call("POST", "/api/search", {}, {"Origin": "http://attacker.example"})[0])
        self.assertEqual(415, self.call("POST", "/api/search", {}, {"Content-Type": "text/plain"})[0])
        self.assertEqual(404, self.call("GET", "/..%2Fserver.py")[0])
        self.assertEqual(404, self.call("GET", "/static/../server.py")[0])
        status, body, r = self.call("GET", "/")
        self.assertEqual(200, status)
        self.assertIn(b"knowmoretabs", body)
        self.assertIn("default-src 'none'", r.getheader("Content-Security-Policy"))
        missing = next(i for i, r in enumerate(self.lib.records) if not r["image_ok"])
        self.assertIsNone(self.app.page(missing)["image"])
        self.assertEqual(404, self.call("GET", f"/img/{sha256_hex(self.lib.records[missing]['key'])}")[0])
        shown = next(i for i, r in enumerate(self.lib.records) if r["image_ok"])
        self.assertEqual(404, self.call("GET", f"/img/{shown}")[0], "images are addressed by page, not by row")
        status, body, r = self.call("GET", f"/img/{self.app.page(shown)['image']}")
        self.assertEqual((200, "image/jpeg"), (status, r.getheader("Content-Type")))

    def test_the_decision_session_and_send_routes_are_gone(self):
        rows = self.rows_for("night train")[:2]
        before = (self.root / "library.json").read_bytes()
        for method, path, body in (
            ("POST", "/api/apply", {"rows": rows, "tag": "Trains", "value": True}),
            ("POST", "/api/undo", {"batch": 1}),
            ("POST", "/api/forget", {"rows": rows, "value": True}),
            ("POST", "/api/tags", {"name": "Sleeper cars"}),
            ("GET", "/api/sessions", None),
            ("GET", "/api/sessions/1", None),
            ("POST", "/api/sessions", {"query": "night train", "rows": rows, "picked": ["Trains"]}),
            ("POST", "/api/sessions/1/pages/0", {"status": "decided"}),
            ("POST", "/api/send/preview", {}),
            ("POST", "/api/send", {"preview": 1}),
            ("GET", "/review.js", None),
            ("GET", "/swipe.js", None),
            ("GET", "/pick.js", None),
            ("GET", "/search.js", None),
        ):
            self.assertEqual(404, self.call(method, path, body)[0], path)
        self.assertEqual(before, (self.root / "library.json").read_bytes())
        self.assertFalse((self.data / "app" / "state.json").exists())

    def test_files_an_earlier_app_kept_its_decisions_in_stay_unread_and_untouched(self):
        rows = self.rows_for("night train")[:3]
        keys = [self.lib.records[r]["key"] for r in rows]
        old = self.data / "app"
        old.mkdir(exist_ok=True)
        (old / "exports" / "20261001T000000Z").mkdir(parents=True)
        files = {
            old / "state.json": json.dumps(
                {
                    "version": 1,
                    "sessions": [],
                    "tags": [{"name": "Only here", "created_at": 1.0}],
                    "direct": [{"key": keys[0], "tag": "Only here", "value": True, "at": 2.0, "batch": 1}],
                }
            ),
            old / "forgotten.json": json.dumps({"version": 1, "keys": keys[1:2]}),
            old / "exports" / "20261001T000000Z" / "answers.jsonl": json.dumps({"url": keys[2], "tags": ["Old"]})
            + "\n",
        }
        for path, text in files.items():
            path.write_text(text)
        app = self.restart()
        names = [t["name"] for t in app.library()["tags"]] + app.library()["app_tags"]
        self.assertNotIn("Only here", names)
        self.assertNotIn(rows[1], app.library()["forgotten"])
        self.assertTrue(app.lib.live[rows[1]])
        self.assertEqual(self.lib.records[rows[0]]["tags"], app.page(rows[0])["tags"])
        self.call("GET", "/api/library")
        self.call("POST", "/api/search", {"query": "night train"})
        self.assertEqual({path: text for path, text in files.items()}, {path: path.read_text() for path in files})
        self.assertEqual(["20261001T000000Z"], [p.name for p in (old / "exports").iterdir()])

    def test_an_empty_search_lists_every_page_newest_first(self):
        n = len(self.lib.records)
        times = [None if i == 3 else float((i * 7) % n) for i in range(n)]  # one page without a time
        self.app.lib = dataclasses.replace(
            self.app.lib, records=[{**r, "added": t} for r, t in zip(self.app.lib.records, times, strict=True)]
        )
        live = np.flatnonzero(self.app.live())
        expected = sorted(live, key=lambda i: (times[i] is None, -(times[i] or 0), i))
        first = self.call("POST", "/api/search", {"query": " ", "n": 20})[1]
        second = self.call("POST", "/api/search", {"query": "", "n": 20, "offset": 20})[1]
        self.assertEqual((0, 20, len(live)), (first["offset"], second["offset"], first["total"]))
        self.assertEqual(expected[:40], [h["row"] for h in first["hits"] + second["hits"]])
        self.assertEqual(3, expected[-1], "a page without a time comes last")
        before = self.call("POST", "/api/search", {"query": "", "n": 50, "untagged": True})[1]
        tagged = next(int(r) for r in expected if not self.lib.records[r]["tags"])
        self.tag([tagged], "Trains")
        untagged = self.call("POST", "/api/search", {"query": "", "n": 50, "untagged": True})[1]
        self.assertEqual(before["total"] - 1, untagged["total"])
        self.assertNotIn(tagged, [h["row"] for h in untagged["hits"]])

    def test_excluded_results_stay_out_of_suggestions(self):
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
        self.assertEqual(engine.suggest(self.app.lib, kept), after["suggested"])
        self.assertNotEqual(engine.suggest(self.app.lib, rows), after["suggested"])
        again = self.call("POST", "/api/search", {"query": "night train", "n": 20})[1]  # a reload or restart
        self.assertEqual((after["excluded"], rows[9]), (again["excluded"], again["cut"]))
        self.assertEqual(400, self.call("POST", "/api/exclusions", judge | {"action": "exclude", "row": -1})[0])
        self.assertTrue((self.data / "app" / "not-relevant.json").exists())

    def test_a_page_excluded_in_one_search_is_untouched_in_another(self):
        other = {"query": "sleeper trains", "n": 20}
        before = self.call("POST", "/api/search", other)[1]
        refined = self.call("POST", "/api/search", other | {"refine": True})[1]
        rows = [h["row"] for h in self.call("POST", "/api/search", {"query": "night train", "n": 20})[1]["hits"]]
        decisions = (self.root / "library.json").read_bytes()
        self.call("POST", "/api/exclusions", {"query": "night train", "rows": rows, "action": "cut", "row": rows[0]})
        self.call(
            "POST", "/api/exclusions", {"query": "night train", "rows": rows, "action": "exclude", "row": rows[0]}
        )
        after = self.call("POST", "/api/search", other)[1]
        self.assertEqual(before, after | {"ms": before["ms"]})  # same hits, ranks, suggestions; nothing excluded
        self.assertEqual([], after["excluded"])
        again = self.call("POST", "/api/search", other | {"refine": True})[1]
        self.assertEqual(refined["hits"], again["hits"])
        self.assertEqual(decisions, (self.root / "library.json").read_bytes(), "an exclusion is not a dismissal")

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
        app = self.restart()
        query = "night train"
        q = fake_encode([query])[0]
        hits = engine.search(self.lib, q, query, 50)
        keys = [self.lib.records[h["row"]]["key"] for h in hits]
        app.exclusions.apply(query, keys[:20], "cut", keys[9])
        with patch("tagger.app.server.prototype", wraps=server.prototype) as prototype:
            app.refined(query, q, hits)
        self.assertEqual(10, len(prototype.call_args.args[1]))
        self.assertEqual(40, len(prototype.call_args.args[2]))

    def test_the_selection_persists_privately_in_order_until_cleared(self):
        self.assertEqual({"pages": []}, self.call("GET", "/api/selection")[1])
        rows = self.rows_for("espresso")[:2] + self.rows_for("night train")[:2]
        picked = [rows[3], rows[0], rows[2]]
        self.assertEqual(200, self.call("POST", "/api/selection", {"rows": picked})[0])
        restarted = self.restart()
        self.assertEqual(picked, [p["row"] for p in restarted.listed(restarted.selection)["pages"]])
        self.assertEqual(0o600, stat.S_IMODE((self.data / "app" / "selection.json").stat().st_mode))
        self.assertEqual(400, self.call("POST", "/api/selection", {"rows": [-1]})[0])
        self.assertEqual(picked, [p["row"] for p in self.call("GET", "/api/selection")[1]["pages"]])
        self.call("POST", "/api/selection", {"rows": []})
        restarted = self.restart()
        self.assertEqual([], restarted.listed(restarted.selection)["pages"])

    def test_a_page_the_library_forgets_leaves_every_view_and_count_and_keeps_its_place_in_the_lists(self):
        rows = self.rows_for("night train")
        gone = rows[1]
        live = int(self.app.lib.live.sum())
        self.call("POST", "/api/selection", {"rows": [rows[0], gone, rows[2]]})
        self.call("POST", "/api/pins", {"rows": [gone, rows[2]]})
        untagged_before = self.call("POST", "/api/search", {"query": "night train", "untagged": True})[1]["total"]
        self.forget([gone])  # Tag's Forget, the Library's, or a CLI run: the library's own write
        searched = self.call("POST", "/api/search", {"query": "night train", "n": 50})[1]
        self.assertNotIn(gone, [h["row"] for h in searched["hits"]])
        self.assertEqual(live - 1, searched["total"])
        untagged = self.call("POST", "/api/search", {"query": "night train", "untagged": True})[1]["total"]
        self.assertEqual(untagged_before - (not self.lib.records[gone]["tags"]), untagged)
        like = self.call("POST", "/api/like", {"tag": "Rust", "n": 50, "offset": 0})[1]
        every = [
            h["row"]
            for o in range(0, like["total"], 50)
            for h in self.call("POST", "/api/like", {"tag": "Rust", "n": 50, "offset": o})[1]["hits"]
        ]
        self.assertNotIn(gone, every)
        library = self.call("GET", "/api/library")[1]
        self.assertEqual(live - 1, library["pages"])
        self.assertIn(gone, library["forgotten"], "the screen leaves it out of its lists and counts")
        self.assertEqual([rows[0], gone, rows[2]], [p["row"] for p in self.call("GET", "/api/selection")[1]["pages"]])
        self.assertEqual([gone, rows[2]], [p["row"] for p in self.call("GET", "/api/pins")[1]["pages"]])
        self.assertEqual(200, self.call("POST", "/api/pins", {"rows": [gone, rows[2]]})[0], "a list keeps its place")
        self.assertEqual(400, self.call("POST", "/api/exclusions", {"query": "q", "rows": [gone], "action": "cut"})[0])
        restarted = self.restart()
        self.assertEqual(live - 1, int(restarted.live().sum()))  # it stays forgotten over a restart
        self.forget([gone], False)
        self.assertIn(gone, self.rows_for("night train", n=50))
        self.assertEqual(live, self.call("GET", "/api/library")[1]["pages"])
        self.assertEqual([gone, rows[2]], [p["row"] for p in self.call("GET", "/api/pins")[1]["pages"]])

    def test_pins_persist_privately_in_order_and_never_touch_the_selection(self):
        rows = self.rows_for("espresso")[:3]
        self.call("POST", "/api/selection", {"rows": rows[:1]})
        self.assertEqual(200, self.call("POST", "/api/pins", {"rows": [rows[2], rows[1]]})[0])
        restarted = self.restart()
        self.assertEqual([rows[2], rows[1]], [p["row"] for p in restarted.listed(restarted.pins)["pages"]])
        self.assertEqual([rows[0]], [p["row"] for p in restarted.listed(restarted.selection)["pages"]])
        self.assertEqual(0o600, stat.S_IMODE((self.data / "app" / "pinned.json").stat().st_mode))

    def test_untagged_only_leaves_out_every_page_with_a_tag(self):
        rows = self.rows_for("night train")
        bare = [r for r in rows if not self.lib.records[r]["tags"]]
        self.tag(bare[:2], "Trains")
        untagged = self.rows_for("night train", untagged=True)
        self.assertTrue(untagged)
        self.assertFalse(any(self.app.lib.records[r]["tags"] for r in untagged))
        self.assertFalse(set(bare[:2]) & set(untagged))
        self.assertTrue(set(bare[2:]) <= set(self.rows_for("night train", n=50, untagged=True)))

    def test_search_and_like_pages_follow_on_from_an_offset_and_count_the_pages_ranked(self):
        every = self.rows_for("night train", n=server.MAX_RESULTS * 4)  # capped at one page
        self.assertEqual(server.MAX_RESULTS, len(every))
        first = self.call("POST", "/api/search", {"query": "night train", "n": 50})[1]
        second = self.call("POST", "/api/search", {"query": "night train", "n": 50, "offset": 50})[1]
        live = int(self.lib.live.sum())
        self.assertEqual((0, 50, live), (first["offset"], second["offset"], second["total"]))
        ranked = [h["row"] for h in engine.search(self.lib, fake_encode(["night train"])[0], "night train", 100)]
        self.assertEqual(ranked, [h["row"] for h in first["hits"] + second["hits"]])
        last = self.call("POST", "/api/search", {"query": "night train", "n": 50, "offset": live - 3})[1]
        self.assertEqual(3, len(last["hits"]))
        self.tag(ranked[:5], "Trains")
        without = int((self.app.lib.live & ~self.app.lib.Y[:, self.app.lib.tags.index("Trains")].astype(bool)).sum())
        untagged = self.call("POST", "/api/search", {"query": "night train", "untagged": True, "offset": 10})[1]
        self.assertEqual(int((self.app.lib.live & ~self.app.lib.Y.any(axis=1)).sum()), untagged["total"])
        like = self.call("POST", "/api/like", {"tag": "Trains", "n": 50, "offset": 50})[1]
        self.assertEqual((50, without), (like["offset"], like["total"]))
        before = self.call("POST", "/api/like", {"tag": "Trains", "n": 50})[1]["hits"]
        self.assertEqual(set(), {h["row"] for h in like["hits"]} & {h["row"] for h in before})
        self.assertEqual(min(50, without - 50), len(like["hits"]))

    def test_more_like_this_excludes_only_its_tag_near_the_tags_pages(self):
        trains = [r for r in self.rows_for("night train") if topic_of(self.lib.records[r]["key"]) == "trains"]
        self.tag(trains[:4], "Sleeper cars")
        self.tag(trains[4:6], "Rust")
        status, out, _ = self.call("POST", "/api/like", {"tag": "Sleeper cars", "n": 20})
        self.assertEqual((200, "Sleeper cars", 20), (status, out["tag"], len(out["hits"])))
        liked = [h["row"] for h in out["hits"]]
        self.assertFalse(set(trains[:4]) & set(liked))
        self.assertTrue(set(trains[4:6]) <= set(liked))  # a different tag still belongs in this view
        self.assertTrue(all("Sleeper cars" not in h["tags"] for h in out["hits"]))
        self.assertEqual(["trains"] * 5, [topic_of(self.lib.records[r]["key"]) for r in liked[:5]])
        self.assertEqual(400, self.call("POST", "/api/like", {"tag": "Nope"})[0])

    def test_more_like_this_ranks_toward_the_tags_pages_and_away_from_its_dismissals(self):
        trains = [r for r in self.rows_for("night train") if topic_of(self.lib.records[r]["key"]) == "trains"]
        self.tag(trains[:6], "Zephyr")  # names no page or topic word shares: only the tag's pages can pull
        self.edit(lambda state: state["vocabulary"].setdefault("Quill", {"created_at": synthetic_archive.AT}))

        def trains_in_top_10(tag: str) -> int:
            hits = self.call("POST", "/api/like", {"tag": tag, "n": 20})[1]["hits"][:10]
            return sum(topic_of(h["url"]) == "trains" for h in hits)

        self.assertGreaterEqual(trains_in_top_10("Zephyr"), trains_in_top_10("Quill") + 3)
        self.tag(trains[6:7], "Vellum")
        self.tag(trains[7:8], "Vellum", "remove")
        with patch("tagger.app.server.prototype", wraps=server.prototype) as prototype:
            self.call("POST", "/api/like", {"tag": "Vellum", "n": 20})
        _, toward, away = prototype.call_args.args
        np.testing.assert_array_equal(self.app.lib.X[trains[6:7]], toward)
        np.testing.assert_array_equal(self.app.lib.X[trains[7:8]], away, "the pages it was turned down on")

    def test_refined_cut_ranking_is_stable_after_reload_and_restart(self):
        body = {"query": "night train ", "n": 50}
        rows = [h["row"] for h in self.call("POST", "/api/search", body)[1]["hits"]]
        judge = {"query": body["query"], "rows": rows}
        self.call("POST", "/api/exclusions", judge | {"action": "exclude", "row": rows[1]})
        self.call("POST", "/api/exclusions", judge | {"action": "cut", "row": rows[9]})
        first = self.call("POST", "/api/search", body | {"refine": True})[1]
        again = self.restart().search(body | {"refine": True})
        self.assertEqual([h["row"] for h in first["hits"]], [h["row"] for h in again["hits"]])
        self.assertEqual(first["excluded"], again["excluded"])
        self.assertEqual(first["suggested"], again["suggested"])


class FollowTests(Served):
    """Every view the service gives follows `library.json` as knowmoretabs leaves it, on the next request."""

    def test_tags_labels_lists_and_counts_follow_a_replaced_file(self):
        rows = self.rows_for("night train")
        bare = [r for r in rows if not self.lib.records[r]["tags"]][:2]
        held = next(r for r in rows if "Trains" in self.lib.records[r]["tags"])
        self.call("POST", "/api/pins", {"rows": bare})
        before = self.call("GET", "/api/library")[1]
        self.tag(bare, "Typography")
        self.tag([held], "Trains", "remove")
        found = self.found("night train")
        self.assertEqual([["Typography"], ["Typography"]], [found[r] for r in bare])
        self.assertNotIn("Trains", found[held])
        self.assertEqual([["Typography"]] * 2, [p["tags"] for p in self.call("GET", "/api/pins")[1]["pages"]])
        j = self.app.lib.tags.index("Typography")
        self.assertTrue(self.app.lib.Y[bare, j].all())
        self.assertFalse(self.app.lib.Y[held, self.app.lib.tags.index("Trains")])
        after = self.call("GET", "/api/library")[1]
        count = lambda library, name: next(t["positives"] for t in library["tags"] if t["name"] == name)  # noqa: E731
        self.assertEqual(count(before, "Typography") + 2, count(after, "Typography"))
        self.assertEqual(count(before, "Trains") - 1, count(after, "Trains"))
        self.assertNotIn(bare[0], self.rows_for("night train", n=50, untagged=True))

    def test_a_new_name_is_encoded_once_and_a_retired_one_leaves_every_list(self):
        encoded = []

        def encode(texts):
            encoded.append(list(texts))
            return fake_encode(texts)

        rows = self.rows_for("night train")
        with patch.object(self.app, "encode", encode):
            self.tag(rows[:2], "Sleeper cars", used_at="2026-10-09T00:00:00Z")
            library = self.call("GET", "/api/library")[1]
            self.call("GET", "/api/library")
            self.call("POST", "/api/search", {"query": "", "n": 5})
        self.assertEqual([["Sleeper cars"]], encoded, "once, at the first read after the change")
        self.assertIn({"name": "Sleeper cars", "positives": 2, "source": "zero shot"}, library["tags"])
        self.assertIn("Sleeper cars", library["app_tags"])
        self.assertEqual("Sleeper cars", library["recent"][0])
        columns = list(self.app.lib.tags)
        self.edit(lambda state: state["vocabulary"]["Sleeper cars"].update(retired_at="2026-10-10T00:00:00Z"))
        library = self.call("GET", "/api/library")[1]
        self.assertNotIn("Sleeper cars", [t["name"] for t in library["tags"]] + library["app_tags"] + library["recent"])
        self.assertFalse(any("Sleeper cars" in self.found("night train").get(r, []) for r in rows[:2]))
        self.assertEqual(400, self.call("POST", "/api/like", {"tag": "Sleeper cars"})[0])
        with patch.object(self.app, "encode", encode):
            self.edit(lambda state: state["vocabulary"]["Sleeper cars"].pop("retired_at"))
            library = self.call("GET", "/api/library")[1]
        self.assertEqual(columns, self.app.lib.tags, "brought back in its own column")
        self.assertEqual([["Sleeper cars"]], encoded, "and not encoded again")
        self.assertIn({"name": "Sleeper cars", "positives": 2, "source": "zero shot"}, library["tags"])

    def test_following_reuses_vectors_and_keyword_index(self):
        before = self.app.lib
        self.tag(self.rows_for("night train")[:3], "Sleeper cars")
        self.forget(self.rows_for("espresso")[:1])
        self.call("GET", "/api/library")
        after = self.app.lib
        self.assertIsNot(before, after)
        for name in ("X", "image", "has_image", "keywords", "K"):
            self.assertIs(getattr(before, name), getattr(after, name), name)

    def test_an_unreadable_file_answers_503_keeps_the_last_view_and_recovers(self):
        rows = self.rows_for("night train")
        shown, good = self.app.lib, json.loads((self.root / "library.json").read_text())
        (self.root / "library.json").write_text('{"schema_version": 1, "forgo')
        for method, path, body in (
            ("GET", "/api/library", None),
            ("POST", "/api/search", {"query": "night train"}),
            ("POST", "/api/like", {"tag": "Trains"}),
            ("GET", "/api/selection", None),
            ("POST", "/api/pins", {"rows": rows[:1]}),
            ("POST", "/api/rank", {"query": "night train"}),
        ):
            status, body, _ = self.call(method, path, body)
            self.assertEqual(503, status, path)
            self.assertIn("library cannot be read", body["error"])
        self.assertIs(shown, self.app.lib, "the last good view stays, never shown as fresh")
        synthetic_archive.decide(good, [self.lib.records[rows[0]]["key"]], "Typography")
        synthetic_archive.write_state(self.root, good)
        self.assertEqual(200, self.call("GET", "/api/library")[0])
        self.assertIn("Typography", self.found("night train")[rows[0]])


if __name__ == "__main__":
    unittest.main()
