"""Synthetic checks of zero shot selection and the label budget."""

import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np

from tagger import evaluate, zeroshot
from tagger.heads import unit
from tagger.paths import Paths, read_json, write_json


class ZeroShotTests(unittest.TestCase):
    def setUp(self):
        self.docs = unit(np.random.default_rng(7).normal(size=(60, 8)))
        self.q = self.docs[:3]
        self.Y = np.eye(3, dtype=np.int8)[np.arange(60) % 3]
        self.lab = evaluate.Labels([], ["Alpha", "Beta", "Gamma"], self.Y, np.arange(60))

    def test_zero_shot_can_run_before_test_scoring(self):
        with tempfile.TemporaryDirectory() as tmp:
            paths = Paths(Path(tmp))
            write_json(paths.eval / "cv_summary.json", {"best": "eg2-B-lr"})
            write_json(paths.data / "zeroshot/descriptions.json", {})
            (paths.emb / "eg2").mkdir(parents=True)
            np.save(paths.emb / "eg2/B.npy", self.docs)
            with (
                patch.object(zeroshot, "labels", return_value=self.lab),
                patch.object(zeroshot, "split", return_value={"train": list(range(60))}),
                patch.object(zeroshot, "MODELS", {"eg2": object()}),
                patch.object(zeroshot, "load", return_value=(object(), {})),
                patch.object(zeroshot, "release"),
                patch.object(zeroshot, "encode_queries", return_value=self.q),
                patch.object(zeroshot, "query_vectors", return_value=self.q),
                patch.object(zeroshot, "query_latency", return_value={}),
                patch.object(zeroshot, "score_sets", return_value={"B": self.docs @ self.q.T}),
                patch.object(zeroshot, "search_ms", return_value=0),
                patch.object(zeroshot, "curve", return_value={}),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                zeroshot.run(paths)
            self.assertFalse((paths.eval / "final.json").exists())
            self.assertEqual("B", read_json(paths.out / "zeroshot.json")["curve"]["input"])

    def test_curve_training_and_thresholds_use_only_fit_labels(self):
        a, b = np.arange(45), np.arange(45, 60)
        captured = []
        thresholds = []
        real_evaluate = zeroshot.evaluate
        real_threshold = zeroshot.best_threshold

        def metrics(Y, scores, pred, tags):
            captured.append((scores.copy(), pred.copy()))
            return real_evaluate(Y, scores, pred, tags)

        def threshold(scores, y):
            thresholds.append(y.copy())
            return real_threshold(scores, y)

        with (
            patch.object(zeroshot, "outer_folds", return_value=[(a, b)]),
            patch.object(zeroshot, "KS", (0, 5)),
            patch.object(zeroshot, "REPEATS", 1),
            patch.object(zeroshot, "evaluate", side_effect=metrics),
            patch.object(zeroshot, "best_threshold", side_effect=threshold),
        ):
            first = zeroshot.curve(self.lab, self.docs, self.q, b)
            changed = self.Y.copy()
            changed[b] = 1 - changed[b]
            other = evaluate.Labels([], self.lab.tags, changed, np.arange(60))
            zeroshot.curve(other, self.docs, self.q, b)
        for i in range(2):
            np.testing.assert_array_equal(captured[i][0], captured[i + 2][0])
            np.testing.assert_array_equal(captured[i][1], captured[i + 2][1])
        zero_ap = zeroshot.macro_ap(self.Y[b], self.docs[b] @ self.q.T)[0]
        self.assertAlmostEqual(zero_ap, first[0]["macro_ap_mean"])
        for y in thresholds:
            self.assertEqual(15, len(y))
            self.assertEqual(5, int(y.sum()))
            self.assertEqual(10, int((y == 0).sum()))


if __name__ == "__main__":
    unittest.main()
