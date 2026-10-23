"""Page held out evaluation of the app's tag suggestions on an explicit private copy.

Extend PAGE_SOURCES with a function returning Corpus, or STRATEGIES with a function
returning SuggestionView. Both keep the saved page split and metric definitions.
Only tagged training rows fit rules; every training row sets the ranking reference.
"""

import hashlib
import importlib.metadata
import json
import shlex
import subprocess
import traceback
from contextlib import redirect_stderr, redirect_stdout
from dataclasses import dataclass, replace
from functools import partial
from pathlib import Path

import numpy as np

from . import dataset, embed, zeroshot
from .app import engine, kmt
from .archive import Archive, LibraryState, sha256_hex
from .evaluate import OUTER_FOLDS, Labels, outer_folds
from .features import Dense
from .guided.search_tag import MIN_HEAD_POSITIVES, fit_rules
from .heads import SEED, fit_knn, fit_prior, unit
from .metrics import best_threshold
from .paths import Paths, read_json, write_json
from .search_eval import Invalid, _safe_paths, fingerprint
from .vector_store import COUNT_KEYS, VectorStore

K = 3


@dataclass
class Corpus(Labels):
    X: np.ndarray
    q: np.ndarray
    metadata: dict
    chunk_coverage: np.ndarray | None = None


@dataclass
class HeldOutScores:
    """The two S operations suggest uses, with its mean frozen on training pages."""

    values: np.ndarray
    training_mean: np.ndarray

    def __getitem__(self, rows):
        return self.values[rows]

    def mean(self, *, axis):
        if axis != 0:
            raise ValueError("only the training column mean is available")
        return self.training_mean


@dataclass
class SuggestionView:
    S: HeldOutScores
    spread: np.ndarray
    tags: list[str]
    state: LibraryState
    heads: np.ndarray  # app head eligibility in canonical columns, also for reference scorers
    thresholds: np.ndarray  # canonical columns, trained even though top k ignores them
    params: dict


def app_vectors(paths: Paths, root: Path, binary: str) -> Corpus:
    """The app's archive records, tag queries and global B then A store selection."""
    archive = Archive.load(root)
    _, records = dataset.app_records(paths, archive, binary=binary)
    tags = sorted(archive.state.active.values(), key=lambda t: (-sum(t in r["tags"] for r in records), t.lower()))
    Y = engine._labels(records, tags)
    engine.require_vectors(paths)
    name = next(n for n in engine.TEXT_INPUTS if VectorStore(paths.emb / engine.MODEL, n).exists)
    # These are frozen private copies. _read validates without creating a lock or
    # letting reconciliation publish a generation in the read only input tree.
    cached = VectorStore(paths.emb / engine.MODEL, name)._read()
    spec = embed.MODELS[engine.MODEL]
    if cached is None or cached[0]["model"] != embed.model_identity(spec):
        raise Invalid("matching frozen app vectors required")
    manifest, arrays = cached
    positions = {r["id"]: i for i, r in enumerate(manifest["rows"])}
    requested = embed.input_rows(records, spec, name)
    if any(r["id"] not in positions or manifest["rows"][positions[r["id"]]] != r for r in requested):
        raise Invalid("frozen app vectors need refresh")
    X = arrays["vectors"][[positions[r["id"]] for r in requested]]
    counts = {**dict.fromkeys(COUNT_KEYS, 0), "reused": len(X)}
    st, runtime = embed.load_text(embed.MODELS[engine.MODEL], local_files_only=True)
    try:
        description_path = paths.data / "zeroshot" / "descriptions.json"
        descriptions = read_json(description_path) if description_path.exists() else {}
        texts = zeroshot.query_variants(tags, descriptions)["description"]
        q = engine._queries(lambda ts: zeroshot.encode_queries(st, ts), texts, X.shape[1])
    finally:
        embed.release(st)
    return Corpus(
        records,
        tags,
        Y,
        np.flatnonzero(Y.any(axis=1)),
        X,
        q,
        {
            "model": engine.MODEL,
            "input": name,
            "dimension": X.shape[1],
            "vectors": counts,
            "vectors_digest": hashlib.sha256(X.tobytes()).hexdigest(),
            "queries_digest": hashlib.sha256(q.tobytes()).hexdigest(),
            "descriptions_digest": fingerprint(texts),
            "runtime": runtime,
            "versions": {
                n: importlib.metadata.version(n)
                for n in ("numpy", "scikit-learn", "sentence-transformers", "transformers", "torch")
            },
        },
    )


