"""Synthetic proof of the page split, leakage boundary and global top three metrics."""

import copy
import hashlib
import json
from dataclasses import replace
from types import SimpleNamespace
from unittest.mock import patch

import numpy as np
import pytest

from tagger import cli, tag_eval, tag_grid
from tagger.app import engine
from tagger.guided.search_tag import MIN_HEAD_POSITIVES
from tagger.heads import unit
from tagger.paths import read_json, write_json
from tagger.search_eval import fingerprint
from tagger.vector_store import VectorStore


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


def test_frozen_app_vectors_validate_hashes_without_writing(corpus, tmp_path):
    paths = tag_eval.Paths(tmp_path / "data")
    records = [{**r, "_embedding_doc": f"synthetic document {i}"} for i, r in enumerate(corpus.records)]
    spec = replace(tag_eval.embed.MODELS["eg2"], dim=corpus.X.shape[1])
    store = VectorStore(paths.emb / "eg2", "B")
    store.reconcile(
        tag_eval.embed.input_rows(records, spec, "B"),
        tag_eval.embed.model_identity(spec),
        lambda rows: corpus.X[rows],
    )
    state = tag_eval.LibraryState(active={t.lower(): t for t in corpus.tags})
    before = {p: (p.read_bytes(), p.stat().st_mtime_ns) for p in paths.data.rglob("*") if p.is_file()}
    with (
        patch.dict(tag_eval.embed.MODELS, {"eg2": spec}),
        patch.object(tag_eval.Archive, "load", return_value=SimpleNamespace(state=state)),
        patch.object(tag_eval.dataset, "app_records", return_value=(None, records)),
        patch.object(tag_eval.embed, "load_text", return_value=(object(), {})),
        patch.object(tag_eval.embed, "release"),
        patch.object(tag_eval.zeroshot, "encode_queries", return_value=corpus.q),
        patch.object(VectorStore, "_lock", side_effect=AssertionError("input writes prohibited")),
    ):
        loaded = tag_eval.app_vectors(paths, tmp_path / "archive", "unused")
        np.testing.assert_array_equal(loaded.X, corpus.X.astype(np.float32))
        records[0]["_embedding_doc"] = "changed synthetic input"
        with pytest.raises(tag_eval.Invalid, match="refresh"):
            tag_eval.app_vectors(paths, tmp_path / "archive", "unused")
    assert {p: (p.read_bytes(), p.stat().st_mtime_ns) for p in paths.data.rglob("*") if p.is_file()} == before


def test_cmean_uses_page_keys_normalized_means_and_app_fallback(corpus, tmp_path):
    paths = tag_eval.Paths(tmp_path / "data")
    rows = [
        {"id": "chunk-b", "page_id": corpus.records[1]["key"]},
        {"id": "chunk-a", "page_id": corpus.records[0]["key"]},
        {"id": "chunk-c", "page_id": corpus.records[0]["key"]},
        {"id": "orphan", "page_id": "synthetic-absent-page"},
    ]
    vectors = np.eye(6)[[0, 1, 2, 3]]
    store = VectorStore(paths.emb / "eg2", "C")
    store.reconcile(rows, {"dim": 6}, lambda selected: vectors[selected])
    corpus.metadata["vectors_digest"] = hashlib.sha256(corpus.X.tobytes()).hexdigest()
    before = {p: p.read_bytes() for p in paths.data.rglob("*") if p.is_file()}
    with patch.object(VectorStore, "_lock", side_effect=AssertionError("input writes prohibited")):
        result = tag_eval.cmean_corpus(corpus, paths)
    np.testing.assert_allclose(result.X[0], unit(vectors[[1, 2]].mean(axis=0, keepdims=True))[0])
    np.testing.assert_allclose(result.X[1], vectors[0])
    np.testing.assert_array_equal(result.X[2:], corpus.X[2:])
    assert result.metadata["pages_without_chunks"] == len(corpus.records) - 2
    assert result.metadata["evaluated_pages_without_chunks"] == len(corpus.labelled) - 2
    assert result.metadata["chunks_ignored"] == 1
    assert result.metadata["chunks_used"] == 3
    assert result.records is corpus.records and result.Y is corpus.Y
    assert {p: p.read_bytes() for p in paths.data.rglob("*") if p.is_file()} == before


