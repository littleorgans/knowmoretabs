"""Synthetic proof of the page split, leakage boundary and global top three metrics."""

import copy
import json
from unittest.mock import patch

import numpy as np
import pytest

from tagger import cli, tag_eval
from tagger.app import engine
from tagger.guided.search_tag import MIN_HEAD_POSITIVES
from tagger.heads import unit
from tagger.paths import read_json, write_json


@pytest.fixture
def corpus():
    tags = ["Common", "Other", "Boundary", "Rare"]
    Y = np.zeros((80, len(tags)), np.int8)
    Y[:30, 0] = 1
    Y[20:45, 1] = 1
    Y[42:52, 2] = 1
    Y[54:56, 3] = 1
    X = np.random.default_rng(12).normal(size=(80, 6))
    X[:, : len(tags)] += Y * 3
    records = [
        {"key": f"synthetic-page-{i}", "tags": [t for j, t in enumerate(tags) if Y[i, j]], "forgotten": False}
        for i in range(len(Y))
    ]
    return tag_eval.Corpus(
        records, tags, Y, np.flatnonzero(Y.any(axis=1)), unit(X), np.eye(6)[: len(tags)], {"input": "synthetic"}
    )


def test_split_is_once_by_key_and_validates_drift(corpus, tmp_path):
    path = tmp_path / "split.json"
    split = tag_eval.page_split(corpus, path)
    before = (path.read_bytes(), path.stat().st_mtime_ns)
    assert tag_eval.page_split(corpus, path) == split
    assert (path.read_bytes(), path.stat().st_mtime_ns) == before
    assert {p["key"] for p in split["pages"]} == {r["key"] for r in corpus.records if r["tags"]}
    assert {p["fold"] for p in split["pages"]} == set(range(5))
    fresh = tag_eval.page_split(corpus, tmp_path / "repeat.json")
    assert fresh == split
    bad = copy.deepcopy(split)
    bad["pages"][0] = bad["pages"][1]
    write_json(path, bad)
    with pytest.raises(tag_eval.Invalid, match="assignments"):
        tag_eval.page_split(corpus, path)
    write_json(path, split)
    corpus.records[0]["tags"] = ["Rare"]
    with pytest.raises(tag_eval.Invalid, match="provenance"):
        tag_eval.page_split(corpus, path)


def test_app_scorer_uses_training_only_and_app_ranking(corpus):
    train, held = np.arange(10, 80), np.arange(10)
    X, Y = corpus.X[train], corpus.Y[train]
    with patch.object(tag_eval, "fit_rules", wraps=tag_eval.fit_rules) as fit:
        scorer = tag_eval.app_scorer(X, Y, corpus.q, corpus.tags, corpus.X[held])
    np.testing.assert_array_equal(fit.call_args.args[0], X[Y.any(axis=1)])
    np.testing.assert_array_equal(scorer.heads, Y.sum(axis=0) >= MIN_HEAD_POSITIVES)
    order = [corpus.tags.index(t) for t in scorer.tags]
    rules = tag_eval.fit_rules(X[Y.any(axis=1)], Y[Y.any(axis=1)][:, order], corpus.q[order])
    reference = rules.scores(X)["supervised"]
    np.testing.assert_allclose(scorer.S.mean(axis=0), reference.mean(axis=0))
    np.testing.assert_allclose(scorer.spread, engine._spread(reference))
    values = rules.scores(corpus.X[held])["supervised"]
    np.testing.assert_allclose(scorer.S.values, values)
    expected = (values - reference.mean(axis=0)) / engine._spread(reference)
    for i in range(len(held)):
        ranked = engine.suggest(scorer, [i], k=len(corpus.tags))
        assert [h["tag"] for h in ranked] == [scorer.tags[j] for j in np.argsort(-expected[i], kind="stable")]
        np.testing.assert_allclose([h["z"] for h in ranked], sorted(expected[i], reverse=True))
    # A different held out vector changes its own score, never the training reference.
    changed = tag_eval.app_scorer(X, Y, corpus.q, corpus.tags, corpus.X[held] * 100)
    np.testing.assert_array_equal(changed.S.training_mean, scorer.S.training_mean)
    np.testing.assert_array_equal(changed.spread, scorer.spread)
    np.testing.assert_array_equal(changed.thresholds, scorer.thresholds)
    # An unseen training tag receives the app's zero shot threshold on training rows.
    unseen_Y = Y.copy()
    unseen_Y[:, 3] = 0
    unseen = tag_eval.app_scorer(X, unseen_Y, corpus.q, corpus.tags, corpus.X[held])
    assert unseen.thresholds[3] == pytest.approx(engine._zero_shot_threshold((X @ corpus.q[3])[:, None])[0])


