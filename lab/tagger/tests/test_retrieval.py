"""Synthetic checks of test R1: the saved search session, its stop rule and costs, and the CLI umask."""

import itertools
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np
from sklearn.metrics import average_precision_score

from tagger import cli
from tagger.guided import oracle, retrieval, session
from tagger.heads import unit


def world(seed=5, n=300, d=12):
    """The tag's pages share a direction the query only half points at; the rest scatter."""
    rng = np.random.default_rng(seed)
    y = rng.random(n) < 0.15
    X = rng.normal(size=(n, d))
    X[y, 0] += 3.0
    X[:, 1] += 1.0
    q = unit(np.array([[1.0, 2.5] + [0.0] * (d - 2)]))[0]
    return unit(X), y.astype(np.int8)[:, None], q


def recording(Y, library, asked):
    ask = oracle.answers(Y, library, 0)
    return lambda rows: asked.extend(np.asarray(rows).tolist()) or ask(rows)


def until_stop(X, q, ask):
    shown, counts = [], []
    for rows, ticks in session.grids(X, q, ask):
        shown.append(rows)
        counts.append(int(ticks.sum()))
        if session.stops(counts):
            return np.concatenate(shown)
    return np.concatenate(shown)


class SessionTests(unittest.TestCase):
    def test_session_never_reads_labels_of_unshown_pages(self):
        X, Y, q = world()
        library = np.arange(len(X))
        asked = []
        shown = until_stop(X, q, recording(Y, library, asked))
        self.assertLess(len(shown), len(X))
        self.assertEqual(sorted(asked), sorted(shown.tolist()))
        self.assertEqual(len(asked), len(set(asked)))
        flipped = Y.copy()
        unshown = np.setdiff1d(library, shown)
        flipped[unshown] = 1 - flipped[unshown]
        again = until_stop(X, q, oracle.answers(flipped, library, 0))
        np.testing.assert_array_equal(shown, again)

    def test_feedback_beats_the_bare_query(self):
        X, Y, q = world()
        positives = int(Y.sum())
        cost = {
            feedback: retrieval.fit_metrics(
                retrieval.replay(X, q, oracle.answers(Y, np.arange(len(X)), 0), positives, feedback), positives
            )["pages_80"]
            for feedback in (True, False)
        }
        self.assertLess(cost[True], cost[False])

    def test_stop_rule(self):
        self.assertTrue(session.stops([5, 1]))
        self.assertFalse(session.stops([2] * (session.CAP - 1)))
        self.assertTrue(session.stops([2] * session.CAP))


class HeldOutTests(unittest.TestCase):
    def test_held_out_labels_and_vectors_cannot_change_the_session_or_threshold(self):
        X, Y, q = world(n=340)
        library, held = np.arange(40, 340), np.arange(40)
        traces, thresholds = [], []
        changed_X, changed_Y = X.copy(), Y.copy()
        changed_X[held] *= -100
        changed_Y[held] = 1 - changed_Y[held]
        for docs, labels in ((X, Y), (changed_X, changed_Y)):
            trace = []
            with patch.object(retrieval, "best_threshold", wraps=retrieval.best_threshold) as threshold:
                retrieval.run_arm(labels, ["test"], docs, q[None], [(library, held)], held, True, trace, "test")
            traces.append(trace)
            thresholds.append(threshold.call_args.args)
            for grid in trace[0]["grids"]:
                self.assertTrue(set(grid["rows"]).issubset(set(library)))
        self.assertEqual(traces[0], traces[1])
        for before, after in zip(*thresholds, strict=True):
            np.testing.assert_array_equal(before, after)

    def test_threshold_and_saved_vector_use_only_the_stop_prefix(self):
        X = np.array([[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]])
        q = X[0]
        rows, ticks = np.array([0, 1]), np.array([True, False])
        trace = retrieval.Trace([rows, np.array([2, 3])], [ticks, np.array([False, True])], 1)
        docs = X[[0, 2]]
        v = session.saved(X, q, rows, ticks)
        with patch.object(retrieval, "best_threshold", return_value=0.2) as threshold:
            scores, pred = retrieval.held_out(trace, X, q, docs, True)
        fit_scores, fit_labels = threshold.call_args.args
        np.testing.assert_array_equal(fit_scores, X[rows] @ v)
        np.testing.assert_array_equal(fit_labels, ticks)
        np.testing.assert_array_equal(scores, docs @ v)
        np.testing.assert_array_equal(pred, scores >= 0.2)

    def test_ap_is_pooled_over_outer_folds_like_supervised_cv(self):
        scores = np.array([0.9, 0.8, 0.7, 0.6, 0.5, 0.4])
        X = np.column_stack([scores, np.zeros(6)])
        Y = np.array([[1], [0], [0], [1], [1], [0]])
        folds = [(np.arange(2, 6), np.arange(2)), (np.arange(2), np.arange(2, 6))]
        _, pooled = retrieval.run_arm(Y, ["test"], X, np.array([[1.0, 0.0]]), folds, np.arange(6), False, [], "test")
        expected = average_precision_score(Y[:, 0], scores)
        fold_mean = np.mean([average_precision_score(Y[b, 0], scores[b]) for _, b in folds])
        self.assertAlmostEqual(expected, pooled["test"]["ap"])
        self.assertNotAlmostEqual(fold_mean, pooled["test"]["ap"])


