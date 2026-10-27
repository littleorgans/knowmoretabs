"""Owner labels refit suggestions without blocking requests. All pages and tags are synthetic."""

import threading
from dataclasses import replace
from unittest.mock import patch

import numpy as np

from test_app import TOPICS, Served, topic_of
from tagger.app import engine, server


class Died(BaseException):
    """Escapes the refit worker's `except Exception`, as a crash outside its handler does."""


class RefitTests(Served):
    def crashes(self) -> threading.Event:
        """Set once a worker dies of `Died`; its hook runs after the worker freed its slot, so wait on it."""
        died, report = threading.Event(), threading.excepthook
        hook = patch.object(threading, "excepthook", lambda args: died.set() if args.exc_type is Died else report(args))
        hook.start()
        self.addCleanup(hook.stop)
        return died

    def finish_refit(self):
        """Wait out the worker and any worker it handed its follow up to."""
        finished = []
        while True:
            with self.app.lock:
                worker = self.app._refit_thread
            if worker is None:
                return
            self.assertNotIn(worker, finished, "a finished worker still holds the refit slot")
            worker.join(20)
            self.assertFalse(worker.is_alive(), "refit finished")
            finished.append(worker)

    def test_new_tag_learns_a_head_and_retired_tag_leaves_suggestions(self):
        rows = [i for i, r in enumerate(self.lib.records) if topic_of(r["key"]) == "coffee" and self.lib.live[i]]
        held = max(rows, key=lambda i: self.lib.X[i, TOPICS.index("coffee")])
        self.tag([i for i in rows if i != held], "Fresh")  # no topic signal in the name; owner labels teach it
        self.assertEqual(200, self.call("GET", "/api/library")[0])
        self.finish_refit()
        j = self.app.lib.tags.index("Fresh")
        self.assertIn(j, self.app.lib.rules.heads)
        self.assertEqual((0, self.app.lib.X.shape[1]), self.app.lib.added_q.shape)
        self.assertIn("Fresh", self.app.suggested(held, []))
        self.assertIn("Fresh", {s["tag"] for s in engine.suggest(self.app.lib, [held], 3)})
        np.testing.assert_allclose(self.app.lib.spread, engine._spread(self.app.lib.S))
        self.edit(lambda state: state["vocabulary"]["Fresh"].update(retired_at="2026-10-24T00:00:00Z"))
        self.assertEqual(200, self.call("GET", "/api/library")[0])
        self.finish_refit()
        self.assertNotIn("Fresh", self.app.suggested(rows[-1], []))
        self.assertNotIn("Fresh", {s["tag"] for s in engine.suggest(self.app.lib, rows)})

    def test_requests_use_old_fit_and_bursts_fit_only_latest_labels(self):
        started, release = threading.Event(), threading.Event()
        fitting = engine.fit_rules
        labels = []

        def blocked(X, Y, q):
            labels.append(Y.copy())
            if len(labels) == 1:
                started.set()
                if not release.wait(10):
                    raise TimeoutError("test did not release fit")
            else:  # visibility alone, changed during the last fit, must survive its publication
                self.forget([5])
                self.call("GET", "/api/library")
            return fitting(X, Y, q)

        body = {"query": "espresso", "n": 20}
        before = self.call("POST", "/api/search", body)[1]["suggested"]
        old = self.app.lib
        with patch.object(engine, "fit_rules", side_effect=blocked):
            try:
                for i in range(4):
                    self.tag([i], "Coffee" if not old.Y[i, old.tags.index("Coffee")] else "Trains")
                    self.assertEqual(200, self.call("GET", "/api/library")[0])
                    self.assertTrue(started.wait(2), "a tag change started a fit")
                self.assertEqual(1, len(labels), "only one fit runs during the burst")
                reply = self.call("POST", "/api/search", body)
                self.assertEqual(200, reply[0])
                self.assertEqual(before, reply[1]["suggested"])
                for field in ("rules", "S", "threshold", "spread"):
                    self.assertIs(getattr(old, field), getattr(self.app.lib, field), field)
                expected = self.app.lib.Y[self.app.lib.Y.any(axis=1)].copy()
            finally:
                release.set()
                self.finish_refit()
        self.assertEqual(2, len(labels), "one follow up fits the latest labels")
        np.testing.assert_array_equal(expected, labels[-1])
        self.assertFalse(self.app.lib.live[5])
        self.assertIsNot(old.rules, self.app.lib.rules)
        np.testing.assert_allclose(self.app.lib.S, self.app.lib.rules.scores(self.app.lib.X)["supervised"])
        np.testing.assert_allclose(self.app.lib.spread, engine._spread(self.app.lib.S))

    def test_sync_during_refit_cannot_publish_scores_for_old_vectors(self):
        started, release = threading.Event(), threading.Event()
        fitting = engine.fit_rules
        calls = []

        def blocked(X, Y, q):
            calls.append(X.copy())
            if len(calls) == 1:
                started.set()
                if not release.wait(10):
                    raise TimeoutError("test did not release fit")
            return fitting(X, Y, q)

        with patch.object(engine, "fit_rules", side_effect=blocked):
            try:
                self.tag([0], "Fresh")
                self.assertEqual(200, self.call("GET", "/api/library")[0])
                self.assertTrue(started.wait(2))
                with self.app.lock:
                    # Isolate App's handoff from vector persistence, already covered by the sync tests.
                    refreshed = replace(self.app.lib, X=np.roll(self.app.lib.X, 1, axis=1))
                    with patch.object(engine, "sync", return_value=refreshed):
                        self.app.sync(self.app.lib.records[0]["key"], None)
            finally:
                release.set()
                self.finish_refit()
        self.assertEqual(2, len(calls))
        np.testing.assert_allclose(self.app.lib.S, self.app.lib.rules.scores(refreshed.X)["supervised"])

    def test_worker_dying_during_fit_still_fits_the_change_it_missed(self):
        started, release = threading.Event(), threading.Event()
        fitting = engine.fit
        labels = []

        def dies_first(X, Y, q):
            labels.append(Y.copy())
            if len(labels) == 1:
                started.set()
                if not release.wait(10):
                    raise TimeoutError("test did not release fit")
                raise Died
            return fitting(X, Y, q)

        died = self.crashes()
        old = self.app.lib
        with patch.object(engine, "fit", side_effect=dies_first):
            try:
                self.tag([0], "Fresh")
                self.assertEqual(200, self.call("GET", "/api/library")[0])
                self.assertTrue(started.wait(2), "a tag change started a fit")
                self.tag([1], "Fresh")
                self.assertEqual(200, self.call("GET", "/api/library")[0])
                self.assertIs(old.rules, self.app.lib.rules, "requests use the last good fit")
            finally:
                release.set()
                self.finish_refit()
        self.assertTrue(died.wait(10), "the worker died outside its handler")
        self.assertEqual(2, len(labels), "the change made during the dead fit is fitted without another")
        np.testing.assert_array_equal(self.app.lib.Y, labels[-1])
        self.assertIsNot(old.rules, self.app.lib.rules)
        np.testing.assert_allclose(self.app.lib.S, self.app.lib.rules.scores(self.app.lib.X)["supervised"])

    def test_worker_dying_while_publishing_keeps_last_fit_and_later_changes_refit(self):
        died = self.crashes()
        old = self.app.lib
        with patch.object(server, "replace", side_effect=Died):
            self.tag([0], "Coffee" if not old.Y[0, old.tags.index("Coffee")] else "Trains")
            self.assertEqual(200, self.call("GET", "/api/library")[0])
            self.finish_refit()
        self.assertTrue(died.wait(10), "the worker died outside its handler")
        for field in ("rules", "S", "threshold", "spread"):
            self.assertIs(getattr(old, field), getattr(self.app.lib, field), field)
        self.assertEqual(200, self.call("POST", "/api/search", {"query": "espresso", "n": 20})[0])
        self.tag([1], "Coffee" if not old.Y[1, old.tags.index("Coffee")] else "Trains")
        self.assertEqual(200, self.call("GET", "/api/library")[0])
        self.finish_refit()
        self.assertIsNot(old.rules, self.app.lib.rules)
        np.testing.assert_allclose(self.app.lib.S, self.app.lib.rules.scores(self.app.lib.X)["supervised"])

    def test_close_waits_out_the_running_fit_and_starts_no_other(self):
        started, release = threading.Event(), threading.Event()
        fitting = engine.fit_rules

        def blocked(X, Y, q):
            started.set()
            if not release.wait(10):
                raise TimeoutError("test did not release fit")
            return fitting(X, Y, q)

        with patch.object(engine, "fit_rules", side_effect=blocked):
            self.tag([0], "Fresh")
            self.assertEqual(200, self.call("GET", "/api/library")[0])
            self.assertTrue(started.wait(2), "a tag change started a fit")
            worker = self.app._refit_thread
            self.tag([1], "Fresh")
            self.assertEqual(200, self.call("GET", "/api/library")[0])  # a follow up waits on the running fit
            threading.Timer(0.2, release.set).start()  # a close that does not wait returns while the fit is held
            self.app.close()
            self.assertFalse(worker.is_alive(), "close joined the running fit")
            self.tag([2], "Fresh")
            self.assertEqual(200, self.call("GET", "/api/library")[0])
        with self.app.lock:
            self.assertIsNone(self.app._refit_thread, "a closed app starts no fit")
