"""Acceptance checks for the digest only, fixed engine search baseline."""

import copy
import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import numpy as np
import pytest

from tagger import search_eval
from tagger.archive import sha256_hex
from tagger.paths import Paths, read_json, write_json


@pytest.fixture
def evaluation(tmp_path):
    records = [
        {
            "key": f"synthetic-page-{i}",
            "title": f"Private synthetic title {i}",
            "a_text": "Private synthetic metadata",
            "forgotten": False,
            "tags": [],
            "text_ok": False,
            "read": None,
        }
        for i in range(12)
    ]
    pages = search_eval.corpus(records)
    relevant = sorted(sha256_hex(r["key"]) for r in records[:5])
    fixture = {
        "schema": 1,
        "k": 10,
        "judge": "synthetic acceptance",
        "rubric": "A title must clearly address the intent",
        "corpus_digest": search_eval.fingerprint(pages),
        "pages": pages,
        "queries": [
            {
                "id": f"q{i}",
                "type": "keyword" if i < 6 else "semantic",
                "text": f"find useful notes {i}",
                "intent": "Find pages explaining the chosen subject",
                "rubric": "The title explicitly addresses the subject",
                "relevant": relevant,
            }
            for i in range(12)
        ],
    }
    fixture_path = tmp_path / "judgements.json"
    write_json(fixture_path, fixture)
    lib = SimpleNamespace(records=records, live=np.ones(12, bool), X=np.ones((12, 2)), input_name="B")
    ranked = [
        {"row": i, "fused": 0.1, "sources": {"text": {"score": 0.2, "rank": j + 1}}, "title": "do not copy"}
        for j, i in enumerate([0, 1, 2, 5, 6, 7, 8])
    ]
    with (
        patch.object(search_eval.dataset, "app_records", return_value=(None, records)),
        patch.object(search_eval.Archive, "load"),
        patch.object(search_eval.engine, "require_vectors"),
        patch.object(search_eval.embed, "load_text", return_value=(object(), {})),
        patch.object(search_eval, "_runtime", return_value={"model": "synthetic", "query_prompt": "query"}),
        patch.object(search_eval.zeroshot, "encode_queries", return_value=np.ones((12, 2))),
        patch.object(search_eval.engine, "load", return_value=(lib, {"vectors": {"embedded": 0}})),
        patch.object(search_eval.engine, "search", return_value=ranked) as search,
    ):

        def run(name, prepare=False):
            return search_eval.run(
                Paths(tmp_path / "data"), tmp_path / "archive", fixture_path, tmp_path / name, prepare=prepare
            )

        yield SimpleNamespace(run=run, fixture=fixture, records=records, search=search, root=tmp_path)


def test_known_ranking_and_repeat(evaluation):
    first = evaluation.run("first", prepare=True)
    again = evaluation.run("again")
    assert first["queries"] == again["queries"]
    assert first["macro"] == again["macro"]
    for group in first["macro"].values():
        assert group["p_at_10"] == pytest.approx(0.3)
        assert group["r_at_10"] == pytest.approx(0.6)
    assert first["queries"][0]["relevant"] == 5
    assert first["queries"][0]["hits"] == 3
    assert evaluation.search.call_args.kwargs["images"] is False
    assert evaluation.search.call_args.kwargs["offset"] == 0
    np.testing.assert_array_equal(evaluation.search.call_args.kwargs["among"], np.ones(12, bool))


def test_drift_refuses_before_scoring(evaluation):
    evaluation.run("first", prepare=True)
    evaluation.records[0]["a_text"] += " changed"
    with pytest.raises(search_eval.Invalid, match="drift"):
        evaluation.run("drift")
    assert not (evaluation.root / "drift" / "results.json").exists()
    assert evaluation.search.call_count == 12


def test_only_allowed_output_fields(evaluation):
    result = evaluation.run("first", prepare=True)
    assert read_json(evaluation.root / "first" / "results.json") == result
    assert set(result) == {
        "schema",
        "source_revision",
        "fixture_digest",
        "corpus_digest",
        "inputs_digest",
        "runtime",
        "input_name",
        "vectors_digest",
        "queries_digest",
        "visibility_digest",
        "preparation",
        "k",
        "pages",
        "queries",
        "macro",
    }
    for query in result["queries"]:
        assert set(query) == {"id", "type", "relevant", "hits", "p_at_10", "r_at_10", "ranked"}
        for hit in query["ranked"]:
            assert set(hit) == {"id", "fused", "signals"}
            assert search_eval.DIGEST.fullmatch(hit["id"])
            assert all(set(signal) == {"source", "score", "rank"} for signal in hit["signals"])
    text = json.dumps(result)
    for private in ("Private synthetic", "synthetic-page", "do not copy"):
        assert private not in text


def test_fixture_and_rankings_refuse_invalid_inputs(evaluation):
    fixture = evaluation.fixture
    bad = copy.deepcopy(fixture)
    bad["pages"][0]["title"] = "Private synthetic title"
    with pytest.raises(search_eval.Invalid, match="digest"):
        search_eval.validate(bad)
    bad = copy.deepcopy(fixture)
    bad["queries"][0]["text"] = "lookup example.invalid"
    with pytest.raises(search_eval.Invalid, match="address"):
        search_eval.validate(bad)
    for ids in ([fixture["pages"][0]["id"]] * 2, ["0" * 64]):
        rankings = {q["id"]: [{"id": p} for p in ids] for q in fixture["queries"]}
        with pytest.raises(search_eval.Invalid, match="duplicate or ineligible"):
            search_eval.score(fixture, rankings)


def test_committed_baseline_scores_committed_judgements():
    """A judgement change without a fresh baseline run leaves slice 3 comparing against stale numbers."""
    evals = Path(__file__).parents[1] / "evals" / "search-v1"
    fixture = read_json(evals / "judgements.json")
    baseline = read_json(evals / "baseline.json")
    assert baseline["fixture_digest"] == search_eval.fingerprint(fixture)
    rescored = search_eval.score(fixture, {q["id"]: q["ranked"] for q in baseline["queries"]})
    assert {key: baseline[key] for key in rescored} == rescored