def test_macro_micro_and_group_metrics_do_not_rerank():
    Y = np.array([[1, 0, 0, 0], [1, 1, 1, 1]], bool)
    pred = np.array([[1, 1, 1, 0], [1, 1, 0, 1]], bool)
    heads = np.array([[1, 1, 0, 0], [1, 1, 0, 0]], bool)
    scored = tag_eval.summaries(Y, pred, heads, heads[0])
    overall = scored["overall"]
    assert overall["macro"] == pytest.approx({"precision_at_3": 2 / 3, "recall_at_3": 0.875, "hit_at_3": 1})
    assert overall["micro"] == pytest.approx({"precision_at_3": 2 / 3, "recall_at_3": 0.8, "hit_at_3": 1})
    assert overall["tag_macro"] == pytest.approx({"precision_at_3": 0.625, "recall_at_3": 0.75})
    for field in ("tp", "fp", "fn", "positives", "predictions"):
        assert sum(g["counts"][field] for g in scored["by_fold_head"].values()) == overall["counts"][field]
    fallback = scored["by_fold_head"]["fallback"]
    assert fallback["micro"]["precision_at_3"] == 0.5
    assert fallback["micro"]["recall_at_3"] == 0.5
    assert fallback["micro"]["hit_at_3"] == 1
    empty = tag_eval.measure(Y, pred, np.zeros(4, bool))
    assert all(v == 0 for v in empty["micro"].values())


def test_real_five_fold_path_and_pluggable_strategy(corpus, tmp_path):
    saved = tag_eval.page_split(corpus, tmp_path / "split.json")
    with patch.object(tag_eval.engine, "suggest", wraps=engine.suggest) as suggest:
        result = tag_eval.cross_validate(corpus, saved, "app")
    assert suggest.call_count == len(corpus.labelled)
    assert result["counts"]["evaluation_pages"] == len(corpus.labelled)
    assert len({p["id"] for p in result["predictions"]}) == len(corpus.labelled)
    assert result["per_tag"]["Boundary"]["has_head_full"]
    assert all(not f["has_head"] for f in result["per_tag"]["Boundary"]["folds"])
    assert all(f["fitting_pages"] + f["evaluation_pages"] == len(corpus.labelled) for f in result["folds"])
    assert all(f["training_pages"] + f["evaluation_pages"] == len(corpus.records) for f in result["folds"])
    assert sum(len(p["top_3"]) for p in result["predictions"]) == 3 * len(corpus.labelled)
    total = result["metrics"]["overall"]["counts"]
    assert sum(t["overall"]["counts"]["tp"] for t in result["per_tag"].values()) == total["tp"]
    assert sum(t["positives"] for t in result["per_tag"].values()) == total["positives"]
    assert json.loads(json.dumps(result, allow_nan=False)) == result
    # A new scorer is one registry entry. Its input contains no held out labels.
    with patch.dict(tag_eval.STRATEGIES, {"alternate": tag_eval.app_scorer}):
        again = tag_eval.cross_validate(corpus, saved, "alternate")
    assert again["predictions"] == result["predictions"]
    assert again["metrics"] == result["metrics"]


def test_runner_outputs_are_private_and_cli_requires_explicit_paths(corpus, tmp_path):
    with patch("sys.argv", ["tagger", "tag-eval", "--out", str(tmp_path), "--split", str(tmp_path / "split")]):
        with pytest.raises(SystemExit) as err:
            cli.main()
    assert err.value.code == 2
    with patch.dict(tag_eval.PAGE_SOURCES, {"synthetic": lambda *args: corpus}):
        result = tag_eval.run(
            tag_eval.Paths(tmp_path / "data"),
            tmp_path / "archive",
            tmp_path / "split.json",
            tmp_path,
            source="synthetic",
        )
    assert read_json(tmp_path / "baseline.json") == result
    text = (tmp_path / "baseline.json").read_text()
    assert "synthetic-page-" not in text
    assert len((tmp_path / "baseline.md").read_text().splitlines()) <= 40
    with pytest.raises(tag_eval.Invalid, match="already exist"):
        tag_eval.run(tag_eval.Paths(tmp_path / "data"), tmp_path / "archive", tmp_path / "split.json", tmp_path)
