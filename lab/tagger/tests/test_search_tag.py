"""Synthetic checks of test S2: step 3 never reads a result page's label, and the flip and pick counts."""

import unittest
from unittest.mock import patch

import numpy as np

from tagger.guided import oracle, search_tag
from tagger.heads import unit


def world(seed=11, n=260, d=12, tags=3):
    """Each tag shifts its pages along its own axis; its query points there too."""
    rng = np.random.default_rng(seed)
    Y = (rng.random((n, tags)) < 0.25).astype(np.int8)
    X = rng.normal(size=(n, d))
    X[:, :tags] += 2.5 * Y
    return unit(X), Y, np.eye(d)[:tags]


class LeakageTests(unittest.TestCase):
    def test_step_three_never_reads_result_page_labels(self):
        X, Y, q = world()
        a, b = np.arange(80, 260), np.arange(80)
        real_picks, real_decide = oracle.picks, search_tag.decide
        # the user's picks stay those of the true labels; `rates` passes fit fold labels, which never change
        picks = lambda Y_, rows, *args: real_picks(Y if len(Y_) == len(Y) else Y_, rows, *args)  # noqa: E731
        runs = []
        for labels in (Y, np.where(np.isin(np.arange(len(Y)), b)[:, None], 1 - Y, Y)):
            seen = []

            def spy(scores, rules, picked, rate, seen=seen):
                out = real_decide(scores, rules, picked, rate)
                seen.append((picked.tolist(), out))
                return out

            with (
                patch.object(search_tag.oracle, "picks", side_effect=picks),
                patch.object(search_tag, "decide", side_effect=spy),
            ):
                search_tag.simulate_fold(labels, X, q, q, a, b, 0)
            runs.append(seen)
        self.assertEqual(len(search_tag.SIZES) * len(q) * len(search_tag.PICKS), len(runs[0]))
        for (picked, first), (again, second) in zip(*runs, strict=True):
            self.assertEqual(picked, again)
            for name in search_tag.SCORERS:
                np.testing.assert_array_equal(first[name], second[name])


class DecideTests(unittest.TestCase):
    def test_thresholds_and_the_in_set_rate(self):
        scores = {
            "zeroshot": np.array([[0.9, 0.1], [0.5, 0.2], [0.1, 0.3], [0.2, 0.4]]),
            "supervised": np.array([[3.0, -1.0], [2.0, -2.0], [-1.0, -3.0], [1.0, -4.0]]),
        }
        rules = search_tag.Rules(
            None, np.array([]), None, {"zeroshot": np.array([0.4, 0.35]), "supervised": np.zeros(2)}
        )
        out = search_tag.decide(scores, rules, np.array([0, 1]), np.array([0.6, 0.0]))
        np.testing.assert_array_equal(out["zeroshot"], [[1, 0], [1, 0], [0, 0], [0, 1]])
        np.testing.assert_array_equal(out["supervised"], [[1, 0], [1, 0], [0, 0], [1, 0]])
        # round(0.6 x 4) = 2 pages for the first tag; at least 1 for the second
        np.testing.assert_array_equal(out["in_set"], [[1, 1], [1, 0], [0, 0], [0, 0]])

    def test_rates_replay_the_pick_on_fit_slices(self):
        X = unit(np.array([[1.0, 0.0]] * 6 + [[0.0, 1.0]] * 6))
        Y = np.zeros((12, 2), np.int8)
        Y[:3, 0] = 1  # 3 of the top 6 for the first query
        rate = search_tag.rates(X, Y, np.array([[1.0, 0.0]]), 6, 12, np.random.default_rng(0))
        np.testing.assert_allclose(rate, [0.5, 0.5])  # the second tag is never picked: the global median


class CountTests(unittest.TestCase):
    def test_flips_and_zero_flip_pages(self):
        pred = np.array([[1, 0], [1, 1], [0, 0]], bool)
        truth = np.array([[1, 0], [0, 1], [1, 0]])
        c = search_tag.counts(pred, truth, np.array([4, 7]))
        self.assertEqual((2, 1, 1, 2, 1), (c["tp"], c["fp"], c["fn"], c["flips"], c["zero_flip_pages"]))
        self.assertEqual({4: [1, 1, 1], 7: [1, 0, 0]}, c["per_tag"])
        agg = search_tag.aggregate([c, c])
        self.assertAlmostEqual(2 / 3, agg["flips_per_page"])
        self.assertAlmostEqual(2 / 3, agg["micro_f1"])
        self.assertAlmostEqual((0.5 + 1.0) / 2, agg["macro_f1"])

    def test_picks_suggestions_and_distractors(self):
        Y = np.array([[1, 1, 0], [1, 1, 0], [1, 0, 1], [0, 1, 0]], np.int8)
        picked = oracle.picks(Y, np.arange(4))
        np.testing.assert_array_equal([0, 1], picked)
        wrong = oracle.distractors(picked, 5, 2, np.random.default_rng(0))
        self.assertEqual(2, len(set(wrong)))
        self.assertFalse(set(wrong) & set(picked))
        z = np.array([[0.0, 2.0, 1.0], [0.0, 2.0, 1.0]])
        with patch.object(search_tag, "SUGGEST_TOP", (1, 2)):
            self.assertEqual({"recall_at_1": 0.5, "recall_at_2": 0.5}, search_tag.suggest_recall(z, picked))


if __name__ == "__main__":
    unittest.main()
