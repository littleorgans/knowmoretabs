"""Fixed, title judged engine retrieval at ten, on private archive and vector copies.

Only page digests and authored judgements leave the private inputs. Preparation may
refresh scratch A/B once; comparisons reuse frozen queries and refuse any refresh.
"""

import importlib.metadata
import json
import re
import sys
from pathlib import Path

import numpy as np

from . import dataset, embed, eval_runner, zeroshot
from .app import engine, kmt
from .archive import Archive, sha256_hex
from .eval_runner import Invalid
from .guided.retrieval import precision_at
from .paths import Paths, read_json, write_json

K = 10
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
ADDRESS = re.compile(r"://|\bwww\.|\b[\w-]+(?:\.[\w-]+)+\b|\bkmt-(?:git|blob):|[/\\]", re.I)


def fingerprint(value) -> str:
    return sha256_hex(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False))


def corpus(records: list[dict]) -> list[dict]:
    return sorted(({"id": sha256_hex(r["key"])} for r in records if not r["forgotten"]), key=lambda p: p["id"])


def validate(fixture: dict) -> set[str]:
    """Strict public schema: a title or an extra source field is never accepted."""
    if set(fixture) != {"schema", "k", "judge", "rubric", "corpus_digest", "pages", "queries"}:
        raise Invalid("invalid fixture fields")
    if fixture["schema"] != 1 or fixture["k"] != K:
        raise Invalid("unsupported fixture schema or k")
    if not isinstance(fixture["judge"], str) or not isinstance(fixture["rubric"], str):
        raise Invalid("invalid judging provenance")
    pages = fixture["pages"]
    if not isinstance(pages, list) or not pages:
        raise Invalid("empty corpus")
    if any(set(p) != {"id"} or not isinstance(p["id"], str) or not DIGEST.fullmatch(p["id"]) for p in pages):
        raise Invalid("invalid page digest")
    ids = {p["id"] for p in pages}
    if len(ids) != len(pages) or pages != sorted(pages, key=lambda p: p["id"]):
        raise Invalid("duplicate or unordered page digests")
    if fixture["corpus_digest"] != fingerprint(pages):
        raise Invalid("corpus fingerprint drift")
    queries = fixture["queries"]
    if not isinstance(queries, list) or len(queries) != 12:
        raise Invalid("expected twelve queries")
    names = set()
    types = []
    for q in queries:
        if set(q) != {"id", "type", "text", "intent", "rubric", "relevant"}:
            raise Invalid("invalid query fields")
        if not isinstance(q["id"], str) or not re.fullmatch(r"[a-z][a-z0-9-]*", q["id"]) or q["id"] in names:
            raise Invalid("duplicate or invalid query id")
        names.add(q["id"])
        types.append(q["type"])
        for field in ("text", "intent", "rubric"):
            if not isinstance(q[field], str) or not q[field].strip() or ADDRESS.search(q[field]):
                raise Invalid("query prose is empty or contains an address")
        relevant = q["relevant"]
        if not isinstance(relevant, list) or not relevant or any(p not in ids for p in relevant):
            raise Invalid("missing or unknown relevant pages")
        if len(set(relevant)) != len(relevant):
            raise Invalid("duplicate relevant pages")
    if types.count("keyword") != 6 or types.count("semantic") != 6:
        raise Invalid("expected six keyword and six semantic queries")
    return ids


