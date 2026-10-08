"""Synthetic checks of the guided discovery scorer and gate."""

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np
from sklearn.metrics import average_precision_score

from tagger import zeroshot
from tagger.guided import gate, model, oracle
from tagger.heads import unit
from tagger.paths import Paths


def world(seed=3, n=400, d=16):
    """Every page shares a large common direction; the tag is a small shift along one axis, and the
    query points elsewhere. A prototype stays near the common direction; the LR head finds the axis."""
    rng = np.random.default_rng(seed)
    y = rng.random(n) < 0.2
    X = rng.normal(size=(n, d)) * 0.3
    X[:, 0] += 4.0
    X[:, 1] += 1.5 * y
    q = np.eye(d)[2]
    return unit(X), y, q


class ScorerTests(unittest.TestCase):
    def test_blend_lifts_a_misleading_query_to_the_head(self):
        X, y, q = world()
        lib = model.Library.of(X[:300])
        rows = np.r_[np.flatnonzero(y[:300])[:10], np.flatnonzero(~y[:300])[:10]]
        held, yh = X[300:], y[300:]
        blended = average_precision_score(yh, model.fit(lib, q, rows, y[rows]).score(held))
        alone = average_precision_score(yh, model.fit(lib, q, rows, y[rows], blend=False).score(held))
        self.assertGreater(blended, 0.9)
        self.assertLess(alone, 0.7)

    def test_alpha_and_thresholds_follow_the_label_counts(self):
        X, y, q = world()
        lib = model.Library.of(X)
        few = model.fit(lib, q, np.flatnonzero(y)[:5], np.ones(5, bool))
        self.assertIsNone(few.head)
        s = few.score(X)
        self.assertAlmostEqual(s.mean() + zeroshot.ZERO_K_SD * s.std(), few.threshold)
        rows = np.r_[np.flatnonzero(y)[:3], np.flatnonzero(~y)[:7]]
        both = model.fit(lib, q, rows, y[rows])
        self.assertIsNotNone(both.head)
        self.assertAlmostEqual(0.5, both.alpha)

    def test_prototype_is_rocchio(self):
        q, pos, neg = np.eye(3)[0], np.eye(3)[[1, 1]], np.eye(3)[[2]]
        np.testing.assert_allclose(unit(np.array([[1, 1, -0.25]]))[0], zeroshot.prototype(q, pos, neg))
        np.testing.assert_allclose(q, zeroshot.prototype(q, pos[:0], neg[:0]))


class GateTests(unittest.TestCase):
    def test_oracle_answers_only_chosen_pages_and_held_out_labels_never_matter(self):
        X, y, q = world(n=200)
        Y = np.column_stack([y, ~y]).astype(np.int8)
        Q = np.stack([q, unit(-q[None])[0]])
        folds = [(np.arange(150), np.arange(150, 200))]
        asked = []
        real = oracle.answers

        def counting(Y_, library, j):
            ask = real(Y_, library, j)
            return lambda rows: asked.append(len(rows)) or ask(rows)

        with patch.object(oracle, "answers", side_effect=counting):
            first = gate.replay(Y, ["Alpha", "Beta"], X, Q, folds)
        runs = 1 + gate.SEEDS
        self.assertEqual(2 * runs * max(gate.BUDGETS), sum(asked))
        flipped = Y.copy()
        flipped[150:] = 1 - flipped[150:]
        other = gate.replay(flipped, ["Alpha", "Beta"], X, Q, folds)
        for key, m in first.items():
            self.assertEqual(m["positives_per_tag"], other[key]["positives_per_tag"])
        self.assertGreater(first[("doubt", 0, 20)]["macro_ap"], first[("doubt", 0, 0)]["macro_ap"])


class QueryCacheTests(unittest.TestCase):
    def test_cached_queries_need_no_model(self):
        class Encoder:
            def encode(self, queries, **_):
                return np.ones((len(queries), 4))

        with tempfile.TemporaryDirectory() as tmp:
            paths = Paths(Path(tmp))
            first = zeroshot.query_vectors(paths, "eg2", ["Alpha", "Beta"], Encoder())
            with patch.object(zeroshot, "load", side_effect=AssertionError("model loaded")):
                again = zeroshot.query_vectors(paths, "eg2", ["Beta", "Alpha"])
            np.testing.assert_array_equal(first, again)
            self.assertEqual(2, len(list((paths.emb / "eg2" / "queries").glob("*.npy"))))


if __name__ == "__main__":
    unittest.main()