def cmean_corpus(corpus: Corpus, paths: Paths) -> Corpus:
    """Store C page_id is the archive key, as in Library.__post_init__."""
    cached = VectorStore(paths.emb / engine.MODEL, "C")._read()
    if cached is None or cached[0]["model"]["dim"] != corpus.X.shape[1]:
        raise Invalid("matching frozen C vectors required")
    manifest, arrays = cached
    positions = {r["key"]: i for i, r in enumerate(corpus.records)}
    sums = np.zeros_like(corpus.X)
    counts = np.zeros(len(sums), int)
    for row, vector in zip(manifest["rows"], arrays["vectors"], strict=True):
        if (i := positions.get(row["page_id"])) is not None:
            sums[i] += vector
            counts[i] += 1
    covered = counts > 0
    X = corpus.X.copy()
    X[covered] = unit(sums[covered] / counts[covered, None])
    return replace(
        corpus,
        X=X,
        chunk_coverage=covered,
        metadata={
            **corpus.metadata,
            "input": "cmean",
            "fallback_input": corpus.metadata["input"],
            "app_vectors_digest": corpus.metadata["vectors_digest"],
            "vectors_digest": hashlib.sha256(X.tobytes()).hexdigest(),
            "chunks_digest": fingerprint(manifest),
            "chunk_vectors_digest": hashlib.sha256(arrays["vectors"].tobytes()).hexdigest(),
            "chunks_used": int(counts.sum()),
            "chunks_ignored": len(manifest["rows"]) - int(counts.sum()),
            "pages_with_chunks": int(covered.sum()),
            "pages_without_chunks": int((~covered).sum()),
            "evaluated_pages_without_chunks": int((~covered[corpus.labelled]).sum()),
        },
    )


def cmean_vectors(paths: Paths, root: Path, binary: str) -> Corpus:
    return cmean_corpus(app_vectors(paths, root, binary), paths)


def bc_corpus(app: Corpus, cmean: Corpus) -> Corpus:
    """Equal unit B and C mean fusion, retaining B where C is absent."""
    X = unit(app.X + cmean.X)
    return replace(
        cmean,
        X=X,
        metadata={**cmean.metadata, "input": "bc", "vectors_digest": hashlib.sha256(X.tobytes()).hexdigest()},
    )


def bc_vectors(paths: Paths, root: Path, binary: str) -> Corpus:
    app = app_vectors(paths, root, binary)
    return bc_corpus(app, cmean_corpus(app, paths))


def prototype_queries(Xtrain, Ytrain, q):
    """Pure positive means, with the app query retained for unseen training tags."""
    counts = Ytrain.sum(axis=0)
    seen = counts > 0
    queries = q.copy()
    queries[seen] = unit((Ytrain[:, seen].T @ Xtrain) / counts[seen, None])
    return queries


