"""Synthetic checks of ranking semantics, nested cuts and paired C subsets."""

from dataclasses import replace
from unittest.mock import patch

import numpy as np
import pytest

from tagger import tag_calib, tag_eval, tag_grid
from tagger.archive import LibraryState, sha256_hex
from tagger.heads import fit_knn, unit
from tagger.paths import read_json, write_json


def test_rankings_keep_head_thresholds_and_raw_margins():
    tags = ["A", "B", "C", "Rare"]
    scores = np.array([[0.4, 0.9, 0.6, 1.0], [-0.1, 0.2, 0.0, 0.3]])
    view = tag_eval.SuggestionView(
        tag_eval.HeldOutScores(scores, np.zeros(4)),
        np.ones(4),
        tags,
        LibraryState(active={t.lower(): t for t in tags}),
        np.array([True, True, True, False]),
        np.array([0.5, 0.7, 0.1, 0.95]),
        {},
    )
    expected = {
        "app": ["Rare", "B", "C", "A"],
        "heads_only": ["B", "C", "A"],
        "heads_first": ["B", "C", "Rare"],
        "margin": ["C", "B", "Rare", "A"],
    }
    for strategy, names in expected.items():
        assert [h["tag"] for h in tag_eval.rank_suggestions(view, 0, strategy)] == names
    assert tag_eval.rank_suggestions(view, 1, "heads_first") == [{"tag": "Rare", "z": 0.3}]
    np.testing.assert_array_equal(view.S.training_mean, np.zeros(4))


def test_knn_fits_tagged_training_rows_and_uses_raw_shares(corpus):
    train, held = np.arange(10, 80), np.arange(10)
    X, Y = corpus.X[train], corpus.Y[train]
    with patch.object(tag_eval, "fit_knn", wraps=fit_knn) as fit:
        view = tag_eval.STRATEGIES["knn"](X, Y, corpus.q, corpus.tags, corpus.X[held])
    np.testing.assert_array_equal(fit.call_args.args[2], np.flatnonzero(Y.any(axis=1)))
    order = [corpus.tags.index(t) for t in view.tags]
    reference = fit_knn(tag_eval.Dense("train", X), Y[:, order], np.flatnonzero(Y.any(axis=1)))
    np.testing.assert_allclose(view.S.values, reference.predict_matrix(corpus.X[held]))
    np.testing.assert_array_equal(view.S.training_mean, np.zeros(len(order)))
    np.testing.assert_array_equal(view.spread, np.ones(len(order)))
    assert view.params == reference.params
    changed = tag_eval.STRATEGIES["knn"](X, Y, corpus.q, corpus.tags, -100 * corpus.X[held])
    assert changed.params == view.params
    np.testing.assert_array_equal(changed.thresholds, view.thresholds)


def test_fusion_uses_unit_sum_and_preserves_coverage(corpus):
    covered = np.arange(len(corpus.X)) % 2 == 0
    X = corpus.X.copy()
    X[covered] = unit(np.roll(X[covered], 1, axis=1))
    cmean = replace(corpus, X=X, chunk_coverage=covered)
    bc = tag_eval.bc_corpus(corpus, cmean)
    np.testing.assert_allclose(bc.X, unit(corpus.X + X))
    np.testing.assert_allclose(bc.X[~covered], corpus.X[~covered])
    assert bc.chunk_coverage is covered
    assert bc.metadata["input"] == "bc"


def test_cut_keeps_ties_and_matches_training_volume():
    scores = [1, 1, 2, 3, 4, 5]
    cut = tag_calib.choose_cut(scores, 2, 1.5)
    assert cut == {"cut": 3.0, "training_shown_per_page": 1.5}
    tied = tag_calib.choose_cut([1, 1, 2, 2], 2, 1.5)
    assert tied == {"cut": 2.0, "training_shown_per_page": 1.0}
    assert tag_calib.choose_cut([1, 2], 2, 0)["cut"] > 2
    with pytest.raises(tag_eval.Invalid, match="finite"):
        tag_calib.choose_cut([float("nan")], 2, 1)


def test_nested_cuts_cannot_read_outer_vectors_or_labels(corpus, tmp_path):
    saved = tag_eval.page_split(corpus, tmp_path / "split.json")
    boundary = next(tag_eval.fold_indices(corpus, saved))
    fold, _, train, held = boundary
    with (
        patch.object(tag_eval, "fold_indices", return_value=[boundary]),
        patch.object(tag_calib, "fit_views", wraps=tag_calib.fit_views) as fit,
    ):
        first = tag_calib.training_cuts(corpus, saved, ["app", "margin", "knn"])
    assert fit.call_count == 3
    for call in fit.call_args_list:
        _, fitting, validating, _ = call.args
        assert not set(held).intersection(fitting)
        assert not set(held).intersection(validating)
        assert not set(fitting).intersection(validating)
        assert set(fitting).union(validating) == set(train)
    changed = replace(corpus, X=corpus.X.copy(), Y=corpus.Y.copy())
    changed.X[held] *= -100
    changed.Y[held] = 1 - changed.Y[held]
    with patch.object(tag_eval, "fold_indices", return_value=[boundary]):
        again = tag_calib.training_cuts(changed, saved, ["app", "margin", "knn"])
    assert again == first
    assert first["app"][0]["fold"] == fold
    assert sum(len(f["validation_ids"]) for f in first["app"][0]["inner_folds"]) == int(
        corpus.Y[train].any(axis=1).sum()
    )


