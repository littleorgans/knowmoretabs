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
        records = []
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
                records.append(search_tag.simulate_fold(labels, X, q, q, a, b, 0))
            runs.append(seen)
        self.assertEqual(len(search_tag.SIZES) * len(q) * len(search_tag.PICKS), len(runs[0]))
        for (picked, first), (again, second) in zip(*runs, strict=True):
            self.assertEqual(picked, again)
            for name in search_tag.SCORERS:
                np.testing.assert_array_equal(first[name], second[name])
        for first, second in zip(*records, strict=True):
            self.assertEqual(first["suggest"], second["suggest"])
            self.assertEqual(first["distractor_tags"], second["distractor_tags"])
            picked = real_picks(Y, np.array(first["rows"]))
            expected = oracle.distractors(
                picked,
                Y.shape[1],
                search_tag.PICKS["distractors"],
                np.random.default_rng([search_tag.SEED, first["fold"], first["n"], first["query"]]),
            )
            np.testing.assert_array_equal(expected, first["distractor_tags"])

    def test_suggestions_use_column_z_scores_and_oracle_picks(self):
        X, Y, q = world(n=80, d=12, tags=12)
        a, b = np.arange(60, 80), np.arange(60)
        rng = np.random.default_rng(5)
        sup = rng.normal(size=(60, 12))
        scales, offsets = np.arange(1, 13), np.arange(12) * 100
        rules = search_tag.Rules(q, np.array([]), None, {name: np.zeros(12) for name in ("zeroshot", "supervised")})
        records = []
        for scores in (sup, sup * scales + offsets):
            with (
                patch.object(search_tag, "fit_rules", return_value=rules),
                patch.object(search_tag.Rules, "scores", return_value={name: scores for name in rules.thresholds}),
                patch.object(search_tag, "rates", return_value=np.zeros(12)),
            ):
                records.append(search_tag.simulate_fold(Y, X, q, q, a, b, 0))
        z = (sup - sup.mean(axis=0)) / sup.std(axis=0)
        for first, second in zip(*records, strict=True):
            self.assertEqual(first["suggest"], second["suggest"])
            top = search_tag.search(X[b], q[first["query"]], first["n"])
            picked = oracle.picks(Y, b[top])
            order = np.argsort(-z[top].mean(axis=0), kind="stable")
            for k in search_tag.SUGGEST_TOP:
                expected = len(set(order[:k]) & set(picked)) / len(picked)
                self.assertAlmostEqual(expected, first["suggest"][f"recall_at_{k}"])


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
    def test_full_vocabulary_flips_include_only_unchosen_owner_tags(self):
        X = np.eye(24)
        Y = np.zeros((24, 6), np.int8)
        Y[:10, 0] = 1  # the only oracle pick
        Y[0, 1:] = 1  # each unpicked tag has one owner positive, including the distractors
        q = np.zeros((6, 24))
        scores = np.zeros((20, 6))
        scores[1, 0] = 1  # one picked true positive, with nine misses
        rules = search_tag.Rules(q, np.array([]), None, {name: np.full(6, 0.5) for name in ("zeroshot", "supervised")})
        with (
            patch.object(search_tag, "SIZES", (20,)),
            patch.object(search_tag, "fit_rules", return_value=rules),
            patch.object(search_tag.Rules, "scores", return_value={name: scores for name in rules.thresholds}),
            patch.object(search_tag, "rates", return_value=np.zeros(6)),
        ):
            sets = search_tag.simulate_fold(Y, X, q, q[:1], np.arange(20, 24), np.arange(20), 0)
        summary = search_tag.summarise(sets)
        record = sets[0]
        np.testing.assert_array_equal(
            oracle.distractors(np.array([0]), 6, 2, np.random.default_rng([search_tag.SEED, 0, 20, 0])),
            record["distractor_tags"],
        )
        for variant in search_tag.PICKS:
            for name in search_tag.SCORERS:
                c = record[variant][name]
                chosen = np.array(list(c["per_tag"]))
                full_pred = np.zeros_like(Y[:20], bool)
                full_pred[:, chosen] = search_tag.decide(
                    {key: scores for key in rules.thresholds}, rules, chosen, np.zeros(6)
                )[name]
                expected = (full_pred != Y[:20]).sum(axis=1)
                self.assertEqual(int(expected.sum()), c["full_vocabulary_flips"])
                self.assertEqual(int((expected == 0).sum()), c["full_vocabulary_zero_flip_pages"])
                self.assertAlmostEqual(expected.mean(), summary[variant][name]["full_vocabulary_flips_per_page"])
                self.assertAlmostEqual(
                    (expected == 0).mean(), summary[variant][name]["full_vocabulary_zero_flip_share"]
                )
        self.assertEqual(5, record["unpicked_owner_tags"])
        self.assertEqual(9, record["oracle"]["supervised"]["flips"])
        self.assertEqual(14, record["oracle"]["supervised"]["full_vocabulary_flips"])
        # Two distractor positives are already counted as misses in the chosen-tag decisions.
        self.assertEqual(11, record["distractors"]["supervised"]["flips"])
        self.assertEqual(14, record["distractors"]["supervised"]["full_vocabulary_flips"])

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