def app_scorer(Xtrain, Ytrain, q, tags, Xtest, *, strategy="app") -> SuggestionView:
    """One training boundary and app ranking path for all deployable scorers."""
    order = np.array(sorted(range(len(tags)), key=lambda j: (-int(Ytrain[:, j].sum()), tags[j].lower())))
    inverse = np.argsort(order)
    labelled = Ytrain.any(axis=1)
    Y = Ytrain[:, order]
    queries = prototype_queries(Xtrain, Ytrain, q) if strategy in ("proto_fallback", "proto_all") else q
    params = {}
    if strategy in ("prior", "knn"):
        fit = fit_prior if strategy == "prior" else fit_knn
        rows = np.arange(len(Xtrain)) if strategy == "prior" else np.flatnonzero(labelled)
        fitted = fit(Dense("train", Xtrain), Y, rows)
        train, held = fitted.predict_matrix(Xtrain), fitted.predict_matrix(Xtest)
        threshold = fitted.thresholds.copy()
        params = fitted.params
    elif strategy == "proto_all":
        train, held = Xtrain @ queries[order].T, Xtest @ queries[order].T
        threshold = np.array([best_threshold(train[labelled, j], Y[labelled, j]) for j in range(len(tags))])
    else:
        rules = fit_rules(Xtrain[labelled], Y[labelled], queries[order])
        train = rules.scores(Xtrain)["supervised"]
        held = rules.scores(Xtest)["supervised"]
        threshold = rules.thresholds["supervised"].copy()
        if rules.fitted is not None:
            params = {
                tags[order[j]]: {k: v[i] for k, v in rules.fitted.params.items()} for i, j in enumerate(rules.heads)
            }
    unseen = ~np.isfinite(threshold)
    threshold[unseen] = engine._zero_shot_threshold(train[:, unseen])
    return SuggestionView(
        HeldOutScores(held, np.zeros(len(tags)) if strategy in ("prior", "knn") else train.mean(axis=0)),
        np.ones(len(tags)) if strategy in ("prior", "knn") else engine._spread(train),
        [tags[j] for j in order],
        LibraryState(active={t.lower(): t for t in tags}),
        Ytrain.sum(axis=0) >= MIN_HEAD_POSITIVES,
        threshold[inverse],
        params,
    )


APP_RANKINGS = ("app", "heads_only", "heads_first", "margin")
PAGE_SOURCES = {"app": app_vectors, "cmean": cmean_vectors, "bc": bc_vectors}
STRATEGIES = {
    name: partial(app_scorer, strategy=name) for name in (*APP_RANKINGS, "knn", "proto_fallback", "proto_all", "prior")
}


def rank_suggestions(fitted: SuggestionView, row: int, strategy: str) -> list[dict]:
    """App ties throughout; heads first admits heads only when prechecks clear."""
    # Head eligibility and thresholds are canonical; scores follow fitted.tags.
    canonical = {t: j for j, t in enumerate(fitted.state.active.values())}
    columns = np.array([canonical[t] for t in fitted.tags])
    if strategy == "margin":
        fitted = replace(
            fitted,
            S=HeldOutScores(fitted.S.values, fitted.thresholds[columns]),
            spread=np.ones(len(columns)),
        )
    ranked = engine.suggest(fitted, [row], k=len(fitted.tags))
    if strategy == "heads_only":
        return [h for h in ranked if fitted.heads[canonical[h["tag"]]]]
    if strategy == "heads_first":
        passing = {
            t
            for j, t in enumerate(fitted.tags)
            if fitted.heads[columns[j]] and fitted.S[row, j] >= fitted.thresholds[columns[j]]
        }
        return [h for h in ranked if h["tag"] in passing] + [h for h in ranked if not fitted.heads[canonical[h["tag"]]]]
    return ranked


def page_split(corpus: Corpus, path: Path) -> dict:
    """Write page keys and folds once; later runs validate and reuse the exact bytes."""
    keys = [corpus.records[i]["key"] for i in corpus.labelled]
    if len(keys) < OUTER_FOLDS or len(set(keys)) != len(keys):
        raise Invalid("at least five distinct tagged pages required")
    expected = {
        "schema": 1,
        "seed": SEED,
        "folds": OUTER_FOLDS,
        "labels_digest": fingerprint(sorted((r["key"], sorted(r["tags"])) for r in corpus.records if r["tags"])),
    }
    if not path.exists():
        assignments = {}
        for fold, (_, held) in enumerate(outer_folds(corpus, corpus.labelled)):
            assignments.update((corpus.records[i]["key"], fold) for i in held)
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("x") as stream:
            json.dump({**expected, "pages": [{"key": key, "fold": assignments[key]} for key in sorted(keys)]}, stream)
            stream.write("\n")
    saved = read_json(path)
    if {k: saved.get(k) for k in expected} != expected or set(saved) != {*expected, "pages"}:
        raise Invalid("saved split provenance differs")
    pages = saved["pages"]
    if (
        not isinstance(pages, list)
        or any(not isinstance(p, dict) or set(p) != {"key", "fold"} for p in pages)
        or any(not isinstance(p["key"], str) or type(p["fold"]) is not int for p in pages)
        or len(pages) != len(keys)
        or {p["key"] for p in pages} != set(keys)
        or {p["fold"] for p in pages} != set(range(OUTER_FOLDS))
    ):
        raise Invalid("invalid saved page assignments")
    return saved


