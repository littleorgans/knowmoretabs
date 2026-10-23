"""The Library's rank route on the synthetic archive with the fake encoder: every page's key once, in the
search's fused order, forgotten pages included; the archive's identity on every reply; a bounded wait for the
model."""

import time
from dataclasses import replace
from types import SimpleNamespace
from unittest.mock import Mock, patch

import numpy as np
from scipy.sparse import csr_matrix
from test_app import Served, fake_encode, topic_of

from tagger import chunks
from tagger.app import engine, server
from tagger.archive import sha256_hex


class RankTests(Served):
    def rank(self, query: str, headers=None):
        return self.call("POST", "/api/rank", {"query": query}, headers)

    def test_every_page_once_forgotten_ones_included(self):
        in_archive = {r["key"] for r in self.lib.records if r["forgotten"]}
        just_now = next(i for i, r in enumerate(self.lib.records) if not r["forgotten"])
        self.forget([just_now])
        self.assertTrue(in_archive)
        status, body, _ = self.rank("espresso grinder")
        self.assertEqual(200, status)
        self.assertEqual(["keys"], list(body))
        keys = body["keys"]
        self.assertEqual(len(keys), len(set(keys)))
        self.assertEqual(sorted(r["key"] for r in self.lib.records), sorted(keys))
        self.assertLessEqual(in_archive | {self.lib.records[just_now]["key"]}, set(keys))

    def test_the_order_is_the_search_ranking_over_every_page(self):
        query = "espresso grinder"
        every = np.ones(len(self.lib.records), bool)
        hits = engine.search(self.lib, fake_encode([query])[0], query, len(every), among=every)
        keys = self.rank(query)[1]["keys"]
        self.assertEqual([self.lib.records[h["row"]]["key"] for h in hits], keys)
        self.assertTrue(all(topic_of(k) == "coffee" for k in keys[:10]))

    def test_a_blank_query_is_refused(self):
        for query in ("", "  "):
            status, body, _ = self.rank(query)
            self.assertEqual((400, {"error": "type a search"}), (status, body))

    def test_every_reply_names_the_archive_by_device_and_inode(self):
        st = self.root.stat()
        named = f"{st.st_dev}:{st.st_ino}"
        replies = {
            "rank": self.rank("night train"),
            "blank": self.rank(""),
            "library": self.call("GET", "/api/library"),
            "static": self.call("GET", "/"),
            "missing": self.call("GET", "/api/nothing"),
            "host": self.call("GET", "/api/library", headers={"Host": "attacker.example"}),
            "origin": self.rank("night train", {"Origin": "http://attacker.example"}),
        }
        for route, (_, _, reply) in replies.items():
            self.assertEqual(named, reply.getheader(server.ARCHIVE_HEADER), route)

    def test_a_held_model_answers_busy_within_the_wait(self):
        with self.app.lock:
            start = time.monotonic()
            status, body, _ = self.rank("night train")
            waited = time.monotonic() - start
        self.assertEqual((503, {"error": "busy"}), (status, body))
        self.assertGreaterEqual(waited, server.RANK_WAIT)
        self.assertLess(waited, server.RANK_WAIT + 2)
        self.assertEqual(200, self.rank("night train")[0])

    def test_only_this_servers_host_reaches_the_rank(self):
        self.assertEqual(200, self.rank("night train")[0])
        self.assertEqual(421, self.rank("night train", {"Host": "attacker.example"})[0])

    def chunk_library(self):
        """Three live pages: an excluded strongest chunk, a strong passage, and a weak C/page fallback."""
        selected = np.flatnonzero(self.app.lib.live)[:3]
        records = [self.app.lib.records[i] for i in selected]
        bodies, refs = {}, []
        for record in records:
            text = "# Intro\r\n\r\nQuiet.\r\n\r\n# Passage\r\n\r\n" + "café 🦊 e\u0301 \r\n" * 40
            title = record["title"]
            bodies[record["key"]] = chunks.Body(title, text.encode(), chunks._hash(title, text), sha256_hex(text))
            refs.extend(chunks.chunk_page(record["key"], title, text))
        self.assertEqual(6, len(refs))
        # Scramble references so an equal score must choose by offsets, rather than stored row order.
        scores = [1.0, 1.0, -0.9, 0.8, -0.7, -0.7]
        vectors = np.zeros((len(refs), self.app.lib.dim), np.float32)
        vectors[:, 0] = scores
        vectors[:, 1] = np.sqrt(1 - vectors[:, 0] ** 2)
        snapshot = chunks.Snapshot(refs[::-1], vectors[::-1], bodies, {})
        X = np.zeros((3, self.app.lib.dim), np.float32)
        X[:, 0] = [0.5, -0.5, 1.0]  # the page vector and the mean chunk vector prefer page 2
        X[:, 1] = np.sqrt(1 - X[:, 0] ** 2)
        self.app.lib = replace(
            self.app.lib,
            records=records,
            X=X,
            live=np.ones(3, bool),
            chunks=snapshot,
            chunk_status="ready",
            keywords=SimpleNamespace(transform=lambda _: csr_matrix([[1.0]])),
            K=csr_matrix([[0.0], [0.1], [0.9]]),
            image=X.copy(),
            has_image=np.array([True, False, True]),
        )
        q = np.zeros(self.app.lib.dim, np.float32)
        q[0] = 1
        self.app.encode = Mock(return_value=q[None, :])
        return q, records, snapshot

    def test_best_chunk_single_text_vote_masks_ties_and_page_fallback(self):
        q, records, _ = self.chunk_library()
        lib, among = self.app.lib, np.array([False, True, True])
        hits = engine.search(lib, q, "synthetic", 3, among=among)
        self.assertEqual([1, 2], [h["row"] for h in hits])  # tied RRF preserves page row order
        self.assertEqual(1, hits[0]["sources"]["text"]["rank"])
        self.assertAlmostEqual(0.8, hits[0]["chunk"]["score"], places=6)
        self.assertAlmostEqual(1 / 61 + 1 / 62, hits[0]["fused"])
        self.assertAlmostEqual(hits[0]["fused"], hits[1]["fused"])
        self.assertEqual({"text", "keyword"}, set(hits[0]["sources"]))
        self.assertEqual(0, hits[1]["chunk"]["start"], "equal chunks choose the smallest reference")
        self.assertLess(hits[1]["sources"]["text"]["score"], 0, "weak C is not replaced by page cosine")
        np.testing.assert_array_equal(engine.order(lib, q, "synthetic", among), [h["row"] for h in hits])
        images = engine.search(lib, q, "synthetic", 3, images=True, among=among)
        self.assertEqual([2, 1], [h["row"] for h in images])
        self.assertEqual(1, images[0]["sources"]["image"]["rank"])
        self.assertEqual(0, images[1]["sources"]["image"]["rank"])
        kept = [j for j, ref in enumerate(lib.chunks.rows) if ref["page_id"] != records[2]["key"]]
        partial = replace(lib.chunks, rows=[lib.chunks.rows[j] for j in kept], vectors=lib.chunks.vectors[kept])
        for name in ("A", "B"):
            missing = replace(lib, chunks=partial, input_name=name)
            fallback = engine.search(missing, q, "synthetic", 3, among=among)
            self.assertEqual(2, fallback[0]["row"])
            self.assertEqual(name, fallback[0]["semantic_source"])
            self.assertEqual("C", fallback[1]["semantic_source"])
        # A C row without a page mapping cannot participate.
        orphan = {**lib.chunks.rows[0], "page_id": "synthetic-orphan", "id": "orphan"}
        extra = replace(lib.chunks, rows=[*lib.chunks.rows, orphan], vectors=np.vstack([lib.chunks.vectors, q]))
        self.assertEqual(hits, engine.search(replace(lib, chunks=extra), q, "synthetic", 3, among=among))

    def test_detailed_signed_rank_exact_snippets_stale_fallback_and_visibility(self):
        _, records, snapshot = self.chunk_library()
        candidates = [{"key": r["key"], "content_sha256": snapshot.bodies[r["key"]].content_sha256} for r in records]
        request = {"schema_version": 1, "query": "synthetic", "among": candidates[1:], "snippet_chars": 200}
        with patch.object(chunks, "reconcile", side_effect=AssertionError("rank cannot refresh C")):
            status, result, _ = self.call("POST", "/api/rank", request)
        self.assertEqual(200, status)
        self.assertEqual([records[1]["key"], records[2]["key"]], result["keys"])
        self.assertEqual([], result["fallback"])
        hit = result["hits"][0]
        self.assertEqual("C", hit["semantic_source"])
        self.assertEqual("C", hit["sources"]["text"]["input"])
        self.assertEqual("ready", hit["chunk_status"])
        self.assertEqual("Passage", hit["section"])
        raw = snapshot.bodies[hit["key"]].raw
        span, ref = hit["snippet_range"], hit["chunk"]
        self.assertEqual(raw[span["start"] : span["end"]].decode(), hit["snippet"])
        self.assertEqual(raw[ref["start"] : ref["end"]].decode()[:200], hit["snippet"])
        self.assertEqual(200, len(hit["snippet"]))
        self.assertIn("\r\n", hit["snippet"])
        self.assertEqual(snapshot.bodies[hit["key"]].content_sha256, hit["content_sha256"])
        candidates[1]["content_sha256"] = "0" * 64
        result = self.call("POST", "/api/rank", request)[1]
        stale = next(h for h in result["hits"] if h["key"] == records[1]["key"])
        self.assertEqual("B", stale["semantic_source"])
        self.assertEqual("stale", stale["chunk_status"])
        self.assertTrue(
            all(stale[k] is None for k in ("section", "chunk", "snippet", "snippet_range", "content_sha256"))
        )
        self.assertEqual([{"stage": "chunks", "code": "chunks_stale", "affected_pages": 1}], result["fallback"])
        self.app.lib = replace(self.app.lib, live=np.array([True, False, True]))
        self.assertEqual([records[2]["key"]], self.call("POST", "/api/rank", request)[1]["keys"])
        self.app.encode.reset_mock()
        empty = self.call("POST", "/api/rank", {**request, "among": []})[1]
        self.assertEqual({"schema_version": 1, "keys": [], "hits": [], "fallback": []}, empty)
        self.app.encode.assert_not_called()
        with patch.object(server, "RANK_REPLY_LIMIT", 1):
            oversized = self.call("POST", "/api/rank", request)[1]
        self.assertEqual("rank_response_too_large", oversized["fallback"][0]["code"])
