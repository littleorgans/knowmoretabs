"""Synthetic checks of the evaluation and suggestion boundaries."""

import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np
from joblib import Parallel

from tagger import evaluate, features, heads, suggest
from tagger.paths import Paths, read_json, write_json


class ProtocolTests(unittest.TestCase):
    def setUp(self):
        rng = np.random.default_rng(7)
        self.Y = rng.integers(0, 2, (90, 3), dtype=np.int8)
        self.lab = evaluate.Labels(
            [{"labelled": i < 87} for i in range(90)],
            ["Alpha", "Beta", "Gamma"],
            self.Y,
            np.arange(87),
        )
        self.feat = features.Dense("synthetic", rng.normal(size=(90, 8)))

    def test_final_is_reused_without_reading_test_rows(self):
        with tempfile.TemporaryDirectory() as tmp:
            paths = Paths(Path(tmp))
            saved = {"best": "synthetic-B-lr", "test": {}}
            write_json(paths.eval / "final.json", saved)
            before = (paths.eval / "final.json").read_bytes()
            metric = {"micro": {}, "macro": {"f1": 1, "ap": 1}, "fold_macro_f1": [1]}
            with (
                patch.object(evaluate, "labels", return_value=self.lab),
                patch.object(evaluate, "split", return_value={"train": [0], "test": [1]}),
                patch.object(evaluate, "grid", return_value=[saved["best"]]),
                patch.object(evaluate, "cross_validate", return_value=metric),
                patch.object(evaluate, "score_test_once", return_value={}) as score,
                contextlib.redirect_stdout(io.StringIO()),
            ):
                evaluate.run(paths, final=True)
            score.assert_not_called()
            self.assertEqual(before, (paths.eval / "final.json").read_bytes())
            self.assertEqual(saved, read_json(paths.eval / "final.json"))

    def test_changed_pick_cannot_rescore_test(self):
        with tempfile.TemporaryDirectory() as tmp:
            paths = Paths(Path(tmp))
            write_json(paths.eval / "final.json", {"best": "old-B-lr", "test": {}})
            metric = {"micro": {}, "macro": {"f1": 1, "ap": 1}, "fold_macro_f1": [1]}
            with (
                patch.object(evaluate, "labels", return_value=self.lab),
                patch.object(evaluate, "split", return_value={"train": [0], "test": [1]}),
                patch.object(evaluate, "grid", return_value=["new-B-lr"]),
                patch.object(evaluate, "cross_validate", return_value=metric),
                patch.object(evaluate, "score_test_once") as score,
                contextlib.redirect_stdout(io.StringIO()),
            ):
                with self.assertRaises(SystemExit):
                    evaluate.run(paths, final=True)
            score.assert_not_called()

    def test_seeded_outer_and_inner_folds_are_nested(self):
        pool = np.arange(75)
        folds = evaluate.outer_folds(self.lab, pool)
        again = evaluate.outer_folds(self.lab, pool)
        seen = []
        for (a, b), (aa, bb) in zip(folds, again, strict=True):
            np.testing.assert_array_equal(a, aa)
            np.testing.assert_array_equal(b, bb)
            self.assertFalse(set(a) & set(b))
            self.assertEqual(set(pool), set(a) | set(b))
            seen.extend(b)
            inner_seen = []
            for x, y in heads.inner_folds(self.Y, a):
                self.assertFalse(set(x) & set(y))
                self.assertFalse((set(x) | set(y)) & set(b))
                self.assertEqual(set(a), set(x) | set(y))
                inner_seen.extend(y)
            self.assertEqual(sorted(a), sorted(inner_seen))
        self.assertEqual(sorted(pool), sorted(seen))

    def test_lr_and_knn_selection_ignore_labels_outside_fit_rows(self):
        rows = np.arange(75)
        changed = self.Y.copy()
        changed[75:] = 1 - changed[75:]
        for fit in (heads.fit_lr, heads.fit_knn):
            with self.subTest(head=fit.__name__), patch.object(
                heads, "Parallel", side_effect=lambda **kw: Parallel(n_jobs=1)
            ):
                first = fit(self.feat, self.Y, rows)
                second = fit(self.feat, changed, rows)
            self.assertEqual(first.params, second.params)
            np.testing.assert_array_equal(first.thresholds, second.thresholds)
            np.testing.assert_array_equal(
                first.predict(self.feat, np.arange(75, 90)), second.predict(self.feat, np.arange(75, 90))
            )

    def test_dense_scaling_ignores_apply_rows(self):
        rows, apply = np.arange(75), np.arange(75, 90)
        Xa, _, scaler = heads._scaled(self.feat, rows, apply)
        changed = self.feat.X.copy()
        changed[apply] = 1e6
        Xb, _, other = heads._scaled(features.Dense("synthetic", changed), rows, apply)
        np.testing.assert_array_equal(Xa, Xb)
        np.testing.assert_array_equal(scaler.mean_, other.mean_)
        np.testing.assert_allclose(scaler.mean_, self.feat.X[rows].mean(axis=0))

    def test_suggestions_exclude_each_pages_labels_from_training(self):
        calls = []

        def score(feat, head, lab, a, b):
            calls.append((set(a), set(b)))
            shape = (len(b), len(lab.tags))
            return heads.Fitted(np.zeros(3), {}, a, None, None), np.ones(shape), np.ones(shape, bool)

        with (
            patch.object(suggest, "feature", return_value=(self.feat, "lr")),
            patch.object(suggest, "fit_and_score", side_effect=score),
        ):
            scores, pred, thresholds = suggest.predictions(None, self.lab, "synthetic-B-lr")
        self.assertTrue(np.isfinite(scores).all())
        self.assertTrue(pred.all())
        self.assertTrue(np.isfinite(thresholds).all())
        for a, b in calls:
            self.assertFalse(a & b)
        self.assertEqual(set(self.lab.labelled), set.union(*(b for _, b in calls[:-1])))
        self.assertEqual(set(self.lab.labelled), calls[-1][0])
        self.assertEqual({87, 88, 89}, calls[-1][1])


if __name__ == "__main__":
    unittest.main()
