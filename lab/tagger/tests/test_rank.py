"""The Library's rank route on the synthetic archive with the fake encoder: every page's key once, in the
search's fused order, forgotten pages included; the archive's identity on every reply; a bounded wait for the
model."""

import time

import numpy as np
from test_app import Served, fake_encode, topic_of

from tagger.app import engine, server


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