def score(fixture: dict, rankings: dict[str, list[dict]]) -> dict:
    """Score fixed judgements; fewer than ten returned pages are padded as misses."""
    eligible = validate(fixture)
    if set(rankings) != {q["id"] for q in fixture["queries"]}:
        raise Invalid("missing or extra query rankings")
    rows = []
    for q in fixture["queries"]:
        ranked = rankings[q["id"]]
        ids = [h["id"] for h in ranked]
        if len(set(ids)) != len(ids) or any(p not in eligible for p in ids):
            raise Invalid("duplicate or ineligible ranked page")
        hits = np.zeros(K, bool)
        hits[: min(K, len(ids))] = [p in q["relevant"] for p in ids[:K]]
        rows.append(
            {
                "id": q["id"],
                "type": q["type"],
                "relevant": len(q["relevant"]),
                "hits": int(hits.sum()),
                "p_at_10": precision_at(hits)["p_at_10"],
                "r_at_10": float(hits.sum() / len(q["relevant"])),
                "ranked": ranked[:K],
            }
        )
    macro = {}
    for group in ("overall", "keyword", "semantic"):
        selected = [r for r in rows if group == "overall" or r["type"] == group]
        macro[group] = {metric: float(np.mean([r[metric] for r in selected])) for metric in ("p_at_10", "r_at_10")}
    return {"pages": len(eligible), "queries": rows, "macro": macro}


def _files(folder: Path) -> dict:
    """Private file digests, excluding writable locks; never use a VectorStore on live data."""
    import hashlib

    digests = {}
    for p in sorted(folder.rglob("*")):
        if p.is_file() and p.name != "lock":
            with p.open("rb") as stream:
                digests[str(p.relative_to(folder))] = hashlib.file_digest(stream, "sha256").hexdigest()
    return digests


def _inputs(paths: Paths, root: Path, records: list[dict]) -> dict:
    bodies = embed.page_bodies(records)
    if any(r.get("read") and not r["text_ok"] and not body for r, body in zip(records, bodies, strict=True)):
        raise Invalid("unreadable external document")
    return {
        "archive": {name: _files(root / name) for name in ("snapshots", "pages")},
        "state": fingerprint(read_json(root / "library.json")) if (root / "library.json").exists() else None,
        "records": fingerprint(
            [
                {**{k: r[k] for k in ("key", "title", "a_text", "forgotten", "tags")}, "body": body}
                for r, body in zip(records, bodies, strict=True)
            ]
        ),
        "ab": {name: _files(paths.emb / engine.MODEL / name) for name in engine.TEXT_INPUTS},
        "legacy": {
            name: fingerprint((paths.emb / engine.MODEL / (name + ".npy")).read_bytes().hex())
            for name in engine.TEXT_INPUTS
            if (paths.emb / engine.MODEL / (name + ".npy")).exists()
        },
    }


def _runtime(st) -> dict:
    from huggingface_hub import snapshot_download

    cached = Path(snapshot_download(embed.MODELS[engine.MODEL].repo, local_files_only=True))
    return {
        "model": embed.model_identity(embed.MODELS[engine.MODEL]),
        "weights_digest": fingerprint(_files(cached)),
        "query_prompt": st.prompts[zeroshot.QUERY_PROMPT],
        "versions": {
            name: importlib.metadata.version(name)
            for name in ("numpy", "torch", "transformers", "sentence-transformers", "scikit-learn")
        }
        | {"python": sys.version.split()[0]},
    }


def _frozen_check(expected, actual) -> None:
    if expected != actual:
        raise Invalid("frozen input fingerprint drift")


