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
        n, positives, k, grid = 7, 3, 2, 2
        pages, ticks = [], []
        for at in itertools.combinations(range(1, n + 1), positives):
            seen = min(grid * -(-at[k - 1] // grid), n)
            pages.append(seen)
            ticks.append(sum(p <= seen for p in at))
        expected = retrieval.random_cost(n, positives, k, grid)
        np.testing.assert_allclose(expected, (np.mean(pages), np.mean(ticks)))


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