def test_abstention_and_covered_metrics_recompute_without_reranking(corpus, tmp_path):
    saved = tag_eval.page_split(corpus, tmp_path / "split.json")
    row = tag_eval.cross_validate(corpus, saved, "app")
    row.update(page_source="app", strategy="app")
    cuts = {
        "app": [
            {
                "fold": f["fold"],
                "operating_points": [
                    {"target": target, "cut": cut}
                    for target, cut in zip(tag_calib.TARGETS, [1e9, 0, -1e9], strict=True)
                ],
            }
            for f in row["folds"]
        ]
    }
    points = tag_calib.abstention(corpus, {"app/app": row}, cuts)
    empty = points["app/app@1.0"]
    assert empty["none_shown_share"] == 1
    assert empty["shown_per_page"] == 0
    assert all(v == 0 for v in empty["metrics"]["overall"]["micro"].values())
    assert points["app/app@2.0"]["metrics"] == row["metrics"]
    covered = {sha256_hex(corpus.records[i]["key"]) for i in corpus.labelled[::2]}
    subset = tag_calib.with_predictions(corpus, row, [p for p in row["predictions"] if p["id"] in covered])
    assert subset["metrics"]["overall"]["counts"]["pages"] == len(covered)
    assert {p["id"] for p in subset["predictions"]} == covered
    for page in subset["predictions"]:
        assert page in row["predictions"]


def test_bootstrap_partial_suggestions_matches_supported_page_precision():
    def row(top):
        return {
            "predictions": [
                {"id": str(i), "fold": 0, "truth": ["A"], "top_3": [{"tag": t} for t in tags]}
                for i, tags in enumerate(top)
            ]
        }

    candidate, baseline = row([["A"], []]), row([["A", "B", "C"], ["B", "C", "D"]])
    paired = tag_grid.paired_bootstrap(candidate, baseline)
    assert paired["precision_at_3"]["difference"] == pytest.approx(1 - 1 / 6)
    assert paired["recall_at_3"]["difference"] == 0
    assert paired == tag_grid.paired_bootstrap(candidate, baseline)


def test_full_study_runner_and_frozen_outputs(corpus, tmp_path):
    corpus.metadata.update(vectors_digest="vectors", queries_digest="queries", descriptions_digest="descriptions")
    root = tmp_path / "archive"
    root.mkdir()
    paths = tag_eval.Paths(root / "tagger")
    paths.data.mkdir()
    split = tmp_path / "split.json"
    saved = tag_eval.page_split(corpus, split)
    baseline = {
        **tag_eval.cross_validate(corpus, saved, "app"),
        "split_digest": tag_eval.fingerprint(saved),
        "page_source": "app",
        "strategy": "app",
        "k": tag_eval.K,
        "seed": tag_eval.SEED,
        "inputs": corpus.metadata,
    }
    baseline_path = tmp_path / "baseline.json"
    write_json(baseline_path, baseline)
    coverage = np.arange(len(corpus.records)) % 2 == 0
    cmean = replace(corpus, chunk_coverage=coverage)
    before = tag_calib.input_snapshot(root, paths.data), split.read_bytes()
    with patch.object(tag_eval, "app_vectors", return_value=corpus), patch.object(
        tag_eval, "cmean_corpus", return_value=cmean
    ):
        result = tag_calib.run(paths, root, split, tmp_path, baseline_path)
    assert read_json(tmp_path / "calib.json") == result
    assert "synthetic-page-" not in (tmp_path / "calib.json").read_text()
    assert len((tmp_path / "calib.md").read_text().splitlines()) <= 40
    assert len(result["rows"]) == 5
    assert len(result["fair_c"]) == 4
    assert len(result["abstention"]) == 3 * len(tag_calib.CURVE_TARGETS)
    assert result["fair_c"]["app/app covered"]["metrics"] == result["fair_c"]["cmean/app covered"]["metrics"]
    assert result["best_row"] != "ceiling"
    assert (tag_calib.input_snapshot(root, paths.data), split.read_bytes()) == before
    with pytest.raises(tag_eval.Invalid, match="already exist"):
        tag_calib.run(paths, root, split, tmp_path, baseline_path)