def _run(paths: Paths, root: Path, fixture: dict, out: Path, binary: str, prepare: bool) -> dict:
    validate(fixture)
    frozen_dir = paths.data / "search-eval"
    frozen_path = frozen_dir / "frozen.json"
    vectors_path = frozen_dir / "queries.npy"
    if prepare == frozen_path.exists():
        raise Invalid("prepare exactly once before comparison")
    _, records = dataset.app_records(paths, Archive.load(root), binary=binary)
    if corpus(records) != fixture["pages"]:
        raise Invalid("fixture pages differ from the visible corpus")
    for q in fixture["queries"]:
        for r in records:
            title = r["title"].strip().casefold()
            if title and any(
                re.search(r"(?<!\w)" + re.escape(title) + r"(?!\w)", q[field].casefold())
                for field in ("text", "intent")
            ):
                raise Invalid("query prose quotes a complete title")
    before = _inputs(paths, root, records)
    fixture_digest = fingerprint(fixture)
    frozen = None if prepare else read_json(frozen_path)
    if frozen is not None:
        _frozen_check(frozen["fixture_digest"], fixture_digest)
        _frozen_check(frozen["inputs"], before)
    engine.require_vectors(paths)
    st, _ = embed.load_text(embed.MODELS[engine.MODEL], local_files_only=True)
    runtime = _runtime(st)
    if frozen is not None:
        _frozen_check(frozen["runtime"], runtime)

    def encode(texts):
        return zeroshot.encode_queries(st, texts)

    def document_vectors(rows, name):
        if not prepare:
            raise Invalid("frozen A or B vectors require refresh")
        return embed.embed_input(st, embed.MODELS[engine.MODEL], rows, name)[0]

    lib, stats = engine.load(paths, root, encode, document_vectors, binary=binary)
    if stats.get("chunks", {}).get("code") == "chunks_unavailable":
        raise Invalid("frozen C could not be reused; chunk refresh is prohibited")
    if corpus(lib.records) != fixture["pages"]:
        raise Invalid("loaded corpus drift")
    after = _inputs(paths, root, lib.records)
    _frozen_check({k: v for k, v in before.items() if k != "ab"}, {k: v for k, v in after.items() if k != "ab"})
    if prepare:
        queries = encode([q["text"] for q in fixture["queries"]])
    else:
        _frozen_check(before, after)
        queries = np.load(vectors_path, allow_pickle=False)
    if queries.shape != (12, lib.X.shape[1]) or not np.isfinite(queries).all():
        raise Invalid("invalid frozen query vectors")
    identity = {
        "fixture_digest": fixture_digest,
        "inputs": after,
        "runtime": runtime,
        "input_name": lib.input_name,
        "vectors_digest": fingerprint(lib.X.tolist()),
        "queries_digest": fingerprint(queries.tolist()),
        "visibility_digest": fingerprint(lib.live.tolist()),
    }
    if prepare:
        frozen_dir.mkdir(parents=True, exist_ok=True)
        with vectors_path.open("xb") as stream:
            np.save(stream, queries, allow_pickle=False)
        write_json(frozen_path, identity)
    else:
        _frozen_check(frozen, identity)
    rankings = {}
    for q, vector in zip(fixture["queries"], queries, strict=True):
        ranked = engine.search(lib, vector, q["text"], K, images=False, among=lib.live, offset=0)
        # Copy numeric signals through an explicit schema, independent of source names.
        rankings[q["id"]] = [
            {
                "id": sha256_hex(lib.records[h["row"]]["key"]),
                "fused": float(h["fused"]),
                "signals": [
                    {"source": name, "score": float(s["score"]), "rank": int(s["rank"])}
                    for name, s in h["sources"].items()
                ],
            }
            for h in ranked
        ]
    _, final_records = dataset.app_records(paths, Archive.load(root), binary=binary)
    _frozen_check(after, _inputs(paths, root, final_records))
    result = {
        "schema": 1,
        "fixture_digest": fixture_digest,
        "corpus_digest": fixture["corpus_digest"],
        "inputs_digest": fingerprint(after),
        "runtime": runtime,
        "input_name": lib.input_name,
        "vectors_digest": identity["vectors_digest"],
        "queries_digest": identity["queries_digest"],
        "visibility_digest": identity["visibility_digest"],
        "preparation": stats["vectors"] if prepare else None,
        "k": K,
        **score(fixture, rankings),
    }
    return result


def run(paths: Paths, root: Path, fixture: Path, out: Path, *, binary: str = kmt.BINARY, prepare: bool = False) -> dict:
    """All diagnostics stay private; only fixed reasons can escape on failure."""
    return eval_runner.run(
        paths,
        root,
        out,
        lambda: _run(paths, root, read_json(fixture), out, binary, prepare),
        stem="results",
        log=out / "diagnostics.log",
    )