class CostTests(unittest.TestCase):
    def test_costs_count_whole_grids_and_the_replay_runs_past_the_stop(self):
        X, Y, q = world()
        positives = int(Y.sum())
        trace = retrieval.replay(X, q, oracle.answers(Y, np.arange(len(X)), 0), positives, True)
        m = retrieval.fit_metrics(trace, positives)
        self.assertGreaterEqual(sum(int(t.sum()) for t in trace.ticks), retrieval.need(0.9, positives))
        self.assertEqual(0, m["pages_90"] % session.GRID)
        self.assertLessEqual(m["pages_50"], m["pages_80"])
        self.assertLessEqual(m["pages_80"], m["pages_90"])
        self.assertEqual(m["stop_pages"], session.GRID * m["stop_grids"])
        self.assertAlmostEqual(m["stop_recall"], m["stop_ticks"] / positives)

    def test_random_cost_is_exact(self):
        for n, positives, k, grid in ((7, 3, 2, 2), (7, 3, 3, 20), (8, 1, 1, 3), (5, 5, 4, 2)):
            with self.subTest(n=n, positives=positives, k=k, grid=grid):
                pages, ticks = [], []
                for at in itertools.combinations(range(1, n + 1), positives):
                    seen = min(grid * -(-at[k - 1] // grid), n)
                    pages.append(seen)
                    ticks.append(sum(p <= seen for p in at))
                expected = retrieval.random_cost(n, positives, k, grid)
                np.testing.assert_allclose(expected, (np.mean(pages), np.mean(ticks)))

    def test_zero_positive_sessions_count_towards_stop_totals(self):
        X, _, q = world(n=40)
        Y = np.zeros((40, 2), dtype=np.int8)
        Y[0, 0] = 1
        tags = ["sometimes", "absent"]
        folds = [(np.arange(20), np.arange(20, 40)), (np.arange(20, 40), np.arange(20))]
        rows, pooled = retrieval.run_arm(Y, tags, X, np.stack([q, q]), folds, np.arange(40), True, [], "test")
        means = {t: retrieval.tag_means(rows[t]) for t in tags}
        summary = retrieval.summarise(means, pooled, tags)
        self.assertEqual(40, summary["stop_pages"]["total"])
        self.assertEqual(0.5, summary["stop_ticks"]["total"])
        self.assertEqual(1, summary["tags_scored"])
        self.assertEqual(20, summary["pages_80"]["total"])
        self.assertNotIn("pages_80", means["absent"])
        self.assertNotIn("stop_recall", means["absent"])
        self.assertEqual(0, pooled["absent"]["predicted"])
        self.assertTrue(np.isnan(pooled["absent"]["ap"]))

    def test_random_ap_is_the_exact_finite_sample_expectation(self):
        for n, positives in ((7, 3), (8, 1), (5, 5), (1, 1)):
            with self.subTest(n=n, positives=positives):
                aps = []
                for at in itertools.combinations(range(n), positives):
                    y = np.zeros(n, dtype=np.int8)
                    y[list(at)] = 1
                    aps.append(average_precision_score(y, -np.arange(n)))
                Y = np.zeros((n, 1), dtype=np.int8)
                Y[:positives] = 1
                _, pooled = retrieval.random_arm(Y, ["test"], [], np.arange(n))
                self.assertAlmostEqual(np.mean(aps), pooled["test"]["ap"])


class CliTests(unittest.TestCase):
    def test_cli_outputs_are_private(self):
        def write(paths):
            paths.retrieval.mkdir(parents=True)
            (paths.retrieval / "sessions.jsonl").write_text("{}\n")

        before = os.umask(0o022)
        try:
            with tempfile.TemporaryDirectory() as tmp:
                argv = ["tagger", "--data", tmp, "retrieval"]
                with patch.object(sys, "argv", argv), patch.object(retrieval, "run", side_effect=write):
                    cli.main()
                mode = stat.S_IMODE((Path(tmp) / "retrieval" / "sessions.jsonl").stat().st_mode)
                self.assertEqual(0o600, mode)
        finally:
            os.umask(before)


if __name__ == "__main__":
    unittest.main()