def ratio(numerator, denominator):
    """The lab's zero division convention: a rate with no support is zero."""
    return np.divide(numerator, denominator, out=np.zeros_like(numerator, dtype=float), where=denominator != 0)


def measure(Y: np.ndarray, pred: np.ndarray, mask: np.ndarray) -> dict:
    """Masks partition the global top three; they never produce a new ranking."""
    y, p = Y.astype(bool) & mask, pred & mask
    tp, positives, predicted = (a.sum(axis=1) for a in (y & p, y, p))
    supported, proposed = positives > 0, predicted > 0
    hit = float((tp[supported] > 0).mean()) if supported.any() else 0.0
    tag_mask = np.broadcast_to(mask, Y.shape).any(axis=0)
    tag_tp, tag_pos, tag_pred = (a.sum(axis=0) for a in (y & p, y, p))
    return {
        "counts": {
            "pages": len(Y),
            "positive_pages": int(supported.sum()),
            "prediction_pages": int(proposed.sum()),
            "hit_pages": int((tp > 0).sum()),
            "positives": int(positives.sum()),
            "predictions": int(predicted.sum()),
            "tp": int(tp.sum()),
            "fp": int(predicted.sum() - tp.sum()),
            "fn": int(positives.sum() - tp.sum()),
        },
        "macro": {
            "precision_at_3": float(ratio(tp, predicted)[proposed].mean()) if proposed.any() else 0.0,
            "recall_at_3": float(ratio(tp, positives)[supported].mean()) if supported.any() else 0.0,
            "hit_at_3": hit,
        },
        "micro": {
            "precision_at_3": float(ratio(tp.sum(), predicted.sum())),
            "recall_at_3": float(ratio(tp.sum(), positives.sum())),
            "hit_at_3": hit,
        },
        "tag_macro": {
            "precision_at_3": float(ratio(tag_tp, tag_pred)[tag_mask].mean()) if tag_mask.any() else 0.0,
            "recall_at_3": float(ratio(tag_tp, tag_pos)[tag_mask].mean()) if tag_mask.any() else 0.0,
        },
    }


def summaries(Y, pred, heads, full_heads) -> dict:
    return {
        "overall": measure(Y, pred, np.ones(Y.shape[1], bool)),
        "by_fold_head": {name: measure(Y, pred, mask) for name, mask in (("head", heads), ("fallback", ~heads))},
        "by_full_data_head": {
            name: measure(Y, pred, mask) for name, mask in (("head", full_heads), ("fallback", ~full_heads))
        },
    }


def fold_indices(corpus: Corpus, saved: dict):
    """Shared saved outer boundary, including untagged training reference pages."""
    assigned = {p["key"]: p["fold"] for p in saved["pages"]}
    folds = np.array([assigned[corpus.records[i]["key"]] for i in corpus.labelled])
    for fold in range(OUTER_FOLDS):
        local = np.flatnonzero(folds == fold)
        held = corpus.labelled[local]
        train = np.flatnonzero(~np.isin(np.arange(len(corpus.records)), held))
        yield fold, local, train, held