def test_prototypes_keep_app_heads_and_unseen_queries(corpus):
    train, held = np.arange(10, 80), np.arange(10)
    X, Y = corpus.X[train], corpus.Y[train].copy()
    Y[:, 3] = 0
    expected = corpus.q.copy()
    for j in np.flatnonzero(Y.sum(axis=0)):
        expected[j] = unit(X[Y[:, j].astype(bool)].mean(axis=0, keepdims=True))[0]
    np.testing.assert_allclose(tag_eval.prototype_queries(X, Y, corpus.q), expected)
    app = tag_eval.app_scorer(X, Y, corpus.q, corpus.tags, corpus.X[held])
    proto = tag_eval.STRATEGIES["proto_fallback"](X, Y, corpus.q, corpus.tags, corpus.X[held])
    np.testing.assert_array_equal(proto.heads, app.heads)
    assert proto.params == app.params
    ordered_heads = [app.heads[corpus.tags.index(t)] for t in app.tags]
    np.testing.assert_array_equal(proto.S.values[:, ordered_heads], app.S.values[:, ordered_heads])
    for j, tag in enumerate(proto.tags):
        canonical = corpus.tags.index(tag)
        if not proto.heads[canonical]:
            reference = X @ expected[canonical]
            np.testing.assert_allclose(proto.S.values[:, j], corpus.X[held] @ expected[canonical])
            assert proto.S.training_mean[j] == pytest.approx(reference.mean())
            assert proto.spread[j] == pytest.approx(engine._spread(reference[:, None])[0])
    with patch.object(tag_eval, "fit_rules", side_effect=AssertionError("proto_all must not fit LR")):
        all_proto = tag_eval.STRATEGIES["proto_all"](X, Y, corpus.q, corpus.tags, corpus.X[held])
    order = [corpus.tags.index(t) for t in all_proto.tags]
    np.testing.assert_allclose(all_proto.S.values, corpus.X[held] @ expected[order].T)
    changed = tag_eval.STRATEGIES["proto_all"](X, Y, corpus.q, corpus.tags, -100 * corpus.X[held])
    np.testing.assert_array_equal(changed.S.training_mean, all_proto.S.training_mean)
    np.testing.assert_array_equal(changed.spread, all_proto.spread)
    # A fold with no eligible heads remains a valid prototype or app scorer.
    rare_Y = np.zeros_like(Y)
    rare_Y[:3, 0] = 1
    assert not tag_eval.app_scorer(X, rare_Y, corpus.q, corpus.tags, corpus.X[held]).heads.any()


def test_prior_and_oracle_cardinality_ceiling(corpus, tmp_path):
    saved = tag_eval.page_split(corpus, tmp_path / "split.json")
    prior = tag_eval.cross_validate(corpus, saved, "prior")
    ceiling = tag_eval.cross_validate(corpus, saved, "ceiling")
    for f in prior["folds"]:
        tags = sorted(corpus.tags, key=lambda t: (-f["training_positives"][corpus.tags.index(t)], t.lower()))[:3]
        predictions = [p for p in prior["predictions"] if p["fold"] == f["fold"]]
        assert all([h["tag"] for h in p["top_3"]] == tags for p in predictions)
    _, rates = tag_grid.page_rates(ceiling)
    expected = [min(3, len(p["truth"])) / 3 for p in sorted(ceiling["predictions"], key=lambda p: p["id"])]
    np.testing.assert_array_equal(rates[:, 0], expected)
    assert ceiling["metrics"]["overall"]["macro"]["precision_at_3"] == pytest.approx(np.mean(expected))
    assert ceiling["metrics"]["overall"]["macro"]["hit_at_3"] == 1


