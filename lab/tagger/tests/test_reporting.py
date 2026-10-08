"""Synthetic checks that reported claims match the emitted predictions and algorithm."""

import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np

from tagger import evaluate, report, suggest
from tagger.paths import Paths, read_json, write_json


class ReportingTests(unittest.TestCase):
    def test_suggestion_precision_is_an_oof_proxy_with_export_tag_weights(self):
        lab = evaluate.Labels(
            [
                {"key": "page-alpha", "title": "Synthetic alpha", "tags": ["Alpha"], "labelled": True},
                {"key": "page-beta", "title": "Synthetic beta", "tags": ["Beta"], "labelled": True},
                {"key": "page-gamma", "title": "Synthetic gamma", "tags": [], "labelled": False},
            ],
            ["Alpha", "Beta"],
            np.array([[1, 0], [0, 1], [0, 0]], dtype=np.int8),
            np.array([0, 1]),
        )
        pred = np.array([[1, 1], [0, 1], [1, 0]], bool)
        with tempfile.TemporaryDirectory() as tmp:
            paths = Paths(Path(tmp))
            write_json(paths.eval / "final.json", {"best": "synthetic-B-lr"})
            with (
                patch.object(suggest, "labels", return_value=lab),
                patch.object(suggest, "predictions", return_value=(pred.astype(float), pred, np.zeros((3, 2)))),
                patch.object(suggest, "dry_run", return_value={"exit_code": 0, "report": {}}),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                suggest.run(paths)
            summary = read_json(paths.out / "suggest-summary.json")
            self.assertEqual(2, summary["suggested_associations"])
            self.assertAlmostEqual(2 / 3, summary["oof_micro_precision_proxy"])
            self.assertAlmostEqual(0.75, summary["suggestion_tag_weighted_precision_proxy"])

    def test_cluster_report_names_the_algorithm_actually_used(self):
        with tempfile.TemporaryDirectory() as tmp:
            paths = Paths(Path(tmp))
            best = "eg2-B-lr"
            metric = {
                "micro": dict(precision=1, recall=1, f1=1, ap=1),
                "macro": dict(precision=1, recall=1, f1=1, ap=1),
                "per_tag": {},
                "params": [{"C": [0.1]}],
            }
            values = {
                "dataset/summary.json": dict.fromkeys(
                    (
                        "known_pages",
                        "labelled_pages",
                        "owner_associations",
                        "labelled_text_ok",
                        "labelled_image_ok",
                        "labelled_both_ok",
                        "labelled_with_metadata",
                    ),
                    4,
                ),
                "dataset/tags.json": {"trained": ["Alpha"], "not_trained": [], "positives": {}},
                "eval/cv_summary.json": {
                    "configs": {best: {**metric, "fold_macro_f1_std": 0}},
                    "test_pages": 1,
                    "train_pages": 3,
                },
                "eval/final.json": {"best": best, "test": {best: metric}},
                f"eval/cv/{best}.json": metric,
                "out/suggest-summary.json": {
                    "source": "synthetic",
                    "suggested_associations": 0,
                    "pages_with_suggestions": 0,
                    "labelled_pages_with_suggestions": 0,
                    "dry_run": {"exit_code": 0},
                    "oof_micro_precision_proxy": 1,
                    "suggestion_tag_weighted_precision_proxy": None,
                },
                "out/cost.json": {"machine": "Synthetic", "memory_gb": 1, "python": "3.12", "models": {}},
                "out/clusters.json": {
                    "config": best,
                    "pages": 4,
                    "k": 2,
                    "silhouette": {"2": 0.1},
                    "clusters": [],
                    "tags": [],
                    "summary": {
                        "no_match_jaccard_below": 0.1,
                        "clusters_without_matching_tag": 0,
                        "tags_split": 0,
                        "mean_best_cluster_f1_trained": 1,
                    },
                },
                "out/zeroshot.json": {},
            }
            for name, value in values.items():
                write_json(paths.data / name, value)
            with patch.object(report, "zeroshot_section", return_value=[]), contextlib.redirect_stdout(io.StringIO()):
                report.run(paths)
            text = (paths.out / "results.md").read_text()
            self.assertIn("K-means on unit rows", text)


if __name__ == "__main__":
    unittest.main()