def cross_validate(corpus: Corpus, saved: dict, strategy: str, *, fitted_folds=None) -> dict:
    rows = corpus.labelled
    Y = corpus.Y[rows]
    if len(corpus.tags) < K:
        raise Invalid("at least three active tags required for precision at three")
    full_heads = corpus.Y.sum(axis=0) >= MIN_HEAD_POSITIVES
    folds = np.empty(len(rows), int)
    pred, heads = np.zeros_like(Y, bool), np.zeros_like(Y, bool)
    details, rankings = [], [None] * len(rows)
    tag_columns = {t: j for j, t in enumerate(corpus.tags)}
    for fold, local, train, held in fold_indices(corpus, saved):
        folds[local] = fold
        if fitted_folds is None:
            scorer = STRATEGIES["prior" if strategy == "ceiling" else strategy]
            fitted = scorer(corpus.X[train], corpus.Y[train], corpus.q, corpus.tags, corpus.X[held])
        else:
            fitted = fitted_folds[fold]
        heads[local] = fitted.heads
        for i, page in enumerate(local):
            suggested = rank_suggestions(fitted, i, strategy)
            if strategy == "ceiling":
                # Oracle reference only: ties within truth and padding follow the prior.
                suggested.sort(key=lambda h: not Y[page, tag_columns[h["tag"]]])
            rankings[page] = suggested[:K]
            pred[page, [tag_columns[h["tag"]] for h in suggested[:K]]] = True
        details.append(
            {
                "fold": fold,
                "training_pages": len(train),
                "fitting_pages": int(corpus.Y[train].any(axis=1).sum()),
                "evaluation_pages": len(held),
                "heads": int(fitted.heads.sum()),
                "training_positives": corpus.Y[train].sum(axis=0).tolist(),
                "has_head": fitted.heads.tolist(),
                "thresholds": fitted.thresholds.tolist(),
                "standardization": {
                    "tags": fitted.tags,
                    "mean": fitted.S.training_mean.tolist(),
                    "spread": fitted.spread.tolist(),
                },
                "params": fitted.params,
                "metrics": summaries(Y[local], pred[local], heads[local], full_heads),
            }
        )
        print(json.dumps({"fold": fold, "evaluation_pages": len(held), "heads": int(fitted.heads.sum())}), flush=True)
    per_tag = {}
    for j, tag in enumerate(corpus.tags):
        metrics = summaries(Y[:, j : j + 1], pred[:, j : j + 1], heads[:, j : j + 1], full_heads[j : j + 1])
        per_tag[tag] = {
            "positives": int(Y[:, j].sum()),
            "has_head_full": bool(full_heads[j]),
            "folds": [
                {
                    "fold": f["fold"],
                    "training_positives": f["training_positives"][j],
                    "has_head": f["has_head"][j],
                    "evaluation_positives": int(Y[folds == f["fold"], j].sum()),
                }
                for f in details
            ],
            **metrics,
        }
    return {
        "counts": {
            "pages": len(corpus.records),
            "evaluation_pages": len(rows),
            "untagged_pages": len(corpus.records) - len(rows),
            "forgotten_evaluation_pages": sum(bool(corpus.records[i]["forgotten"]) for i in rows),
            "tags": len(corpus.tags),
            "positives": int(Y.sum()),
            "heads_full": int(full_heads.sum()),
            "fallback_full": int((~full_heads).sum()),
            "heads_per_fold": [d["heads"] for d in details],
        },
        "metrics": summaries(Y, pred, heads, full_heads),
        "per_tag": per_tag,
        "folds": details,
        "predictions": [
            {
                "id": sha256_hex(corpus.records[i]["key"]),
                "fold": int(folds[n]),
                "truth": [corpus.tags[j] for j in np.flatnonzero(Y[n])],
                "top_3": rankings[n],
            }
            for n, i in enumerate(rows)
        ],
    }