def test_paired_bootstrap_resamples_pages_deterministically():
    def result(tags):
        return {
            "predictions": [
                {"id": str(i), "fold": i, "truth": ["A"], "top_3": [{"tag": t} for t in top]}
                for i, top in enumerate(tags)
            ]
        }

    baseline = result([["A", "B", "C"], ["B", "C", "D"]])
    candidate = result([["B", "C", "D"], ["A", "B", "C"]])
    paired = tag_grid.paired_bootstrap(candidate, baseline)
    assert paired == tag_grid.paired_bootstrap(candidate, baseline)
    assert paired["precision_at_3"]["difference"] == 0
    assert paired["precision_at_3"]["interval"] == pytest.approx([-1 / 3, 1 / 3])
    assert paired["recall_at_3"]["interval"] == [-1, 1]
    candidate["predictions"].reverse()
    assert paired == tag_grid.paired_bootstrap(candidate, baseline)
    same = tag_grid.paired_bootstrap(baseline, baseline)
    assert same["precision_at_3"] == {"difference": 0, "interval": [0, 0]}
    candidate["predictions"][0]["fold"] = 9
    with pytest.raises(tag_eval.Invalid, match="differ"):
        tag_grid.paired_bootstrap(candidate, baseline)


def test_full_grid_reuses_split_and_reproduces_baseline(corpus, tmp_path):
    corpus.metadata.update(
        vectors_digest=hashlib.sha256(corpus.X.tobytes()).hexdigest(),
        queries_digest=hashlib.sha256(corpus.q.tobytes()).hexdigest(),
        descriptions_digest="synthetic-description-digest",
    )
    split_path = tmp_path / "split.json"
    saved = tag_eval.page_split(corpus, split_path)
    before = split_path.read_bytes(), split_path.stat().st_mtime_ns
    baseline = {
        **tag_eval.cross_validate(corpus, saved, "app"),
        "split_digest": fingerprint(saved),
        "page_source": "app",
        "strategy": "app",
        "k": tag_eval.K,
        "seed": tag_eval.SEED,
        "inputs": corpus.metadata,
    }
    baseline_path = tmp_path / "baseline.json"
    write_json(baseline_path, baseline)
    cmean = replace(
        corpus,
        metadata={
            **corpus.metadata,
            "pages_with_chunks": 70,
            "pages_without_chunks": 10,
            "evaluated_pages_without_chunks": 2,
            "chunks_used": 150,
        },
    )
    with (
        patch.object(tag_eval, "app_vectors", return_value=corpus),
        patch.object(tag_eval, "cmean_corpus", return_value=cmean),
    ):
        result = tag_grid.run(
            tag_eval.Paths(tmp_path / "data"), tmp_path / "archive", split_path, tmp_path, baseline_path
        )
    assert len(result["rows"]) == 8
    assert result["rows"]["app/app"]["paired_vs_baseline"]["precision_at_3"]["interval"] == [0, 0]
    assert result["best_row"] != "ceiling"
    assert (split_path.read_bytes(), split_path.stat().st_mtime_ns) == before
    assert read_json(tmp_path / "grid.json") == result
    assert "synthetic-page-" not in (tmp_path / "grid.json").read_text()
    assert len((tmp_path / "grid.md").read_text().splitlines()) <= 40
    with pytest.raises(tag_eval.Invalid, match="already exist"):
        tag_grid.run(tag_eval.Paths(tmp_path / "data"), tmp_path / "archive", split_path, tmp_path, baseline_path)
    baseline["split_digest"] = "different"
    with pytest.raises(tag_eval.Invalid, match="provenance"):
        tag_grid.evaluate({"app": corpus, "cmean": corpus}, saved, baseline)