def report(result: dict) -> str:
    c = result["counts"]
    lines = [
        "# Tag suggestion baseline",
        f"Setup: {OUTER_FOLDS} page folds, seed {SEED}; source {result['page_source']} ({result['inputs']['input']}), scorer {result['strategy']}; k={K}.",
        f"Counts: {c['pages']} pages; {c['evaluation_pages']} tagged evaluated; {c['untagged_pages']} untagged training reference only; {c['positives']} positives.",
        f"Tags: {c['tags']} active names; {c['heads_full']} full data heads, {c['fallback_full']} fallback; fold heads {c['heads_per_fold']}.",
        f"Truth: one positive per page and distinct active owner tag; retired tags excluded; forgotten tagged pages included ({c['forgotten_evaluation_pages']}), as app loading includes them.",
        "Fit: tagged training pages only; LR scaling, C and thresholds use app fit_rules; score mean, spread and unseen thresholds use all training pages.",
        "Ranking: app suggest for one held out page, all active tags; no label exclusion; top three shared by head and fallback groups.",
        "Macro averages pages (P on pages with group predictions; R/hit on group positive pages); micro pools associations; hit is a page rate in both. Zero denominators give 0.",
        "Tag macro P/R are also in JSON. Group P counts only predictions assigned to that group, without reranking. Groups below use each fold's fitted heads.",
        "| Group | Macro P@3 | Macro R@3 | Hit@3 | Micro P@3 | Micro R@3 |",
        "| --- | ---: | ---: | ---: | ---: | ---: |",
    ]
    groups = {"All": result["metrics"]["overall"], **result["metrics"]["by_fold_head"]}
    for name, m in groups.items():
        values = [m["macro"][k] for k in ("precision_at_3", "recall_at_3", "hit_at_3")]
        values += [m["micro"][k] for k in ("precision_at_3", "recall_at_3")]
        lines.append(f"| {name} | " + " | ".join(f"{v:.2%}" for v in values) + " |")
    lines += ["| Tag (15 largest) | Positives | Head full / folds | P@3 | R@3 |", "| --- | ---: | --- | ---: | ---: |"]
    for tag, t in sorted(result["per_tag"].items(), key=lambda item: (-item[1]["positives"], item[0].lower()))[:15]:
        m = t["overall"]["micro"]
        status = f"{'yes' if t['has_head_full'] else 'no'} / {sum(f['has_head'] for f in t['folds'])}"
        lines.append(
            f"| {tag.replace('|', '&#124;')} | {t['positives']} | {status} | {m['precision_at_3']:.2%} | {m['recall_at_3']:.2%} |"
        )
    fallback = result["metrics"]["by_full_data_head"]["fallback"]
    lines.append(
        f"Full data fallback: {c['fallback_full']} tags, {fallback['counts']['positives']} positives; pooled P@3 {fallback['micro']['precision_at_3']:.2%}, R@3 {fallback['micro']['recall_at_3']:.2%}."
    )
    lines.append(
        "Adaptation: load only inputs used by suggest; HeldOutScores freezes its mean on training pages; fold tag order follows training counts. Image, keyword and C search paths are unused."
    )
    lines += ["Run from repo root (reuse split; choose a fresh --out for a repeat):", f"`{result['command']}`"]
    return "\n".join(lines) + "\n"


def run(paths: Paths, root: Path, split: Path, out: Path, *, source="app", strategy="app", binary=kmt.BINARY) -> dict:
    _safe_paths(paths, root, out)
    if split.resolve().is_relative_to((Path.home() / ".knowmoretabs").resolve()):
        raise Invalid("scratch split path required")
    if any((out / name).exists() for name in ("baseline.json", "baseline.md")):
        raise Invalid("baseline outputs already exist; choose a separate output directory")
    logs = out / "baseline-logs"
    logs.mkdir(parents=True, exist_ok=True)
    with (logs / "tag-eval.log").open("a") as log, redirect_stdout(log), redirect_stderr(log):
        try:
            corpus = PAGE_SOURCES[source](paths, root, binary)
            saved = page_split(corpus, split)
            result = {
                "schema": 1,
                "source_revision": subprocess.run(
                    ["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True
                ).stdout.strip(),
                "seed": SEED,
                "k": K,
                "page_source": source,
                "strategy": strategy,
                "split_digest": fingerprint(saved),
                "inputs": corpus.metadata,
                **cross_validate(corpus, saved, strategy),
            }
            result["command"] = shlex.join(
                [
                    "uv",
                    "run",
                    "--locked",
                    "--project",
                    "lab/tagger",
                    "tagger",
                    "--root",
                    str(root),
                    "--data",
                    str(paths.data),
                    "tag-eval",
                    "--split",
                    str(split),
                    "--out",
                    str(out),
                    "--source",
                    source,
                    "--strategy",
                    strategy,
                    "--kmt-bin",
                    binary,
                ]
            )
            # Reject nonfinite metrics or scores before publishing any result.
            json.dumps(result, allow_nan=False)
            write_json(out / "baseline.json", result)
            (out / "baseline.md").write_text(report(result))
            return result
        except Exception as err:
            traceback.print_exc()
            raise Invalid(
                str(err) if isinstance(err, Invalid) else "unreadable inputs or unavailable runtime"
            ) from None
